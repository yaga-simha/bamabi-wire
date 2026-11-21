//! Per-World [`GuestInstance`] state container and `bamabi_init` / `bamabi_shutdown`
//! lifecycle helpers with zero process-global mutable state.

use core::ffi::{CStr, c_char, c_void};
#[cfg(not(kani))]
use core::sync::atomic::{AtomicPtr, Ordering};
#[cfg(not(kani))]
use std::collections::HashMap;
use std::ffi::CString;

use bamabi_sys::{
    BamComponent, BamHostApi, BamInstance, BamLoadCtx, BamManifest, BamStateWriterFn,
    ComponentDesc, ComponentUuid, FfiResult, ModComponent,
};

use crate::{
    component::{generic_drop_trampoline, register_component_raw, resolve_component},
    host::{Host, ValidatedLoadCtx},
    panic::{catch_guest_result, catch_guest_unit},
    trampoline::ReentrantCell,
};

type SaveStateHook = Box<dyn FnMut() -> Result<Vec<u8>, FfiResult> + Send + 'static>;
type RestoreStateHook = Box<dyn FnMut(&[u8]) -> FfiResult + Send + 'static>;

/// Type-erased handle to a guest-heap `Box<T>` tracked by [`GuestInstance`] or
/// [`RuntimeAllocTracker`].
///
/// Ensures strict allocator isolation: every closure, `CString`, or descriptor slice allocated by
/// the guest is reclaimed on the guest heap during `bamabi_shutdown` (or init rollback) without
/// leaking or crossing the host allocator boundary.
struct ErasedAllocation {
    ptr: *mut c_void,
    drop_fn: unsafe fn(*mut c_void),
}

// SAFETY: `ErasedAllocation` is constructed exclusively from `Box<T>` where `T: Send + 'static`.
unsafe impl Send for ErasedAllocation {}

impl Drop for ErasedAllocation {
    /// Reclaims the tracked allocation via its typed destructor inside [`catch_guest_unit`] so a
    /// panicking `Drop` implementation cannot unwind across `bamabi_shutdown`.
    fn drop(&mut self) {
        let ptr = self.ptr;
        let drop_fn = self.drop_fn;
        catch_guest_unit(move || {
            // SAFETY: `ptr` was allocated via `Box::into_raw` of the exact `T` paired with
            // `drop_fn` and is dropped exactly once here when `ErasedAllocation` drops.
            unsafe { drop_fn(ptr) };
        });
    }
}

/// Monomorphized destructor function stored in [`ErasedAllocation::drop_fn`] to reconstitute and
/// drop a `Box<T>` created by `Box::into_raw`.
unsafe fn drop_boxed<T>(ptr: *mut c_void) {
    // SAFETY: Caller guarantees `ptr` came from `Box::into_raw` for `T` and has not been freed.
    drop(unsafe { Box::from_raw(ptr.cast::<T>()) });
}

/// Thread-safe append-only tracker for runtime observer/closure allocations created after
/// `bamabi_init` (such as `ValidatedCommands::observe`), owned by [`GuestInstance`] and
/// drained in LIFO order during `bamabi_shutdown` prior to init-time allocations.
///
/// Also hosts the per-instance `"bam.v0.*"` resolved symbol/VTable cache shared across all
/// [`Host`] handles belonging to the same [`GuestInstance`].
pub(crate) struct RuntimeAllocTracker {
    #[cfg(not(kani))]
    pinned_host: AtomicPtr<Host>,
    #[cfg(kani)]
    pinned_host: core::cell::Cell<*mut Host>,
    #[cfg(not(kani))]
    allocations: std::sync::Mutex<Vec<ErasedAllocation>>,
    #[cfg(kani)]
    allocations: core::cell::RefCell<Vec<ErasedAllocation>>,
    #[cfg(not(kani))]
    symbol_cache: std::sync::Mutex<HashMap<(core::any::TypeId, u8), GuestSymbolPtr>>,
}

#[cfg(kani)]
// SAFETY: Kani verification is single-threaded; `RefCell` enforces dynamic borrow safety.
unsafe impl Sync for RuntimeAllocTracker {}

impl RuntimeAllocTracker {
    /// Creates an empty mutex-guarded [`RuntimeAllocTracker`].
    fn new() -> Self {
        Self {
            #[cfg(not(kani))]
            pinned_host: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(kani)]
            pinned_host: core::cell::Cell::new(core::ptr::null_mut()),
            #[cfg(not(kani))]
            allocations: std::sync::Mutex::new(Vec::new()),
            #[cfg(kani)]
            allocations: core::cell::RefCell::new(Vec::new()),
            #[cfg(not(kani))]
            symbol_cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Looks up a cached `"bam.v0.*"` symbol or VTable pointer keyed by `(TypeId, kind)`, calling
    /// `resolve` outside the lock on a cache miss and storing only non-null `Ok` pointers.
    #[cfg(not(kani))]
    pub(crate) fn cached_symbol_lookup(
        &self,
        key: (core::any::TypeId, u8),
        resolve: impl FnOnce() -> Result<*const c_void, FfiResult>,
    ) -> Result<*const c_void, FfiResult> {
        {
            let guard = self
                .symbol_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(&cached) = guard.get(&key) {
                return Ok(cached.0);
            }
        }
        let ptr = resolve()?;
        if !ptr.is_null() {
            let mut guard = self
                .symbol_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(key, GuestSymbolPtr(ptr));
        }
        Ok(ptr)
    }

    /// Binds the owning [`GuestInstance`]'s stable heap-pinned [`Host`] pointer so runtime
    /// observer allocations can borrow it without duplicating the 584-byte `Host` table.
    #[inline]
    fn set_pinned_host(&self, ptr: *mut Host) {
        #[cfg(not(kani))]
        self.pinned_host.store(ptr, Ordering::Release);
        #[cfg(kani)]
        self.pinned_host.set(ptr);
    }

    /// Returns the owning [`GuestInstance`]'s stable `*const Host` pointer (`null` if unbound).
    #[inline]
    pub(crate) fn pinned_host_ptr(&self) -> *const Host {
        #[cfg(not(kani))]
        {
            self.pinned_host.load(Ordering::Acquire).cast_const()
        }
        #[cfg(kani)]
        {
            self.pinned_host.get().cast_const()
        }
    }

    /// Converts `boxed` into a stable raw pointer, records its type-erased destructor under the
    /// mutex (recovering from poison if a prior thread panicked), and returns the raw pointer.
    pub(crate) fn track<T: Send + 'static>(&self, boxed: Box<T>) -> *mut T {
        let raw = Box::into_raw(boxed);
        #[cfg(not(kani))]
        let mut guard = self
            .allocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(kani)]
        let mut guard = self.allocations.borrow_mut();
        guard.push(ErasedAllocation {
            ptr: raw.cast::<c_void>(),
            drop_fn: drop_boxed::<T>,
        });
        raw
    }

    /// Returns the number of runtime allocations currently tracked.
    fn len(&self) -> usize {
        #[cfg(not(kani))]
        {
            self.allocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
        }
        #[cfg(kani)]
        {
            self.allocations.borrow().len()
        }
    }

    /// Drains and drops all tracked runtime allocations in reverse (LIFO) registration order.
    ///
    /// Swaps the vector out of the mutex before dropping so destructors cannot deadlock on
    /// re-acquiring the lock.
    fn drain_lifo(&self) {
        let mut drained = {
            #[cfg(not(kani))]
            let mut guard = self
                .allocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(kani)]
            let mut guard = self.allocations.borrow_mut();
            core::mem::take(&mut *guard)
        };
        while let Some(alloc) = drained.pop() {
            drop(alloc);
        }
    }
}

/// Raw-pointer owner for a heap-pinned [`RuntimeAllocTracker`] inside [`GuestInstance`].
///
/// Uses `Box::into_raw` instead of `Box<RuntimeAllocTracker>` so moving `GuestInstance` (for
/// example when boxing it at the end of `init_guest_instance`) does not retag or invalidate the
/// shared `*const RuntimeAllocTracker` pointer stored inside [`Host`] under Miri Tree Borrows.
struct OwnedRuntimeAllocs(*mut RuntimeAllocTracker);

// SAFETY: `OwnedRuntimeAllocs` uniquely owns a heap-allocated `RuntimeAllocTracker` created via
// `Box::into_raw` (avoiding `Box` `Unique` retagging when `GuestInstance` moves) and freed
// exclusively in `OwnedRuntimeAllocs::drop` after all `Host` copies inside `GuestInstance`
// closures/hooks have already finished dropping.
unsafe impl Send for OwnedRuntimeAllocs {}

impl OwnedRuntimeAllocs {
    /// Heap-allocates a fresh [`RuntimeAllocTracker`] with stable raw provenance.
    fn new() -> Self {
        Self(Box::into_raw(Box::new(RuntimeAllocTracker::new())))
    }

    /// Returns the stable `*const RuntimeAllocTracker` pointer stamped onto [`Host`].
    #[inline]
    const fn as_ptr(&self) -> *const RuntimeAllocTracker {
        self.0.cast_const()
    }

    /// Returns a shared reference to the underlying [`RuntimeAllocTracker`].
    #[inline]
    fn tracker(&self) -> &RuntimeAllocTracker {
        // SAFETY: `self.0` was allocated via `Box::into_raw` in `OwnedRuntimeAllocs::new` and
        // remains alive and immutable (internally `Mutex`-synchronized) until `Drop`.
        unsafe { &*self.0 }
    }
}

impl Drop for OwnedRuntimeAllocs {
    /// Reclaims the heap-allocated [`RuntimeAllocTracker`] after `GuestInstance::drop` has
    /// already dropped every closure holding a [`Host`] reference to it.
    fn drop(&mut self) {
        // SAFETY: `self.0` was allocated via `Box::into_raw(Box::new(RuntimeAllocTracker))` and
        // `GuestInstance::drop` already tore down every closure/hook holding a `Host` pointer to it.
        drop(unsafe { Box::from_raw(self.0) });
    }
}

/// Raw-pointer owner for immutable UTF-8 manifest name bytes inside [`GuestInstance`].
///
/// Uses `Box::into_raw` on `Box<[u8]>` so moving `GuestInstance` preserves pointer provenance
/// for the `*const u8` copy embedded in [`Host`] under Miri Tree Borrows.
struct OwnedManifestBytes {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: `OwnedManifestBytes` uniquely owns a heap-allocated immutable UTF-8 `[u8]` buffer (when
// `len > 0`) created via `Box::into_raw` and freed exclusively in `OwnedManifestBytes::drop`.
unsafe impl Send for OwnedManifestBytes {}

impl OwnedManifestBytes {
    /// Converts `name` into a pinned raw UTF-8 byte allocation (or a dangling zero-length handle
    /// when `name` is empty).
    fn new(name: String) -> Self {
        let len = name.len();
        if len == 0 {
            Self {
                ptr: core::ptr::NonNull::<u8>::dangling().as_ptr(),
                len: 0,
            }
        } else {
            let boxed: Box<[u8]> = name.into_bytes().into_boxed_slice();
            Self {
                ptr: Box::into_raw(boxed).cast::<u8>(),
                len,
            }
        }
    }
}

impl Drop for OwnedManifestBytes {
    /// Frees the pinned UTF-8 byte slice after all closures referencing [`Host::manifest_name`]
    /// have been dropped.
    fn drop(&mut self) {
        if self.len > 0 {
            // SAFETY: `self.ptr` was allocated via `Box::into_raw` of a `Box<[u8]>` of length
            // `self.len` in `OwnedManifestBytes::new` and all `Host` copies inside `GuestInstance`
            // closures/hooks have already finished dropping before this field drops.
            drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(self.ptr, self.len)) });
        }
    }
}

/// Raw-pointer owner for a heap-pinned [`Host`] inside [`GuestInstance`].
///
/// Uses `Box::into_raw` instead of `Box<Host>` so moving `GuestInstance` (for example when
/// boxing it at the end of `init_guest_instance`) does not retag or invalidate `*const Host`
/// pointers stored in closure and observer trampolines under Miri Stacked/Tree Borrows.
struct OwnedPinnedHost(*mut Host);

// SAFETY: `OwnedPinnedHost` uniquely owns a heap-allocated immutable `Host` created via
// `Box::into_raw` (avoiding `Box` `Unique` retagging when `GuestInstance` moves) and freed
// exclusively in `OwnedPinnedHost::drop` after `GuestInstance::drop` has torn down every
// closure, observer, and hook holding a `*const Host` pointer to it.
unsafe impl Send for OwnedPinnedHost {}
// SAFETY: Shared references only expose immutable `&Host` / `*const Host`; `Host` is `Send + Sync`.
unsafe impl Sync for OwnedPinnedHost {}

impl OwnedPinnedHost {
    /// Heap-allocates `host` with stable raw provenance.
    fn new(host: Host) -> Self {
        Self(Box::into_raw(Box::new(host)))
    }

    /// Returns the raw `*mut Host` allocation handle for binding onto [`RuntimeAllocTracker`].
    #[inline]
    const fn as_mut_ptr(&self) -> *mut Host {
        self.0
    }

    /// Returns the stable `*const Host` pointer embedded in per-instance trampolines.
    #[inline]
    const fn as_ptr(&self) -> *const Host {
        self.0.cast_const()
    }

    /// Returns a shared reference to the pinned [`Host`].
    #[inline]
    const fn as_ref(&self) -> &Host {
        // SAFETY: `self.0` was allocated via `Box::into_raw(Box::new(host))` in
        // `OwnedPinnedHost::new` and remains alive and unmutated until `Drop`.
        unsafe { &*self.0 }
    }
}

impl Drop for OwnedPinnedHost {
    /// Reclaims the heap-allocated [`Host`] after `GuestInstance::drop` has already torn down
    /// every closure, observer, and hook referencing it.
    fn drop(&mut self) {
        // SAFETY: `self.0` was allocated via `Box::into_raw(Box::new(host))` and
        // `GuestInstance::drop` already tore down every closure/hook holding `*const Host`.
        drop(unsafe { Box::from_raw(self.0) });
    }
}

/// `Send + Sync` newtype wrapping a `'static` guest-exported `"bam.v0.*"` function or VTable
/// address stored in [`GuestInstance::symbols`].
#[derive(Clone, Copy)]
struct GuestSymbolPtr(*const c_void);

// SAFETY: `GuestSymbolPtr` stores `'static` function/VTable pointers owned by this guest binary
// and valid for the lifetime of the loaded library.
unsafe impl Send for GuestSymbolPtr {}
// SAFETY: Shared read-only symbol pointers are immutable across threads.
unsafe impl Sync for GuestSymbolPtr {}

/// Heap-allocated per-World guest mod instance returned via `*instance_out` from
/// `bamabi_init` and reclaimed in `bamabi_shutdown`.
///
/// Owns the validated [`Host`] table, all registered system/`run_if` closure states, owned
/// descriptor strings, per-instance `"bam.v0.*"` symbols, hot-reload state hooks, and per-instance
/// shutdown hooks so loading the same `.cdylib` into multiple Bevy `World`s simultaneously shares
/// zero mutable global state.
pub struct GuestInstance {
    allocations: Vec<ErasedAllocation>,
    shutdown_hooks: Vec<Box<dyn FnOnce() + Send + 'static>>,
    save_state_hook: Option<ReentrantCell<SaveStateHook>>,
    restore_state_hook: Option<ReentrantCell<RestoreStateHook>>,
    #[cfg(not(kani))]
    symbols: HashMap<String, GuestSymbolPtr>,
    #[cfg(kani)]
    symbols: Vec<(String, GuestSymbolPtr)>,
    // Declared last so field drop runs strictly after `GuestInstance::drop` tears down every
    // closure/hook/observer holding `*const Host` or a `Host` copy. `pinned_host` drops before
    // `_runtime_allocs` and `_manifest_bytes` because `Host` points into those two allocations.
    pinned_host: OwnedPinnedHost,
    _runtime_allocs: OwnedRuntimeAllocs,
    _manifest_bytes: OwnedManifestBytes,
}

impl GuestInstance {
    /// Creates an empty [`GuestInstance`] bound to `host` and `manifest_name`.
    pub fn new(host: Host, manifest_name: impl Into<String>) -> Self {
        let manifest_bytes = OwnedManifestBytes::new(manifest_name.into());
        let runtime_allocs = OwnedRuntimeAllocs::new();
        // SAFETY: When `manifest_bytes.len > 0`, `manifest_bytes.ptr` points to immutable, valid
        // UTF-8 heap bytes that remain pinned at stable provenance until `_manifest_bytes` drops
        // after `pinned_host`, `shutdown_hooks`, `_runtime_allocs`, and `allocations`. Similarly,
        // `runtime_allocs.as_ptr()` was created via `Box::into_raw` and never retags on move,
        // staying valid until `_runtime_allocs` drops after `pinned_host` and all closures finish.
        let host = unsafe {
            host.with_manifest_name(manifest_bytes.ptr.cast_const(), manifest_bytes.len)
                .with_runtime_allocs(runtime_allocs.as_ptr())
        };
        let pinned_host = OwnedPinnedHost::new(host);
        runtime_allocs
            .tracker()
            .set_pinned_host(pinned_host.as_mut_ptr());
        Self {
            allocations: Vec::new(),
            shutdown_hooks: Vec::new(),
            save_state_hook: None,
            restore_state_hook: None,
            #[cfg(not(kani))]
            symbols: HashMap::new(),
            #[cfg(kani)]
            symbols: Vec::new(),
            pinned_host,
            _runtime_allocs: runtime_allocs,
            _manifest_bytes: manifest_bytes,
        }
    }

    /// Returns the cached [`Host`] table for this mod instance.
    #[inline]
    pub const fn host(&self) -> &Host {
        self.pinned_host.as_ref()
    }

    /// Returns the stable heap-pinned `*const Host` pointer owned by this [`GuestInstance`].
    #[inline]
    pub(crate) const fn pinned_host_ptr(&self) -> *const Host {
        self.pinned_host.as_ptr()
    }

    /// Returns the canonical mod name declared in [`BamManifest::name`].
    #[inline]
    pub fn manifest_name(&self) -> &str {
        self.pinned_host.as_ref().manifest_name()
    }

    /// Transfers ownership of `boxed` into this instance, returning a stable raw pointer valid
    /// until `bamabi_shutdown` (or until rollback if `bamabi_init` fails).
    pub fn track_allocation<T: Send + 'static>(&mut self, boxed: Box<T>) -> *mut T {
        let raw = Box::into_raw(boxed);
        self.allocations.push(ErasedAllocation {
            ptr: raw.cast::<c_void>(),
            drop_fn: drop_boxed::<T>,
        });
        raw
    }

    /// Registers a per-instance shutdown hook executed in reverse registration (LIFO) order during
    /// `bamabi_shutdown` before closure allocations are freed.
    ///
    /// Shutdown hooks registered during a failed `bamabi_init` are discarded without running
    /// during transactional rollback.
    pub fn add_shutdown_hook<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(hook));
    }

    /// Registers or replaces the hot-reload state snapshot callback for this instance.
    pub fn set_save_state_hook<F>(&mut self, hook: F)
    where
        F: FnMut() -> Result<Vec<u8>, FfiResult> + Send + 'static,
    {
        self.save_state_hook = Some(ReentrantCell::new(Box::new(hook)));
    }

    /// Registers or replaces the hot-reload state restoration callback for this instance.
    pub fn set_restore_state_hook<F>(&mut self, hook: F)
    where
        F: FnMut(&[u8]) -> FfiResult + Send + 'static,
    {
        self.restore_state_hook = Some(ReentrantCell::new(Box::new(hook)));
    }

    /// Returns the number of tracked heap allocations owned by this instance.
    #[inline]
    pub fn allocation_count(&self) -> usize {
        self.allocations.len() + self._runtime_allocs.tracker().len()
    }

    /// Invokes all registered per-instance shutdown hooks in LIFO order, catching any panics.
    pub fn run_shutdown_hooks(&mut self) {
        while let Some(hook) = self.shutdown_hooks.pop() {
            catch_guest_unit(hook);
        }
    }

    /// Registers a `"bam.v0.*"` symbol address in this instance's symbol table.
    pub fn register_symbol(&mut self, name: impl Into<String>, ptr: *const c_void) {
        if !ptr.is_null() {
            #[cfg(not(kani))]
            self.symbols.insert(name.into(), GuestSymbolPtr(ptr));
            #[cfg(kani)]
            {
                let key = name.into();
                if let Some(entry) = self.symbols.iter_mut().find(|(k, _)| *k == key) {
                    entry.1 = GuestSymbolPtr(ptr);
                } else {
                    self.symbols.push((key, GuestSymbolPtr(ptr)));
                }
            }
        }
    }

    /// Resolves a `"bam.v0.*"` symbol previously registered on this instance.
    #[inline]
    pub fn resolve_symbol(&self, name: &str) -> Option<*const c_void> {
        #[cfg(not(kani))]
        {
            self.symbols.get(name).map(|entry| entry.0)
        }
        #[cfg(kani)]
        {
            self.symbols
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, entry)| entry.0)
        }
    }
}

impl Drop for GuestInstance {
    /// Tears down hooks, symbol tables, runtime observer allocations, and init-time allocations
    /// in strict LIFO order before Rust drops `_runtime_allocs` and `_manifest_bytes`.
    fn drop(&mut self) {
        // Drop tracked allocations in reverse (LIFO) registration order so later closures or
        // descriptors drop before earlier dependencies. Unrun `shutdown_hooks` (from failed init
        // rollback) are dropped without invoking them.
        self.shutdown_hooks.clear();
        self.save_state_hook = None;
        self.restore_state_hook = None;
        self.symbols.clear();
        self._runtime_allocs.tracker().drain_lifo();
        while let Some(alloc) = self.allocations.pop() {
            drop(alloc);
        }
    }
}

/// Pure-Rust NUL-terminated `CStr` wrapper for Kani verification (avoids libc `strlen`).
///
/// # Safety
/// `ptr` must be non-null and point to a valid NUL-terminated byte sequence.
#[cfg(kani)]
unsafe fn manifest_cstr_from_ptr<'a>(ptr: *const c_char) -> &'a CStr {
    let mut len = 0usize;
    loop {
        // SAFETY: Caller guarantees `ptr` points to a NUL-terminated C string.
        if unsafe { *ptr.add(len) } == 0 {
            // SAFETY: Slice `0..=len` ends with a single NUL terminator at `len`.
            return unsafe {
                CStr::from_bytes_with_nul_unchecked(core::slice::from_raw_parts(
                    ptr.cast::<u8>(),
                    len + 1,
                ))
            };
        }
        len += 1;
    }
}

/// Panic-safe `bamabi_init` driver constructing a per-World [`GuestInstance`] and writing it
/// to `*instance_out` on [`FfiResult::OK`].
///
/// If `api` or `manifest` fails validation, `build_fn` returns a non-`OK` [`FfiResult`], or
/// `build_fn` panics, all staged allocations inside [`GuestInstance`] are immediately rolled back
/// on the guest heap and `*instance_out` remains `null`.
///
/// # Safety
/// - If `instance_out` is non-null, it must point to writable `*mut BamInstance` storage.
/// - `api` and `load_ctx` must satisfy [`Host::from_load_ctx`] when non-null.
/// - `manifest.name` and `manifest.version`, when non-null, must point to NUL-terminated C strings.
pub unsafe fn init_guest_instance<F>(
    api: *const BamHostApi,
    load_ctx: *mut BamLoadCtx,
    instance_out: *mut *mut BamInstance,
    manifest: &BamManifest,
    build_fn: F,
) -> FfiResult
where
    F: FnOnce(&mut GuestInstance, ValidatedLoadCtx<'_>) -> FfiResult,
{
    catch_guest_result(|| {
        if instance_out.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !instance_out.is_aligned() {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: Verified `instance_out` is non-null, aligned, and writable per caller contract;
        // initialize to null upfront so any early error or panic leaves `*instance_out == null`.
        unsafe {
            *instance_out = core::ptr::null_mut();
        }

        let manifest_status = manifest.validate();
        if !manifest_status.is_ok() {
            return manifest_status;
        }
        #[cfg(not(kani))]
        // SAFETY: `manifest.validate()` confirmed `manifest.name` is non-null, and caller guarantees
        // it points to a NUL-terminated C string.
        let manifest_cstr = unsafe { CStr::from_ptr(manifest.name) };
        #[cfg(kani)]
        // SAFETY: `manifest.validate()` confirmed `manifest.name` is non-null, and caller guarantees
        // it points to a NUL-terminated C string.
        let manifest_cstr = unsafe { manifest_cstr_from_ptr(manifest.name) };
        let Ok(manifest_name) = manifest_cstr.to_str() else {
            return FfiResult::INVALID_UTF8;
        };
        if manifest_name.is_empty() {
            return FfiResult::INVALID_ARGUMENT;
        }

        // SAFETY: Forwarding `api` and `load_ctx` under caller's `bamabi_init` contract.
        let host = match unsafe { Host::from_load_ctx(api, load_ctx) } {
            Ok(h) => h,
            Err(err) => return err,
        };

        let mut instance = GuestInstance::new(host, manifest_name);
        let host_copy = *instance.host();
        // SAFETY: `load_ctx` was verified non-null in `Host::from_load_ctx` and remains valid for
        // the duration of `init_guest_instance`.
        let validated_load = match unsafe { ValidatedLoadCtx::from_raw(load_ctx, &host_copy) } {
            Ok(ctx) => ctx,
            Err(err) => return err,
        };

        let status = build_fn(&mut instance, validated_load);
        if !status.is_ok() {
            drop(instance);
            return status;
        }

        let raw_instance = Box::into_raw(Box::new(instance)).cast::<BamInstance>();
        // SAFETY: `instance_out` is non-null and writable; transfers ownership to the host until
        // `shutdown_guest_instance` is called with `raw_instance`.
        unsafe {
            *instance_out = raw_instance;
        }
        FfiResult::OK
    })
}

/// Panic-safe `bamabi_shutdown` driver executing per-instance shutdown hooks in LIFO order
/// and freeing the [`GuestInstance`] returned by [`init_guest_instance`].
///
/// Passing a `null` `instance` pointer is a defined no-op.
///
/// # Safety
/// If `instance` is non-null, it must have been produced by [`init_guest_instance`] in this guest
/// binary and must not have been shut down previously or remain referenced by running systems.
pub unsafe fn shutdown_guest_instance(instance: *mut BamInstance) {
    if instance.is_null()
        || !(instance as usize).is_multiple_of(core::mem::align_of::<GuestInstance>())
    {
        return;
    }
    catch_guest_unit(|| {
        // SAFETY: Caller guarantees `instance` was allocated via `Box::into_raw(Box::new(GuestInstance))`
        // in `init_guest_instance` and is uniquely reclaimed here.
        let mut boxed = unsafe { Box::from_raw(instance.cast::<GuestInstance>()) };
        boxed.run_shutdown_hooks();
        drop(boxed);
    });
}

/// Panic-safe `BamSaveStateFn` trampoline exporting guest state into the host-owned writer.
///
/// # Safety
/// - When `instance` is non-null, it must point to a live [`GuestInstance`] allocated by
///   [`init_guest_instance`] in this guest binary.
/// - `writer` and `writer_ctx` must satisfy [`BamStateWriterFn`] for the duration of this call.
pub unsafe extern "C" fn guest_save_state_trampoline(
    instance: *mut BamInstance,
    writer: Option<BamStateWriterFn>,
    writer_ctx: *mut c_void,
) -> FfiResult {
    catch_guest_result(|| {
        let Some(writer_fn) = writer else {
            return FfiResult::NULL_POINTER;
        };
        if instance.is_null() {
            return FfiResult::OK;
        }
        if !(instance as usize).is_multiple_of(core::mem::align_of::<GuestInstance>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `instance` is non-null, aligned, and points to a live `GuestInstance` prior to `bamabi_shutdown`.
        let inst = unsafe { &*instance.cast::<GuestInstance>() };
        let Some(ref hook_cell) = inst.save_state_hook else {
            return FfiResult::OK;
        };
        let Some(blob_res) = hook_cell.try_with_mut(|hook| hook()) else {
            return FfiResult::ACCESS_CONFLICT;
        };
        let bytes = match blob_res {
            Ok(b) => b,
            Err(err) => return err,
        };
        if bytes.is_empty() {
            return FfiResult::OK;
        }
        // SAFETY: `writer_fn` copies `(bytes.as_ptr(), bytes.len())` into host-owned storage before
        // returning; `bytes` is dropped on the guest heap immediately afterward.
        unsafe { writer_fn(writer_ctx, bytes.as_ptr(), bytes.len()) }
    })
}

/// Panic-safe `BamRestoreStateFn` trampoline restoring host-buffered state into a newly initialized
/// [`GuestInstance`].
///
/// # Safety
/// - When `instance` is non-null, it must point to a live [`GuestInstance`] allocated by
///   [`init_guest_instance`] in this guest binary.
/// - When `len > 0`, `bytes` must be non-null and point to `len` initialized readable bytes.
pub unsafe extern "C" fn guest_restore_state_trampoline(
    instance: *mut BamInstance,
    bytes: *const u8,
    len: usize,
) -> FfiResult {
    catch_guest_result(|| {
        if len > 0 && bytes.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if len > isize::MAX as usize {
            return FfiResult::OUT_OF_RANGE;
        }
        if instance.is_null() {
            return FfiResult::OK;
        }
        if !(instance as usize).is_multiple_of(core::mem::align_of::<GuestInstance>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `instance` is non-null, aligned, and points to a live `GuestInstance` after `bamabi_init`.
        let inst = unsafe { &*instance.cast::<GuestInstance>() };
        let Some(ref hook_cell) = inst.restore_state_hook else {
            return FfiResult::OK;
        };
        let Some(status) = hook_cell.try_with_mut(|hook| {
            let slice = if len == 0 {
                &[]
            } else {
                // SAFETY: Verified `len <= isize::MAX` and `bytes` is non-null; caller guarantees
                // `bytes` covers `len` initialized readable bytes for this call.
                unsafe { core::slice::from_raw_parts(bytes, len) }
            };
            hook(slice)
        }) else {
            return FfiResult::ACCESS_CONFLICT;
        };
        status
    })
}

/// Panic-safe `bamabi_resolve_symbol` driver resolving a `"bam.v0.*"` symbol previously
/// registered on `instance`.
///
/// Returns `null` if `instance` or `name` is null, `name` is not valid UTF-8, or no symbol matches.
///
/// # Safety
/// - If `instance` is non-null, it must point to a live [`GuestInstance`] produced by
///   [`init_guest_instance`] in this guest binary.
/// - If `name` is non-null, it must point to a NUL-terminated C string.
pub unsafe extern "C" fn resolve_guest_instance_symbol(
    instance: *const BamInstance,
    name: *const c_char,
) -> *const c_void {
    let mut out = core::ptr::null();
    catch_guest_unit(|| {
        if instance.is_null()
            || name.is_null()
            || !(instance as usize).is_multiple_of(core::mem::align_of::<GuestInstance>())
        {
            return;
        }
        // SAFETY: Caller guarantees `name` is a valid NUL-terminated C string for this call.
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else {
            return;
        };
        // SAFETY: Verified non-null, aligned `instance` pointing to a live `GuestInstance` prior to shutdown.
        let inst = unsafe { &*instance.cast::<GuestInstance>() };
        if let Some(ptr) = inst.resolve_symbol(name_str) {
            out = ptr;
        }
    });
    out
}

/// Computes the manifest-qualified `(canonical_name, ComponentUuid)` pair for `T: ModComponent`.
pub fn qualify_mod_component_identity<T: ModComponent>(
    manifest_name: &str,
) -> Result<(String, ComponentUuid), FfiResult> {
    let raw_name = T::CANONICAL_NAME;
    if raw_name.is_empty() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    if raw_name.contains("::") || manifest_name.is_empty() {
        return Ok((raw_name.to_string(), T::UUID));
    }
    let qualified = format!("{manifest_name}::{raw_name}");
    let default_canonical_uuid = ComponentUuid::from_canonical_name(raw_name);
    let default_parts_uuid = ComponentUuid::from_parts(
        raw_name,
        core::mem::size_of::<T>(),
        core::mem::align_of::<T>(),
        0,
    );
    let uuid = if T::UUID == default_canonical_uuid {
        ComponentUuid::from_canonical_name(&qualified)
    } else if T::UUID == default_parts_uuid {
        ComponentUuid::from_parts(
            &qualified,
            core::mem::size_of::<T>(),
            core::mem::align_of::<T>(),
            0,
        )
    } else {
        T::UUID
    };
    Ok((qualified, uuid))
}

/// Resolves a dynamic [`ModComponent`] handle on `host`, trying the manifest-qualified name first
/// when `T::CANONICAL_NAME` lacks `"::"` and falling back to `(T::COMPONENT_CSTR, T::UUID)`.
pub fn resolve_mod_component<T: ModComponent>(host: &Host) -> Result<BamComponent, FfiResult> {
    let (qualified, uuid) = qualify_mod_component_identity::<T>(host.manifest_name())?;
    let Ok(qualified_cstr) = CString::new(qualified.as_bytes()) else {
        return Err(FfiResult::INVALID_ARGUMENT);
    };
    match resolve_component(host, Some(qualified_cstr.as_c_str()), Some(uuid)) {
        Ok(comp) => Ok(comp),
        Err(FfiResult::NOT_FOUND) if qualified != T::CANONICAL_NAME => {
            resolve_component(host, Some(T::COMPONENT_CSTR), Some(T::UUID))
        }
        Err(err) => Err(err),
    }
}

/// Panic-safe `extern "C"` default-constructor trampoline writing `T::default()` into `dst`.
///
/// # Safety
/// When `size_of::<T>() > 0`, `dst` must be non-null, aligned to `align_of::<T>()`, and point to
/// `size_of::<T>()` writable bytes whose ownership transfers to the caller on [`FfiResult::OK`].
pub(crate) unsafe extern "C" fn init_default_trampoline<T: Default>(dst: *mut c_void) -> FfiResult {
    catch_guest_result(|| {
        let size = core::mem::size_of::<T>();
        if size == 0 {
            let val = T::default();
            core::mem::forget(val);
            return FfiResult::OK;
        }
        if dst.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(dst as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        let val = T::default();
        // SAFETY: Verified `dst` is non-null, aligned for `T`, and writable for `size_of::<T>()`
        // bytes; ownership transfers to the host caller.
        unsafe {
            core::ptr::write(dst.cast::<T>(), val);
        }
        FfiResult::OK
    })
}

impl<'a> ValidatedLoadCtx<'a> {
    /// Registers a non-POD (or POD) [`ModComponent`] dynamic component `T` with automatic
    /// manifest qualification, panic-guarded guest `ComponentDropFn`, `T::erased_vtable_ptr()`,
    /// `"bam.v0.<name>.init_default"`, and all `T::symbols()` (`vtable`, `field_info`,
    /// `export_fields`, `import_field`, getters/setters) stored in [`GuestInstance`].
    pub fn register_dynamic_mod_component<T: ModComponent + Default + Send + Sync + 'static>(
        &self,
        instance: &mut GuestInstance,
        flags: u32,
    ) -> Result<BamComponent, FfiResult> {
        let (qualified_name, uuid) = qualify_mod_component_identity::<T>(instance.manifest_name())?;
        let Ok(owned_name) = CString::new(qualified_name.as_bytes()) else {
            return Err(FfiResult::INVALID_ARGUMENT);
        };
        let raw_name = instance.track_allocation(Box::new(owned_name));
        // SAFETY: `raw_name` was allocated via `instance.track_allocation` and remains pinned and
        // valid until `bamabi_shutdown` (or rollback).
        let name_ptr = unsafe { (*raw_name).as_ptr() };

        let mut desc = ComponentDesc::new(
            name_ptr,
            uuid,
            core::mem::size_of::<T>(),
            core::mem::align_of::<T>(),
        )
        .with_flags(flags | T::COMPONENT_FLAGS)
        .with_vtable(T::erased_vtable_ptr())
        .with_schema_fingerprint(T::SCHEMA_FINGERPRINT);

        if !T::BAM_FIELDS.is_empty() {
            desc = desc.with_fields(T::BAM_FIELDS.as_ptr(), T::BAM_FIELDS.len());
        }
        if !T::REQUIRED_COMPONENTS.is_empty() {
            desc = desc.with_required_init_descs(
                T::REQUIRED_COMPONENTS.as_ptr(),
                T::REQUIRED_COMPONENTS.len(),
            );
        }

        if core::mem::needs_drop::<T>() {
            desc = desc.with_drop(generic_drop_trampoline::<T>);
        }

        // SAFETY: `self.as_raw()` was validated in `ValidatedLoadCtx::from_raw`, `name_ptr` is
        // owned by `instance`, `T::erased_vtable_ptr()` is `'static`, and `generic_drop_trampoline`
        // catches panics and drops `T` in place on the guest heap.
        let handle = unsafe { register_component_raw(self.as_raw(), self.host(), &desc) }?;

        let canonical = T::CANONICAL_NAME;
        let canonical_prefix = format!("bam.v0.{canonical}.");
        for entry in T::symbols() {
            instance.register_symbol(entry.name, entry.ptr);
            if qualified_name != canonical
                && let Some(suffix) = entry.name.strip_prefix(&canonical_prefix)
            {
                instance.register_symbol(format!("bam.v0.{qualified_name}.{suffix}"), entry.ptr);
            }
        }

        let init_ptr = init_default_trampoline::<T> as *const c_void;
        instance.register_symbol(format!("bam.v0.{canonical}.init_default"), init_ptr);
        if qualified_name != canonical {
            instance.register_symbol(format!("bam.v0.{qualified_name}.init_default"), init_ptr);
        }

        Ok(handle)
    }
}
