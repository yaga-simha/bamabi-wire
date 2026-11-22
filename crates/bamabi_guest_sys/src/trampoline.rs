//! Lock-free re-entrancy-guarded guest closure trampolines (`BamSystemFn` and `BamRunIfFn`) and
//! unified [`BamSystemDesc`] registration.

use core::{
    cell::{Cell, UnsafeCell},
    ffi::{CStr, c_char, c_void},
    mem::{ManuallyDrop, MaybeUninit},
    sync::atomic::{AtomicBool, Ordering},
};
use std::ffi::CString;

use bamabi_sys::{
    AbiPod, BAM_STORAGE_FLAG_IMMUTABLE, BAM_SYSTEM_KNOWN_FLAGS, BamChangeTicks, BamCommands,
    BamComponent, BamEventKind, BamHostCtx, BamIter, BamLayoutInfo, BamLoadCtx, BamObserverDesc,
    BamObserverFn, BamObserverMutFn, BamQueryDesc, BamRunIfFn, BamScheduleSetDesc, BamSymbol,
    BamSystem, BamSystemCtx, BamSystemDesc, BamSystemFn, BamSystemSet, BamTermDesc, ComponentUuid,
    FfiBool, FfiEntity, FfiResult, MAX_QUERIES_PER_SYSTEM, MAX_TERMS_PER_QUERY, ModComponent,
    ScheduleId,
};

use crate::{
    commands::{SpawnEntry, ValidatedCommands, validate_spawn_entry},
    host::{Host, ValidatedWireVTable},
    instance::{GuestInstance, qualify_mod_component_identity},
    iter::{QuerySlotScratch, UuidFilter, ValidatedIter},
    panic::{catch_guest_bool, catch_guest_result},
};

/// RAII guard that resets [`ReentrantCell::in_use`] to `false` with `Ordering::Release` on drop,
/// ensuring the cell unlocks even if the guarded guest closure unwinds.
struct ReleaseGuard<'a>(&'a AtomicBool);

impl Drop for ReleaseGuard<'_> {
    /// Clears the re-entrancy lock flag with `Ordering::Release`.
    #[inline]
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Lock-free single-entry cell guarding `FnMut` system and `run_if` closures across FFI calls.
///
/// Uses an `Acquire`/`Release` [`AtomicBool`] re-entrancy flag instead of `std::sync::Mutex` so
/// hot per-frame system dispatch avoids OS mutex operations while deterministically rejecting
/// accidental re-entrant or concurrent entry without undefined behavior.
pub struct ReentrantCell<T> {
    in_use: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `T: Send` permits moving the cell across threads.
unsafe impl<T: Send> Send for ReentrantCell<T> {}

// SAFETY: Bevy's scheduler never runs the same system instance concurrently with itself across
// worker threads; sequential ticks on different threads synchronize-with each other via both the
// scheduler's thread-pool handoff and our `Acquire`/`Release` pair on `in_use`. Any buggy host or
// re-entrant callback attempting overlapping entry observes `in_use == true` via atomic
// `compare_exchange` and returns immediately without touching `UnsafeCell<T>`.
unsafe impl<T: Send> Sync for ReentrantCell<T> {}

impl<T> ReentrantCell<T> {
    /// Wraps `value` in an unlocked [`ReentrantCell`].
    #[inline]
    pub const fn new(value: T) -> Self {
        Self {
            in_use: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Acquires exclusive mutable access to `T` for the duration of `f`, returning `None` if this
    /// cell is already executing on the current or another thread.
    ///
    /// An RAII guard resets `in_use` to `false` with `Ordering::Release` even if `f` panics and
    /// unwinds into [`catch_guest_result`] / [`catch_guest_bool`].
    #[inline]
    pub fn try_with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if self
            .in_use
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let _guard = ReleaseGuard(&self.in_use);
        // SAFETY: `compare_exchange` transitioned `in_use` from `false` to `true` with `Acquire`
        // ordering, granting exclusive access to `self.value` until `_guard` drops (`Release`).
        // Bevy's scheduler invariant ensures a single system instance is never scheduled
        // concurrently with itself, and re-entrant entries fail `compare_exchange` above.
        Some(f(unsafe { &mut *self.value.get() }))
    }

    /// Returns `true` if `try_with_mut` is currently active on this cell.
    #[inline]
    pub fn is_in_use(&self) -> bool {
        self.in_use.load(Ordering::Acquire)
    }
}

const _: () = {
    assert!(MAX_QUERIES_PER_SYSTEM <= 16);
    assert!(!core::mem::needs_drop::<ValidatedIter<'static>>());
};

/// `Send + Sync` newtype wrapping a stable `*const Host` pointer owned by [`GuestInstance`].
///
/// Replaces embedding a 584-byte [`Host`] value inside [`SystemClosureState`],
/// [`RunIfClosureState`], and [`ObserverTrampolineState`] with an 8-byte pointer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PinnedHostPtr(*const Host);

// SAFETY: Points to an immutable, heap-pinned `Host` (`Host: Send + Sync`) owned by
// `GuestInstance` that drops strictly after all closures and observers referencing this pointer.
unsafe impl Send for PinnedHostPtr {}
// SAFETY: Shared reads through `*const Host` are concurrently safe because `Host` is immutable and
// `Send + Sync`.
unsafe impl Sync for PinnedHostPtr {}

impl PinnedHostPtr {
    /// Wraps a stable `*const Host` pointer that outlives the trampoline state holding it.
    #[inline]
    pub(crate) const fn new(ptr: *const Host) -> Self {
        Self(ptr)
    }

    /// Borrows the pinned [`Host`] for `'a`.
    ///
    /// # Safety
    /// The pointed-to [`Host`] must remain alive, aligned, and unmutated for `'a`.
    #[inline]
    pub(crate) const unsafe fn as_ref<'a>(self) -> &'a Host {
        // SAFETY: Caller guarantees the pointed-to `Host` is valid and immutable for `'a`.
        unsafe { &*self.0 }
    }
}

const CTX_UNRESOLVED: u8 = 0;
const CTX_BASE_FALLBACK: u8 = 1;
const CTX_RUNTIME_ONLY: u8 = 2;
const CTX_HOST_MATERIALIZED: u8 = 3;

/// Compact stack-pinned lazy per-tick [`Host`] and [`BamHostCtx`] resolver (`40` bytes).
///
/// Systems that never inspect `Host::host_ctx` pay zero `BamHostApi::sys_host_ctx` FFI calls and
/// zero 624-byte `Host` struct copies. Systems that only need a refreshed [`BamHostCtx`] pointer
/// (`ValidatedSystemRef::intern_symbol`, `ValidatedCommands::insert_resource`, etc.) query
/// `sys_host_ctx(sys_ctx)` at most once via [`Self::resolve_ctx`] without materializing a 624B
/// [`Host`] copy. Only callers requesting `&Host` materialize a single `Host` into the owning
/// `SystemClosureState::host_scratch` slot, preserving strict Tree/Stacked Borrows provenance by
/// always stamping non-null `raw_ctx` without numeric address comparison.
pub(crate) struct LazyInvocationHost<'base> {
    base_host: &'base Host,
    sys_ctx: *mut BamSystemCtx,
    scratch_slot: &'base UnsafeCell<MaybeUninit<Host>>,
    cached_ctx: Cell<*const BamHostCtx>,
    ctx_state: Cell<u8>,
}

const _: () = {
    assert!(core::mem::size_of::<LazyInvocationHost<'static>>() <= 48);
};

impl<'base> LazyInvocationHost<'base> {
    /// Creates an unresolved lazy host frame referencing `base_host`, `sys_ctx`, and the
    /// heap-pinned `scratch_slot` without calling `sys_host_ctx`.
    #[inline]
    pub(crate) const fn new(
        base_host: &'base Host,
        sys_ctx: *mut BamSystemCtx,
        scratch_slot: &'base UnsafeCell<MaybeUninit<Host>>,
    ) -> Self {
        Self {
            base_host,
            sys_ctx,
            scratch_slot,
            cached_ctx: Cell::new(core::ptr::null()),
            ctx_state: Cell::new(CTX_UNRESOLVED),
        }
    }

    /// Returns the init-time base [`Host`] immediately without calling `sys_host_ctx`.
    #[inline]
    pub(crate) const fn base_host(&self) -> &'base Host {
        self.base_host
    }

    /// Resolves and caches the active per-tick [`BamHostCtx`] pointer at most once per invocation
    /// without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub(crate) fn resolve_ctx(&self) -> *const BamHostCtx {
        if self.ctx_state.get() != CTX_UNRESOLVED {
            return self.cached_ctx.get();
        }
        self.resolve_ctx_slow()
    }

    /// Cold path querying `BamHostApi::sys_host_ctx(sys_ctx)` once and recording either
    /// `CTX_RUNTIME_ONLY` (for non-null `raw_ctx`) or `CTX_BASE_FALLBACK`.
    #[cold]
    #[inline(never)]
    fn resolve_ctx_slow(&self) -> *const BamHostCtx {
        let mut ptr = self.base_host.host_ctx();
        let mut next_state = CTX_BASE_FALLBACK;
        if !self.sys_ctx.is_null() {
            if let Some(sys_host_ctx_fn) = host_slot!(self.base_host.api(), sys_host_ctx) {
                // SAFETY: `self.sys_ctx` is non-null and active for the duration of this system call.
                let raw_ctx = unsafe { sys_host_ctx_fn(self.sys_ctx.cast_const()) };
                if !raw_ctx.is_null() {
                    // Always stamp `raw_ctx` rather than comparing addresses with `base_host.host_ctx()`:
                    // under strict provenance / Tree Borrows, `raw_ctx` may carry a fresh borrow tag
                    // at the same numeric address as the init-time `host_ctx`.
                    ptr = raw_ctx;
                    next_state = CTX_RUNTIME_ONLY;
                }
            }
        }
        self.cached_ctx.set(ptr);
        self.ctx_state.set(next_state);
        ptr
    }

    /// Resolves and caches the active per-tick [`Host`] reference at most once per invocation.
    #[inline]
    pub(crate) fn resolve(&self) -> &Host {
        match self.ctx_state.get() {
            CTX_HOST_MATERIALIZED => {
                // SAFETY: `ctx_state == CTX_HOST_MATERIALIZED` proves `self.scratch_slot` was
                // initialized once for this tick by `resolve_slow` and will not be overwritten
                // again until after all `'self` references expire at tick end.
                unsafe { &*self.scratch_slot.get().cast::<Host>() }
            }
            CTX_BASE_FALLBACK => self.base_host,
            _ => self.resolve_slow(),
        }
    }

    /// Cold path performing the one-time `sys_host_ctx` query (if unresolved) and materializing an
    /// updated `Host` into `self.scratch_slot` only when a non-null runtime context was returned.
    #[cold]
    #[inline(never)]
    fn resolve_slow(&self) -> &Host {
        if self.ctx_state.get() == CTX_UNRESOLVED {
            let _ = self.resolve_ctx_slow();
        }
        if self.ctx_state.get() == CTX_BASE_FALLBACK {
            return self.base_host;
        }
        if self.ctx_state.get() == CTX_RUNTIME_ONLY {
            let raw_ctx = self.cached_ctx.get();
            // SAFETY: `raw_ctx` was returned by `BamHostApi::sys_host_ctx(sys_ctx)` and remains
            // valid for the duration of this system invocation.
            let updated = unsafe { self.base_host.with_host_ctx_ptr(raw_ctx) };
            let slot_ptr: *mut Host = self.scratch_slot.get().cast::<Host>();
            // SAFETY: `LazyInvocationHost` is `!Sync`, `SystemClosureState::closure.try_with_mut`
            // holds exclusive access to `scratch_slot` for this tick, `ctx_state` is
            // `CTX_RUNTIME_ONLY` so no shared reference into `scratch_slot` exists yet this tick,
            // and `ptr::write` preserves raw `SharedReadWrite` provenance without forming `&mut`.
            unsafe { slot_ptr.write(updated) };
            self.ctx_state.set(CTX_HOST_MATERIALIZED);
            // SAFETY: `slot_ptr` was just initialized and will not be mutated again this tick.
            return unsafe { &*slot_ptr.cast_const() };
        }
        // SAFETY: Only remaining state is `CTX_HOST_MATERIALIZED`.
        unsafe { &*self.scratch_slot.get().cast::<Host>() }
    }
}

/// Fetches raw `*mut BamIter` for query slot `index` via `BamHostApi::sys_query` out-of-line
/// returning a scalar `Result<*mut BamIter, FfiResult>` (`16` bytes) in registers.
#[inline(never)]
fn fetch_query_slot_raw(
    sys_ctx: *mut BamSystemCtx,
    host: &Host,
    index: usize,
) -> Result<*mut BamIter, FfiResult> {
    if sys_ctx.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    let Some(sys_query_fn) = host.api().sys_query else {
        return Err(FfiResult::NULL_POINTER);
    };
    let mut raw_iter: *mut BamIter = core::ptr::null_mut();
    // SAFETY: `sys_ctx` is active for this tick, `index < total_query_count`, and
    // `raw_iter` is writable stack storage.
    let status = unsafe { sys_query_fn(sys_ctx, index, &mut raw_iter) };
    if status.is_ok() && !raw_iter.is_null() {
        Ok(raw_iter)
    } else if !status.is_ok() {
        Err(status)
    } else {
        Err(FfiResult::NULL_POINTER)
    }
}

/// Fetches query slot `index` via [`fetch_query_slot_raw`] and constructs [`ValidatedIter`]
/// directly in the caller's target slot without a struct-return `_memcpy`.
#[inline(always)]
fn build_query_slot<'a>(
    sys_ctx: *mut BamSystemCtx,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
    scratch: Option<&'a QuerySlotScratch>,
    index: usize,
) -> ValidatedIter<'a> {
    match fetch_query_slot_raw(sys_ctx, host, index) {
        Ok(raw_iter) => {
            // SAFETY: `raw_iter` was returned by `sys_query` for query `index` on active `sys_ctx`,
            // and callers ensure `scratch` is exclusively bound to slot `index` for `'a`.
            unsafe { ValidatedIter::from_raw_with_parts(raw_iter, host, lazy_host, scratch) }
        }
        Err(_) => ValidatedIter::empty(host),
    }
}

/// Fetches the active invocation's [`ValidatedCommands`] buffer via `BamHostApi::sys_commands`
/// without calling `sys_host_ctx` or copying [`Host`], forwarding `lazy_host` for on-demand use.
#[inline]
fn fetch_commands_from_raw<'a>(
    sys_ctx: *mut BamSystemCtx,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
) -> ValidatedCommands<'a> {
    if sys_ctx.is_null() {
        return ValidatedCommands::empty(host);
    }
    let Some(sys_cmds_fn) = host.api().sys_commands else {
        return ValidatedCommands::empty(host);
    };
    let mut raw_cmds: *mut BamCommands = core::ptr::null_mut();
    // SAFETY: `sys_ctx` is active for `'a` and `raw_cmds` is writable stack storage.
    let status = unsafe { sys_cmds_fn(sys_ctx, &mut raw_cmds) };
    if status.is_ok() && !raw_cmds.is_null() {
        // SAFETY: `raw_cmds` was returned by `sys_commands` for `sys_ctx`.
        unsafe { ValidatedCommands::from_raw_with_lazy_host(raw_cmds, host, lazy_host) }
    } else {
        ValidatedCommands::empty(host)
    }
}

/// Internal state backing [`LazyCommands`].
enum LazyCommandsState<'a> {
    Pending {
        sys_ctx: *mut BamSystemCtx,
        host: &'a Host,
        lazy_host: Option<&'a LazyInvocationHost<'a>>,
    },
    Ready(ValidatedCommands<'a>),
}

/// Single-use, non-`Copy`, non-`Clone` lazy handle to the per-system [`BamCommands`] buffer.
///
/// Extracted at most once from [`ValidatedSystemInvocation::take_lazy_commands`] or
/// [`ValidatedSystemSlice::take_lazy_commands`] so systems that never touch commands avoid the
/// `BamHostApi::sys_commands` FFI call while preserving single-owner exclusivity (no duplicate
/// [`ValidatedCommands`] handles can be created for the same tick).
pub struct LazyCommands<'a> {
    state: LazyCommandsState<'a>,
}

impl<'a> LazyCommands<'a> {
    /// Constructs a disabled [`LazyCommands`] that resolves to [`ValidatedCommands::empty`].
    #[inline]
    pub const fn empty(host: &'a Host) -> Self {
        Self {
            state: LazyCommandsState::Ready(ValidatedCommands::empty(host)),
        }
    }

    /// Consumes this handle and materializes the per-system [`ValidatedCommands`] buffer on demand
    /// via `BamHostApi::sys_commands`, keeping `BamHostApi::sys_host_ctx` deferred until a caller
    /// inspects [`ValidatedCommands::host`] or a layout-checked command requires `host_ctx`.
    #[inline]
    pub fn resolve(self) -> ValidatedCommands<'a> {
        match self.state {
            LazyCommandsState::Ready(cmds) => cmds,
            LazyCommandsState::Pending {
                sys_ctx,
                host,
                lazy_host,
            } => fetch_commands_from_raw(sys_ctx, host, lazy_host),
        }
    }
}

/// Validated per-tick system execution frame wrapping `*mut BamSystemCtx`, up to
/// [`MAX_QUERIES_PER_SYSTEM`] disjoint [`ValidatedIter`] slots, and a lazy [`ValidatedCommands`] slot.
pub struct ValidatedSystemInvocation<'a> {
    sys_ctx: *mut BamSystemCtx,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
    scratches: &'a [QuerySlotScratch],
    total_query_count: usize,
    user_query_count: usize,
    resource_query_index: Option<usize>,
    sys_ref: Option<ValidatedSystemRef<'a>>,
    queries: [MaybeUninit<ValidatedIter<'a>>; MAX_QUERIES_PER_SYSTEM],
    query_init_mask: u16,
    query_taken_mask: u16,
    empty_iter: Option<ValidatedIter<'a>>,
    commands: Option<ValidatedCommands<'a>>,
    commands_taken: bool,
    empty_commands: Option<ValidatedCommands<'a>>,
}

impl<'a> ValidatedSystemInvocation<'a> {
    /// Internal constructor shared by [`Self::from_raw`] and [`Self::from_raw_with_parts`].
    #[inline]
    unsafe fn from_parts(
        sys_ctx: *mut BamSystemCtx,
        host: &'a Host,
        lazy_host: Option<&'a LazyInvocationHost<'a>>,
        scratches: &'a [QuerySlotScratch],
        total_query_count: usize,
        user_query_count: usize,
        resource_query_index: Option<usize>,
    ) -> Self {
        let clamped_total = total_query_count.min(MAX_QUERIES_PER_SYSTEM);
        let clamped_user = user_query_count.min(clamped_total);
        Self {
            sys_ctx,
            host,
            lazy_host,
            scratches,
            total_query_count: clamped_total,
            user_query_count: clamped_user,
            resource_query_index: resource_query_index.filter(|&idx| idx < clamped_total),
            sys_ref: Some(ValidatedSystemRef {
                sys_ctx,
                host,
                lazy_host,
            }),
            queries: [const { MaybeUninit::uninit() }; MAX_QUERIES_PER_SYSTEM],
            query_init_mask: 0,
            query_taken_mask: 0,
            empty_iter: None,
            commands: None,
            commands_taken: false,
            empty_commands: None,
        }
    }

    /// Creates a new system execution frame over `sys_ctx` and `host`.
    ///
    /// # Safety
    /// `sys_ctx` must be a non-null, active [`BamSystemCtx`] supplied by the host for the duration
    /// of `'a` with `total_query_count <= MAX_QUERIES_PER_SYSTEM`.
    #[inline]
    pub unsafe fn from_raw(
        sys_ctx: *mut BamSystemCtx,
        host: &'a Host,
        total_query_count: usize,
        user_query_count: usize,
        resource_query_index: Option<usize>,
    ) -> Self {
        // SAFETY: Forwarding caller contract on `sys_ctx` and query counts.
        unsafe {
            Self::from_parts(
                sys_ctx,
                host,
                None,
                &[],
                total_query_count,
                user_query_count,
                resource_query_index,
            )
        }
    }

    /// Creates a new system execution frame backed by a [`LazyInvocationHost`] and per-slot
    /// [`QuerySlotScratch`] slice, deferring `BamHostApi::sys_host_ctx(sys_ctx)` until demanded.
    ///
    /// # Safety
    /// - `sys_ctx` must be a non-null, active [`BamSystemCtx`] matching `lazy_host` for `'a` with
    ///   `total_query_count <= MAX_QUERIES_PER_SYSTEM`.
    /// - `scratches` must be exclusively borrowed for this tick (`try_with_mut` locked).
    #[inline]
    pub(crate) unsafe fn from_raw_with_lazy_host(
        sys_ctx: *mut BamSystemCtx,
        lazy_host: &'a LazyInvocationHost<'a>,
        scratches: &'a [QuerySlotScratch],
        total_query_count: usize,
        user_query_count: usize,
        resource_query_index: Option<usize>,
    ) -> Self {
        // SAFETY: Forwarding caller contract on `sys_ctx`, `scratches`, and query counts.
        unsafe {
            Self::from_parts(
                sys_ctx,
                lazy_host.base_host(),
                Some(lazy_host),
                scratches,
                total_query_count,
                user_query_count,
                resource_query_index,
            )
        }
    }

    /// Returns the raw `*mut BamSystemCtx` pointer.
    #[inline]
    pub const fn as_raw_sys_ctx(&self) -> *mut BamSystemCtx {
        self.sys_ctx
    }

    /// Borrows the unique [`ValidatedSystemRef`] token immutably (`None` if already extracted via
    /// [`Self::take_system_ref`]).
    #[inline]
    pub const fn system_ref(&self) -> Option<&ValidatedSystemRef<'a>> {
        self.sys_ref.as_ref()
    }

    /// Borrows the unique [`ValidatedSystemRef`] token mutably (`None` if already extracted via
    /// [`Self::take_system_ref`]).
    #[inline]
    pub fn system_ref_mut(&mut self) -> Option<&mut ValidatedSystemRef<'a>> {
        self.sys_ref.as_mut()
    }

    /// Extracts the unique non-`Copy` [`ValidatedSystemRef`] token by value (succeeds at most once).
    #[inline]
    pub fn take_system_ref(&mut self) -> Option<ValidatedSystemRef<'a>> {
        self.sys_ref.take()
    }

    /// Returns the init-time base [`Host`] table without invoking `BamHostApi::sys_host_ctx`.
    ///
    /// Safe for immutable per-instance metadata (`api()`, `manifest_name()`) on hot paths.
    #[inline]
    pub const fn base_host(&self) -> &'a Host {
        self.host
    }

    /// Returns the active [`Host`] table for this system invocation, lazily resolving
    /// `BamHostApi::sys_host_ctx(sys_ctx)` at most once per tick when backed by a trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        match self.lazy_host {
            Some(lazy) => lazy.resolve(),
            None => self.host,
        }
    }

    /// Returns the number of entity/explicit queries declared on this system (`0..=MAX_QUERIES_PER_SYSTEM`).
    #[inline]
    pub const fn query_count(&self) -> usize {
        self.user_query_count
    }

    /// Returns the dedicated resource query index if system-level resources were declared.
    #[inline]
    pub const fn resource_query_index(&self) -> Option<usize> {
        self.resource_query_index
    }

    /// Fetches query slot `index` via `BamHostApi::sys_query` without touching `self.queries`.
    #[inline]
    fn fetch_query_slot(&self, index: usize) -> ValidatedIter<'a> {
        build_query_slot(
            self.sys_ctx,
            self.host,
            self.lazy_host,
            self.scratches.get(index),
            index,
        )
    }

    /// Borrows initialized query slot `index` immutably if populated in `self.queries`.
    #[inline]
    fn query_slot_ref(&self, index: usize) -> Option<&ValidatedIter<'a>> {
        if index >= MAX_QUERIES_PER_SYSTEM {
            return None;
        }
        let bit = 1u16 << index;
        if (self.query_init_mask & bit) != 0 {
            // SAFETY: `query_init_mask & bit != 0` proves `self.queries[index]` is initialized.
            Some(unsafe { self.queries[index].assume_init_ref() })
        } else {
            None
        }
    }

    /// Borrows initialized query slot `index` mutably if populated in `self.queries`.
    #[inline]
    fn query_slot_mut(&mut self, index: usize) -> Option<&mut ValidatedIter<'a>> {
        if index >= MAX_QUERIES_PER_SYSTEM {
            return None;
        }
        let bit = 1u16 << index;
        if (self.query_init_mask & bit) != 0 {
            // SAFETY: `query_init_mask & bit != 0` proves `self.queries[index]` is initialized.
            Some(unsafe { self.queries[index].assume_init_mut() })
        } else {
            None
        }
    }

    /// Extracts validated query slot `index` (`0..total_query_count`) by value (succeeds at most once
    /// per slot).
    #[inline]
    pub fn take_raw_query_slot(&mut self, index: usize) -> Option<ValidatedIter<'a>> {
        if index >= self.total_query_count {
            return None;
        }
        let bit = 1u16 << index;
        if (self.query_taken_mask & bit) != 0 {
            return None;
        }
        self.query_taken_mask |= bit;
        if (self.query_init_mask & bit) != 0 {
            self.query_init_mask &= !bit;
            // SAFETY: `query_init_mask & bit != 0` proved `self.queries[index]` was initialized,
            // and clearing `bit` ensures it is never read again without re-initialization.
            Some(unsafe { self.queries[index].assume_init_read() })
        } else {
            Some(self.fetch_query_slot(index))
        }
    }

    /// Extracts a single-use [`LazyCommands`] token deferring `BamHostApi::sys_commands` until
    /// [`LazyCommands::resolve`] is invoked.
    ///
    /// Returns a disabled [`LazyCommands`] if commands were already extracted via
    /// [`Self::take_commands`] or [`Self::take_lazy_commands`].
    #[inline]
    pub fn take_lazy_commands(&mut self) -> LazyCommands<'a> {
        if self.commands_taken {
            return LazyCommands::empty(self.host);
        }
        self.commands_taken = true;
        if let Some(cmds) = self.commands.take() {
            LazyCommands {
                state: LazyCommandsState::Ready(cmds),
            }
        } else {
            LazyCommands {
                state: LazyCommandsState::Pending {
                    sys_ctx: self.sys_ctx,
                    host: self.host,
                    lazy_host: self.lazy_host,
                },
            }
        }
    }

    /// Fetches a [`ValidatedCommands`] buffer for this system invocation by value (returns an empty
    /// disabled buffer if already extracted).
    #[inline]
    pub fn take_commands(&mut self) -> ValidatedCommands<'a> {
        if self.commands_taken {
            return ValidatedCommands::empty(self.host);
        }
        self.ensure_commands_slot();
        self.commands_taken = true;
        self.commands
            .take()
            .unwrap_or_else(|| ValidatedCommands::empty(self.host))
    }

    /// Lazily fetches and caches the [`ValidatedIter`] for query slot `index` via
    /// `BamHostApi::sys_query` if not already fetched or extracted.
    #[inline]
    fn ensure_query_slot(&mut self, index: usize) {
        if index >= self.total_query_count {
            return;
        }
        let bit = 1u16 << index;
        if ((self.query_taken_mask | self.query_init_mask) & bit) != 0 {
            return;
        }
        let iter = self.fetch_query_slot(index);
        self.queries[index].write(iter);
        self.query_init_mask |= bit;
    }

    /// Fetches the active invocation's [`ValidatedCommands`] buffer via `BamHostApi::sys_commands`,
    /// returning an empty disabled buffer if unavailable without calling `sys_host_ctx`.
    #[inline]
    fn fetch_commands_slot(&self) -> ValidatedCommands<'a> {
        fetch_commands_from_raw(self.sys_ctx, self.host, self.lazy_host)
    }

    /// Populates `self.commands` on first access if not already fetched or taken.
    #[inline]
    fn ensure_commands_slot(&mut self) {
        if !self.commands_taken && self.commands.is_none() {
            self.commands = Some(self.fetch_commands_slot());
        }
    }

    /// Borrows primary query `0` mutably (yielding an empty exhausted iterator if no entity query
    /// was declared).
    #[inline]
    pub fn iter_mut(&mut self) -> &mut ValidatedIter<'a> {
        if self.user_query_count == 0 {
            let host = self.host;
            return self
                .empty_iter
                .get_or_insert_with(|| ValidatedIter::empty(host));
        }
        self.ensure_query_slot(0);
        if (self.query_init_mask & 1u16) != 0 {
            // SAFETY: `query_init_mask & 1 != 0` proves `self.queries[0]` is initialized.
            unsafe { self.queries[0].assume_init_mut() }
        } else {
            let host = self.host;
            self.empty_iter
                .get_or_insert_with(|| ValidatedIter::empty(host))
        }
    }

    /// Borrows declared query `index` (`0..query_count()`) mutably, returning `None` if out of range.
    #[inline]
    pub fn query_mut(&mut self, index: usize) -> Option<&mut ValidatedIter<'a>> {
        if index >= self.user_query_count {
            return None;
        }
        self.ensure_query_slot(index);
        self.query_slot_mut(index)
    }

    /// Simultaneously borrows two distinct queries `idx_a` and `idx_b` (`None` if `idx_a == idx_b`
    /// or either index is out of range).
    pub fn queries_2_mut(
        &mut self,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(&mut ValidatedIter<'a>, &mut ValidatedIter<'a>)> {
        if idx_a == idx_b || idx_a >= self.user_query_count || idx_b >= self.user_query_count {
            return None;
        }
        self.ensure_query_slot(idx_a);
        self.ensure_query_slot(idx_b);
        let mask = self.query_init_mask;
        if (mask & (1u16 << idx_a)) == 0 || (mask & (1u16 << idx_b)) == 0 {
            return None;
        }
        if idx_a < idx_b {
            let (left, right) = self.queries.split_at_mut(idx_b);
            // SAFETY: Both `idx_a` and `idx_b` are initialized per `query_init_mask` and disjoint.
            Some(unsafe { (left[idx_a].assume_init_mut(), right[0].assume_init_mut()) })
        } else {
            let (left, right) = self.queries.split_at_mut(idx_a);
            // SAFETY: Both `idx_a` and `idx_b` are initialized per `query_init_mask` and disjoint.
            Some(unsafe { (right[0].assume_init_mut(), left[idx_b].assume_init_mut()) })
        }
    }

    /// Borrows the per-system [`ValidatedCommands`] buffer mutably.
    #[inline]
    pub fn commands_mut(&mut self) -> &mut ValidatedCommands<'a> {
        if self.commands_taken {
            let host = self.host;
            return self
                .empty_commands
                .get_or_insert_with(|| ValidatedCommands::empty(host));
        }
        let fetched = if self.commands.is_none() {
            Some(self.fetch_commands_slot())
        } else {
            None
        };
        if let Some(cmds) = fetched {
            self.commands.insert(cmds)
        } else {
            let host = self.host;
            let empty_cmds = &mut self.empty_commands;
            self.commands
                .as_mut()
                .unwrap_or_else(|| empty_cmds.get_or_insert_with(|| ValidatedCommands::empty(host)))
        }
    }

    /// Simultaneously borrows primary query `0` and the deferred [`ValidatedCommands`] buffer.
    #[inline]
    pub fn iter_and_commands_mut(
        &mut self,
    ) -> (&mut ValidatedIter<'a>, &mut ValidatedCommands<'a>) {
        if self.user_query_count > 0 {
            self.ensure_query_slot(0);
        }
        self.ensure_commands_slot();
        let host = self.host;
        let empty_cmds = &mut self.empty_commands;
        let cmds = if self.commands_taken {
            empty_cmds.get_or_insert_with(|| ValidatedCommands::empty(host))
        } else {
            self.commands
                .as_mut()
                .unwrap_or_else(|| empty_cmds.get_or_insert_with(|| ValidatedCommands::empty(host)))
        };
        let empty_iter = &mut self.empty_iter;
        let iter = if self.user_query_count == 0 || (self.query_init_mask & 1u16) == 0 {
            empty_iter.get_or_insert_with(|| ValidatedIter::empty(host))
        } else {
            // SAFETY: `query_init_mask & 1 != 0` proves `self.queries[0]` is initialized.
            unsafe { self.queries[0].assume_init_mut() }
        };
        (iter, cmds)
    }

    /// Simultaneously borrows two distinct queries `idx_a` and `idx_b` alongside [`ValidatedCommands`].
    pub fn queries_2_and_commands_mut(
        &mut self,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(
        &mut ValidatedIter<'a>,
        &mut ValidatedIter<'a>,
        &mut ValidatedCommands<'a>,
    )> {
        if idx_a == idx_b || idx_a >= self.user_query_count || idx_b >= self.user_query_count {
            return None;
        }
        self.ensure_query_slot(idx_a);
        self.ensure_query_slot(idx_b);
        self.ensure_commands_slot();
        let mask = self.query_init_mask;
        if (mask & (1u16 << idx_a)) == 0 || (mask & (1u16 << idx_b)) == 0 {
            return None;
        }
        let cmds = self.commands.as_mut()?;
        if idx_a < idx_b {
            let (left, right) = self.queries.split_at_mut(idx_b);
            // SAFETY: Both `idx_a` and `idx_b` are initialized per `query_init_mask` and disjoint.
            Some(unsafe {
                (
                    left[idx_a].assume_init_mut(),
                    right[0].assume_init_mut(),
                    cmds,
                )
            })
        } else {
            let (left, right) = self.queries.split_at_mut(idx_a);
            // SAFETY: Both `idx_a` and `idx_b` are initialized per `query_init_mask` and disjoint.
            Some(unsafe {
                (
                    right[0].assume_init_mut(),
                    left[idx_b].assume_init_mut(),
                    cmds,
                )
            })
        }
    }

    /// Borrows singleton resource `R` at declaration index `decl_index` (routing to the dedicated
    /// system-level resource query if declared, or query `0` otherwise).
    #[inline]
    pub fn resource_checked<R: AbiPod>(
        &mut self,
        decl_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&R> {
        let q_idx = self.resource_query_index.unwrap_or(0);
        if q_idx >= self.total_query_count {
            return None;
        }
        self.ensure_query_slot(q_idx);
        self.query_slot_ref(q_idx)?
            .resource_checked::<R>(decl_index, expected_uuid)
    }

    /// Mutably borrows singleton resource `R` at declaration index `decl_index` (routing to the
    /// dedicated system-level resource query if declared, or query `0` otherwise).
    #[inline]
    pub fn resource_mut_checked<R: AbiPod>(
        &mut self,
        decl_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&mut R> {
        let q_idx = self.resource_query_index.unwrap_or(0);
        if q_idx >= self.total_query_count {
            return None;
        }
        self.ensure_query_slot(q_idx);
        self.query_slot_mut(q_idx)?
            .resource_mut_checked::<R>(decl_index, expected_uuid)
    }
}

/// Zero-array per-tick system execution view passed to bucketed `system_closure_slice_trampoline`
/// closures, avoiding duplicate `[MaybeUninit<ValidatedIter>; 16]` stack storage when callers
/// (`SystemContext`) already own an exact-bucket `[Option<ChunkIter>; N]` slice.
pub struct ValidatedSystemSlice<'a> {
    sys_ctx: *mut BamSystemCtx,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
    scratches: &'a [QuerySlotScratch],
    total_query_count: usize,
    user_query_count: usize,
    resource_query_index: Option<usize>,
    sys_ref: Option<ValidatedSystemRef<'a>>,
    query_taken_mask: u16,
    commands_taken: bool,
}

const _: () = {
    assert!(core::mem::size_of::<ValidatedSystemSlice<'static>>() <= 128);
};

impl<'a> ValidatedSystemSlice<'a> {
    /// Creates a new zero-array system execution view backed by [`LazyInvocationHost`] and
    /// per-slot [`QuerySlotScratch`] storage.
    ///
    /// # Safety
    /// - `sys_ctx` must be a non-null, active [`BamSystemCtx`] matching `lazy_host` for `'a` with
    ///   `total_query_count <= MAX_QUERIES_PER_SYSTEM`.
    /// - `scratches` must be exclusively borrowed for this tick (`try_with_mut` locked).
    #[inline(always)]
    pub(crate) unsafe fn from_raw_with_lazy_host(
        sys_ctx: *mut BamSystemCtx,
        lazy_host: &'a LazyInvocationHost<'a>,
        scratches: &'a [QuerySlotScratch],
        total_query_count: usize,
        user_query_count: usize,
        resource_query_index: Option<usize>,
    ) -> Self {
        let clamped_total = total_query_count.min(MAX_QUERIES_PER_SYSTEM);
        let clamped_user = user_query_count.min(clamped_total);
        let base_host = lazy_host.base_host();
        Self {
            sys_ctx,
            host: base_host,
            lazy_host: Some(lazy_host),
            scratches,
            total_query_count: clamped_total,
            user_query_count: clamped_user,
            resource_query_index: resource_query_index.filter(|&idx| idx < clamped_total),
            sys_ref: Some(ValidatedSystemRef {
                sys_ctx,
                host: base_host,
                lazy_host: Some(lazy_host),
            }),
            query_taken_mask: 0,
            commands_taken: false,
        }
    }

    /// Creates a standalone [`ValidatedSystemSlice`] without a [`LazyInvocationHost`] or scratch
    /// slice (used for Kani proofs and unit tests).
    ///
    /// # Safety
    /// `sys_ctx` must be a non-null, active [`BamSystemCtx`] supplied by the host for `'a`.
    #[cfg(kani)]
    #[inline]
    pub(crate) unsafe fn from_raw_for_kani(
        sys_ctx: *mut BamSystemCtx,
        host: &'a Host,
        total_query_count: usize,
        user_query_count: usize,
        resource_query_index: Option<usize>,
    ) -> Self {
        let clamped_total = total_query_count.min(MAX_QUERIES_PER_SYSTEM);
        let clamped_user = user_query_count.min(clamped_total);
        Self {
            sys_ctx,
            host,
            lazy_host: None,
            scratches: &[],
            total_query_count: clamped_total,
            user_query_count: clamped_user,
            resource_query_index: resource_query_index.filter(|&idx| idx < clamped_total),
            sys_ref: Some(ValidatedSystemRef {
                sys_ctx,
                host,
                lazy_host: None,
            }),
            query_taken_mask: 0,
            commands_taken: false,
        }
    }

    /// Returns the raw `*mut BamSystemCtx` pointer.
    #[inline]
    pub const fn as_raw_sys_ctx(&self) -> *mut BamSystemCtx {
        self.sys_ctx
    }

    /// Borrows the unique [`ValidatedSystemRef`] token immutably (`None` if already extracted via
    /// [`Self::take_system_ref`]).
    #[inline]
    pub const fn system_ref(&self) -> Option<&ValidatedSystemRef<'a>> {
        self.sys_ref.as_ref()
    }

    /// Borrows the unique [`ValidatedSystemRef`] token mutably (`None` if already extracted via
    /// [`Self::take_system_ref`]).
    #[inline]
    pub fn system_ref_mut(&mut self) -> Option<&mut ValidatedSystemRef<'a>> {
        self.sys_ref.as_mut()
    }

    /// Extracts the unique non-`Copy` [`ValidatedSystemRef`] token by value (succeeds at most once).
    #[inline(always)]
    pub fn take_system_ref(&mut self) -> Option<ValidatedSystemRef<'a>> {
        self.sys_ref.take()
    }

    /// Returns the init-time base [`Host`] table without invoking `BamHostApi::sys_host_ctx`.
    #[inline(always)]
    pub const fn base_host(&self) -> &'a Host {
        self.host
    }

    /// Returns the active [`Host`] table for this system invocation, lazily resolving
    /// `BamHostApi::sys_host_ctx(sys_ctx)` at most once per tick when backed by a trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        match self.lazy_host {
            Some(lazy) => lazy.resolve(),
            None => self.host,
        }
    }

    /// Returns the number of entity/explicit queries declared on this system (`0..=MAX_QUERIES_PER_SYSTEM`).
    #[inline(always)]
    pub const fn query_count(&self) -> usize {
        self.user_query_count
    }

    /// Returns the dedicated resource query index if system-level resources were declared.
    #[inline(always)]
    pub const fn resource_query_index(&self) -> Option<usize> {
        self.resource_query_index
    }

    /// Extracts validated query slot `index` (`0..total_query_count`) by value (succeeds at most once
    /// per slot).
    #[inline(always)]
    pub fn take_raw_query_slot(&mut self, index: usize) -> Option<ValidatedIter<'a>> {
        if index >= self.total_query_count {
            return None;
        }
        let bit = 1u16 << index;
        if (self.query_taken_mask & bit) != 0 {
            return None;
        }
        self.query_taken_mask |= bit;
        Some(build_query_slot(
            self.sys_ctx,
            self.host,
            self.lazy_host,
            self.scratches.get(index),
            index,
        ))
    }

    /// Extracts a single-use [`LazyCommands`] token deferring `BamHostApi::sys_commands` until
    /// [`LazyCommands::resolve`] is invoked.
    #[inline(always)]
    pub fn take_lazy_commands(&mut self) -> LazyCommands<'a> {
        if self.commands_taken {
            return LazyCommands::empty(self.host);
        }
        self.commands_taken = true;
        LazyCommands {
            state: LazyCommandsState::Pending {
                sys_ctx: self.sys_ctx,
                host: self.host,
                lazy_host: self.lazy_host,
            },
        }
    }

    /// Fetches a [`ValidatedCommands`] buffer for this system invocation by value (returns an empty
    /// disabled buffer if already extracted).
    #[inline]
    pub fn take_commands(&mut self) -> ValidatedCommands<'a> {
        self.take_lazy_commands().resolve()
    }
}

/// Unique (non-`Copy`, non-`Clone`) validated per-tick `*mut BamSystemCtx` token supporting
/// borrow-checked random per-entity component lookups and handle-based singleton resource access.
///
/// Uniqueness guarantees that `&mut self` methods such as [`Self::res_mut_ptr_by_handle_checked`]
/// and [`Self::write_mapped_wire_resource_by_handle`] cannot produce aliasing `&mut` references to
/// the same host resource.
pub struct ValidatedSystemRef<'a> {
    sys_ctx: *mut BamSystemCtx,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
}

impl<'a> ValidatedSystemRef<'a> {
    /// Constructs a disabled [`ValidatedSystemRef`] (`sys_ctx = null`) whose lookups return
    /// `None` / `Err(FfiResult::NULL_POINTER)` / `Err(FfiResult::INVALID_COMPONENT)`.
    #[inline]
    pub const fn empty(host: &'a Host) -> Self {
        Self {
            sys_ctx: core::ptr::null_mut(),
            host,
            lazy_host: None,
        }
    }

    /// Returns the raw `*mut BamSystemCtx` pointer.
    #[inline]
    pub const fn as_raw_sys_ctx(&self) -> *mut BamSystemCtx {
        self.sys_ctx
    }

    /// Returns the init-time base [`Host`] table without querying `BamHostApi::sys_host_ctx`.
    ///
    /// Safe for immutable per-instance metadata (`api()`, `manifest_name()`) on hot paths.
    #[inline]
    pub const fn base_host(&self) -> &'a Host {
        self.host
    }

    /// Resolves and caches the active per-tick [`BamHostCtx`] pointer at most once per invocation
    /// without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub(crate) fn resolve_ctx(&self) -> *const BamHostCtx {
        match self.lazy_host {
            Some(lazy) => lazy.resolve_ctx(),
            None => self.host.host_ctx(),
        }
    }

    /// Returns the active [`Host`] table for this system invocation, lazily resolving
    /// `BamHostApi::sys_host_ctx(sys_ctx)` at most once per tick when backed by a trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        match self.lazy_host {
            Some(lazy) => lazy.resolve(),
            None => self.host,
        }
    }

    /// Reads the most recent human-readable UTF-8 diagnostic recorded on this world's host context
    /// without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn last_error(&self) -> String {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe { Host::last_error_with_ctx(self.host.api(), self.resolve_ctx()) }
    }

    /// Emits a structured guest diagnostic message via `BamHostApi::log_diagnostic` (slot 50)
    /// without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn log_diagnostic(&self, level: u32, message: &str) -> FfiResult {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe {
            Host::log_diagnostic_with_ctx(self.host.api(), self.resolve_ctx(), level, message)
        }
    }

    /// Records a guest-side fault message on this world's host context via
    /// `BamHostApi::report_guest_fault` (slot 51) without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn report_guest_fault(&self, message: &str) -> FfiResult {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe { Host::report_guest_fault_with_ctx(self.host.api(), self.resolve_ctx(), message) }
    }

    /// Resolves a registered component or resource strictly by its 128-bit [`ComponentUuid`] via
    /// `BamHostApi::resolve_component_by_uuid` (slot 52) without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn resolve_component_by_uuid(
        &self,
        uuid: ComponentUuid,
    ) -> Result<BamComponent, FfiResult> {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe {
            Host::resolve_component_by_uuid_with_ctx(self.host.api(), self.resolve_ctx(), uuid)
        }
    }

    /// Queries ABI `0.3.0` [`BamLayoutInfo`] metadata for `component` via
    /// `BamHostApi::component_layout_info` (slot 53) without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn component_layout_info(
        &self,
        component: BamComponent,
    ) -> Result<BamLayoutInfo, FfiResult> {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe {
            Host::component_layout_info_with_ctx(self.host.api(), self.resolve_ctx(), component)
        }
    }

    /// Interns a non-empty UTF-8 symbol string to an `O(1)` comparable [`BamSymbol`] handle via
    /// `BamHostApi::intern_symbol` (slot 55) without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn intern_symbol(&self, symbol: &str) -> Result<BamSymbol, FfiResult> {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe { Host::intern_symbol_with_ctx(self.host.api(), self.resolve_ctx(), symbol) }
    }

    /// Binds a canonical UTF-8 lookup `name` to `entity` in the host symbol registry via
    /// `BamHostApi::bind_named_object` (slot 56) without materializing a 624-byte [`Host`] copy.
    #[inline]
    pub fn bind_named_object(&self, name: &str, entity: FfiEntity) -> FfiResult {
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        unsafe {
            Host::bind_named_object_with_ctx(self.host.api(), self.resolve_ctx(), name, entity)
        }
    }

    /// Validates `iter.column_info(term_index)` via `check_info` and invokes
    /// `BamHostApi::sys_entity_get` to retrieve a shared component pointer for `entity`.
    #[inline]
    fn raw_entity_ptr_ro(
        &self,
        iter: &ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
        check_info: impl FnOnce(&bamabi_sys::BamColumnInfo) -> bool,
    ) -> Option<*const u8> {
        if self.sys_ctx.is_null() {
            return None;
        }
        let get_fn = host_slot!(self.host.api(), sys_entity_get)?;
        let info = iter.column_info(term_index).ok()?;
        if !info.mode.has_entity_column_data() || info.is_resource() || info.is_message() {
            return None;
        }
        if !check_info(&info) {
            return None;
        }
        // SAFETY: `self.sys_ctx` is an active `BamSystemCtx` for `'a`, and `&self` + `&iter`
        // prevent concurrent mutable access through this system context.
        let ptr = unsafe { get_fn(self.sys_ctx.cast_const(), query_index, entity, term_index) };
        (!ptr.is_null()).then_some(ptr)
    }

    /// Validates writable `iter.column_info(term_index)` via `check_info` and invokes
    /// `BamHostApi::sys_entity_get_mut` to retrieve an exclusive component pointer for `entity`.
    #[inline]
    // `&mut ValidatedIter` is an intentional borrow-guard preventing aliasing chunk borrows while
    // `sys_entity_get_mut` holds a mutable pointer into `iter`'s query storage.
    #[allow(clippy::needless_pass_by_ref_mut)]
    fn raw_entity_ptr_mut(
        &mut self,
        iter: &mut ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
        check_info: impl FnOnce(&bamabi_sys::BamColumnInfo) -> bool,
    ) -> Option<*mut u8> {
        if self.sys_ctx.is_null() {
            return None;
        }
        let get_mut_fn = host_slot!(self.host.api(), sys_entity_get_mut)?;
        let info = iter.column_info(term_index).ok()?;
        if !info.mode.is_mutable()
            || !info.mode.has_entity_column_data()
            || info.is_immutable()
            || info.is_resource()
            || info.is_message()
        {
            return None;
        }
        if !check_info(&info) {
            return None;
        }
        // SAFETY: `self.sys_ctx` is an active `BamSystemCtx` for `'a`, and `&mut self` +
        // `&mut iter` prevent any aliasing borrow on this system context.
        let ptr = unsafe { get_mut_fn(self.sys_ctx, query_index, entity, term_index) };
        (!ptr.is_null()).then_some(ptr)
    }

    /// Converts a verified non-null read-only entity/resource pointer into `&'b T`, checking
    /// alignment and handling zero-sized `T` via `NonNull::dangling()`.
    #[inline]
    unsafe fn cast_entity_ref<'b, T>(ptr: *const u8) -> Option<&'b T> {
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T` may be referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<T>::dangling().as_ref() });
        }
        if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: Caller verified non-null `ptr`, layout/UUID/VTable, and shared borrow lifetime `'b`.
        Some(unsafe { &*ptr.cast::<T>() })
    }

    /// Converts a verified non-null writable entity/resource pointer into `&'b mut T`, checking
    /// alignment and handling zero-sized `T` via `NonNull::dangling()`.
    #[inline]
    unsafe fn cast_entity_mut<'b, T>(ptr: *mut u8) -> Option<&'b mut T> {
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T` may be mutably referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<T>::dangling().as_mut() });
        }
        if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: Caller verified non-null `ptr`, layout/UUID/VTable, and exclusive borrow `'b`.
        Some(unsafe { &mut *ptr.cast::<T>() })
    }

    /// Looks up a shared `&'b T` component reference for `entity` using declared access on
    /// `(query_index, term_index)`.
    pub fn entity_get_checked<'b, T: AbiPod>(
        &'b self,
        iter: &'b ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&'b T> {
        let ptr = self.raw_entity_ptr_ro(iter, query_index, term_index, entity, |info| {
            info.size == core::mem::size_of::<T>()
                && info.align == core::mem::align_of::<T>()
                && expected_uuid.is_none_or(|f| f.matches(info.uuid))
        })?;
        // SAFETY: Host verified query membership and declared read/write access; `info` confirmed
        // `T`'s layout and optional UUID, and `&'b self` + `&'b iter` hold a shared borrow for `'b`.
        unsafe { Self::cast_entity_ref::<T>(ptr) }
    }

    /// Looks up an exclusive `&'b mut T` component reference for `entity` using declared writable
    /// access on `(query_index, term_index)`, marking the component changed.
    pub fn entity_get_mut_checked<'b, T: AbiPod>(
        &'b mut self,
        iter: &'b mut ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&'b mut T> {
        let ptr = self.raw_entity_ptr_mut(iter, query_index, term_index, entity, |info| {
            info.size == core::mem::size_of::<T>()
                && info.align == core::mem::align_of::<T>()
                && expected_uuid.is_none_or(|f| f.matches(info.uuid))
        })?;
        // SAFETY: Host verified query membership, writable `FilteredAccessSet` declaration, and
        // marked change ticks; `&'b mut self` + `&'b mut iter` guarantee exclusive access for `'b`.
        unsafe { Self::cast_entity_mut::<T>(ptr) }
    }

    /// Looks up `N` simultaneous disjoint exclusive `&'b mut T` component references for
    /// `entities` using declared writable access on `(query_index, term_index)`, with zero heap
    /// allocation.
    ///
    /// Rejects null entities with [`FfiResult::INVALID_ENTITY`], duplicate entities or hostile
    /// overlapping host pointers with [`FfiResult::ACCESS_CONFLICT`], missing entities with
    /// [`FfiResult::NOT_FOUND`], misaligned pointers or layout/UUID mismatches with
    /// [`FfiResult::TYPE_MISMATCH`], non-writable/immutable columns with
    /// [`FfiResult::PERMISSION_DENIED`], and missing `sys_entity_get_mut` slot with
    /// [`FfiResult::UNSUPPORTED`].
    // `&mut ValidatedIter` is an intentional borrow-guard preventing aliasing chunk borrows while
    // `sys_entity_get_mut` holds mutable pointers into `iter`'s query storage.
    #[allow(clippy::needless_pass_by_ref_mut)]
    pub fn entity_get_many_mut_checked<'b, const N: usize, T: AbiPod>(
        &'b mut self,
        iter: &'b mut ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entities: [FfiEntity; N],
        expected_uuid: Option<UuidFilter>,
    ) -> Result<[&'b mut T; N], FfiResult> {
        if self.sys_ctx.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let get_mut_fn =
            host_slot!(self.host.api(), sys_entity_get_mut).ok_or(FfiResult::UNSUPPORTED)?;
        for i in 0..N {
            if entities[i].is_null() {
                return Err(FfiResult::INVALID_ENTITY);
            }
            for j in (i + 1)..N {
                if entities[i] == entities[j] {
                    return Err(FfiResult::ACCESS_CONFLICT);
                }
            }
        }
        let info = iter.column_info(term_index)?;
        if !info.mode.is_mutable()
            || !info.mode.has_entity_column_data()
            || info.is_immutable()
            || info.is_resource()
            || info.is_message()
        {
            return Err(FfiResult::PERMISSION_DENIED);
        }
        let elem_size = core::mem::size_of::<T>();
        if info.size != elem_size
            || info.align != core::mem::align_of::<T>()
            || !expected_uuid.is_none_or(|f| f.matches(info.uuid))
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }

        // Fetch and validate all raw pointers before creating any `&mut T` references so hostile
        // overlapping pointers never trigger aliasing UB under Stacked/Tree Borrows.
        let mut raw_ptrs: [*mut u8; N] = [core::ptr::null_mut(); N];
        for i in 0..N {
            // SAFETY: `self.sys_ctx` is an active `BamSystemCtx` for `'a`, and `&mut self` +
            // `&mut iter` prevent any aliasing borrow on this system context.
            let ptr = unsafe { get_mut_fn(self.sys_ctx, query_index, entities[i], term_index) };
            if ptr.is_null() {
                return Err(FfiResult::NOT_FOUND);
            }
            if elem_size > 0 {
                if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
                    return Err(FfiResult::TYPE_MISMATCH);
                }
                let start = ptr.addr();
                let end = start
                    .checked_add(elem_size)
                    .ok_or(FfiResult::OUT_OF_RANGE)?;
                for prev_ptr in &raw_ptrs[..i] {
                    let prev_start = prev_ptr.addr();
                    let prev_end = prev_start + elem_size;
                    if start < prev_end && prev_start < end {
                        return Err(FfiResult::ACCESS_CONFLICT);
                    }
                }
            }
            raw_ptrs[i] = ptr;
        }

        let mut out: [MaybeUninit<&'b mut T>; N] = [const { MaybeUninit::uninit() }; N];
        for i in 0..N {
            let reference: &'b mut T = if elem_size == 0 {
                // SAFETY: Zero-sized `T: AbiPod` may be mutably referenced via `NonNull::dangling()`.
                unsafe { core::ptr::NonNull::<T>::dangling().as_mut() }
            } else {
                // SAFETY: All `raw_ptrs[0..N]` are verified non-null, aligned for `T`, pairwise
                // byte-range disjoint, and exclusively accessible for `'b` under `&'b mut self` +
                // `&'b mut iter`.
                unsafe { &mut *raw_ptrs[i].cast::<T>() }
            };
            out[i].write(reference);
        }
        // SAFETY: The loop above initialized all `N` elements of `out`.
        Ok(out.map(|slot| unsafe { slot.assume_init() }))
    }

    /// Looks up a shared singleton resource `&R` by its pre-resolved `BamComponent` handle,
    /// validating `(size, align)` and optional `expected_uuid` via `Host::component_layout_info`
    /// (failing closed on mismatch or `UNSUPPORTED`).
    pub fn res_ptr_by_handle_with_uuid_checked<R: AbiPod>(
        &self,
        component: BamComponent,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<(&R, BamChangeTicks)> {
        if self.sys_ctx.is_null() || !component.is_valid() {
            return None;
        }
        let info = self.component_layout_info(component).ok()?;
        if info.size != core::mem::size_of::<R>() || info.align != core::mem::align_of::<R>() {
            return None;
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return None;
        }
        let get_fn = host_slot!(self.host.api(), sys_res_ptr)?;
        let mut ticks = BamChangeTicks::default();
        // SAFETY: `self.sys_ctx` is an active `BamSystemCtx` and `&mut ticks` is aligned writable storage.
        let ptr = unsafe { get_fn(self.sys_ctx.cast_const(), component, &mut ticks) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: `component_layout_info` verified `R`'s exact size, alignment, and optional UUID;
        // host verified resource borrow permissions in `FilteredAccessSet`.
        let val = unsafe { Self::cast_entity_ref::<R>(ptr) }?;
        Some((val, ticks))
    }

    /// Looks up a shared singleton resource `&R` by its pre-resolved `BamComponent` handle,
    /// validating `(size, align)` via `Host::component_layout_info` (failing closed on mismatch or
    /// `UNSUPPORTED`).
    #[inline]
    pub fn res_ptr_by_handle_checked<R: AbiPod>(
        &self,
        component: BamComponent,
    ) -> Option<(&R, BamChangeTicks)> {
        self.res_ptr_by_handle_with_uuid_checked::<R>(component, None)
    }

    /// Looks up an exclusive mutable singleton resource `&mut R` by its pre-resolved
    /// `BamComponent` handle, validating `(size, align, !immutable)` and optional `expected_uuid`
    /// via `Host::component_layout_info` (failing closed on mismatch or `UNSUPPORTED`).
    pub fn res_mut_ptr_by_handle_with_uuid_checked<R: AbiPod>(
        &mut self,
        component: BamComponent,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<(&mut R, BamChangeTicks)> {
        if self.sys_ctx.is_null() || !component.is_valid() {
            return None;
        }
        let info = self.component_layout_info(component).ok()?;
        if (info.flags & BAM_STORAGE_FLAG_IMMUTABLE) != 0
            || info.size != core::mem::size_of::<R>()
            || info.align != core::mem::align_of::<R>()
        {
            return None;
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return None;
        }
        let get_mut_fn = host_slot!(self.host.api(), sys_res_mut_ptr)?;
        let mut ticks = BamChangeTicks::default();
        // SAFETY: `self.sys_ctx` is active, unique `&mut self` prevents aliasing, and `&mut ticks` is aligned.
        let ptr = unsafe { get_mut_fn(self.sys_ctx, component, &mut ticks) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: `component_layout_info` verified `R`'s exact size, alignment, mutability, and
        // optional UUID; host verified exclusive `ResourceWrite` permission and stamped change ticks.
        let val = unsafe { Self::cast_entity_mut::<R>(ptr) }?;
        Some((val, ticks))
    }

    /// Looks up an exclusive mutable singleton resource `&mut R` by its pre-resolved
    /// `BamComponent` handle, validating `(size, align, !immutable)` via
    /// `Host::component_layout_info` (failing closed on mismatch or `UNSUPPORTED`).
    #[inline]
    pub fn res_mut_ptr_by_handle_checked<R: AbiPod>(
        &mut self,
        component: BamComponent,
    ) -> Option<(&mut R, BamChangeTicks)> {
        self.res_mut_ptr_by_handle_with_uuid_checked::<R>(component, None)
    }

    /// Borrows the contiguous zero-copy `&[FfiEntity]` slice of child/target entities for `entity`
    /// under `relationship_target` (`BamComponent::INVALID` defaults to Bevy's built-in `Children`)
    /// via `BamHostApi::sys_entity_children`.
    pub fn entity_children(
        &self,
        entity: FfiEntity,
        relationship_target: BamComponent,
    ) -> Result<&[FfiEntity], FfiResult> {
        if self.sys_ctx.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if entity.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        let children_fn =
            host_slot!(self.host.api(), sys_entity_children).ok_or(FfiResult::UNSUPPORTED)?;
        let mut ptr: *const FfiEntity = core::ptr::null();
        let mut len: usize = 0;
        // SAFETY: `self.sys_ctx` is an active `BamSystemCtx` and `ptr`/`len` are aligned stack slots.
        let status = unsafe {
            children_fn(
                self.sys_ctx.cast_const(),
                entity,
                relationship_target,
                &mut ptr,
                &mut len,
            )
        };
        if !status.is_ok() {
            return Err(status);
        }
        if len == 0 {
            return Ok(&[]);
        }
        if ptr.is_null() || !ptr.is_aligned() {
            return Err(FfiResult::NULL_POINTER);
        }
        let byte_len = len
            .checked_mul(core::mem::size_of::<FfiEntity>())
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        // SAFETY: Host verified declared read access to `relationship_target`; `ptr` is non-null,
        // aligned for `FfiEntity`, and valid for shared reads throughout `&self`.
        Ok(unsafe { core::slice::from_raw_parts(ptr, len) })
    }

    /// Verifies via `Host::component_layout_info` that `component` matches `vtable`'s `(host_size,
    /// host_align, uuid)` and mutability before invoking a mapped wire VTable getter/setter.
    fn validate_wire_resource_layout<W: AbiPod>(
        &self,
        component: BamComponent,
        vtable: &ValidatedWireVTable<W>,
        require_mutable: bool,
    ) -> Result<(), FfiResult> {
        let info = self.component_layout_info(component)?;
        if info.size != vtable.host_size()
            || info.align != vtable.host_align()
            || (vtable.host_size() > 0
                && (!vtable.host_align().is_power_of_two()
                    || !vtable.host_size().is_multiple_of(vtable.host_align())))
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if !vtable.uuid().is_zero() && info.uuid != vtable.uuid() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if require_mutable && (info.flags & BAM_STORAGE_FLAG_IMMUTABLE) != 0 {
            return Err(FfiResult::PERMISSION_DENIED);
        }
        Ok(())
    }

    /// Reads an opaque host-bridged singleton resource by `component` handle using `vtable`
    /// (`BamHostApi::sys_res_ptr`), validating `component` against `vtable.uuid()`,
    /// `vtable.host_size()`, and `vtable.host_align()` via `Host::component_layout_info`.
    pub fn read_mapped_wire_resource_by_handle<W: AbiPod>(
        &self,
        component: BamComponent,
        vtable: &ValidatedWireVTable<W>,
    ) -> Result<(W, BamChangeTicks), FfiResult> {
        if self.sys_ctx.is_null() || !component.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        self.validate_wire_resource_layout(component, vtable, false)?;
        let get_ptr_fn = host_slot!(self.host.api(), sys_res_ptr).ok_or(FfiResult::UNSUPPORTED)?;
        let Some(vt_get) = vtable.raw.get else {
            return Err(FfiResult::UNSUPPORTED);
        };
        let mut ticks = BamChangeTicks::default();
        // SAFETY: `self.sys_ctx` is active and `&mut ticks` is aligned writable storage.
        let ptr = unsafe { get_ptr_fn(self.sys_ctx.cast_const(), component, &mut ticks) };
        if ptr.is_null() {
            return Err(FfiResult::NOT_FOUND);
        }
        if vtable.host_size() > 0 && !(ptr as usize).is_multiple_of(vtable.host_align()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let mut out = core::mem::MaybeUninit::<W>::zeroed();
        // SAFETY: Host verified resource read access on `component`, `validate_wire_resource_layout`
        // confirmed `vtable`'s exact UUID and host layout, and `vt_get` initializes `out`.
        let status = unsafe { vt_get(ptr.cast::<c_void>(), out.as_mut_ptr()) };
        if status.is_ok() {
            // SAFETY: `vt_get` reported `OK`, confirming `out` is initialized `W`.
            Ok((unsafe { out.assume_init() }, ticks))
        } else {
            Err(status)
        }
    }

    /// Writes `value` (`W: AbiPod`) into an opaque host-bridged singleton resource by `component`
    /// handle using `vtable` (`BamHostApi::sys_res_mut_ptr`), validating `component` against
    /// `vtable.uuid()`, `vtable.host_size()`, and `vtable.host_align()` via
    /// `Host::component_layout_info`.
    pub fn write_mapped_wire_resource_by_handle<W: AbiPod>(
        &mut self,
        component: BamComponent,
        vtable: &ValidatedWireVTable<W>,
        value: &W,
    ) -> Result<BamChangeTicks, FfiResult> {
        if self.sys_ctx.is_null() || !component.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        self.validate_wire_resource_layout(component, vtable, true)?;
        let get_mut_fn =
            host_slot!(self.host.api(), sys_res_mut_ptr).ok_or(FfiResult::UNSUPPORTED)?;
        let Some(vt_set) = vtable.raw.set else {
            return Err(FfiResult::PERMISSION_DENIED);
        };
        let mut ticks = BamChangeTicks::default();
        // SAFETY: `self.sys_ctx` is active under unique `&mut self` and `&mut ticks` is writable storage.
        let ptr = unsafe { get_mut_fn(self.sys_ctx, component, &mut ticks) };
        if ptr.is_null() {
            return Err(FfiResult::NOT_FOUND);
        }
        if vtable.host_size() > 0 && !(ptr as usize).is_multiple_of(vtable.host_align()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        // SAFETY: Host verified exclusive `ResourceWrite` permission, `validate_wire_resource_layout`
        // confirmed `vtable`'s exact UUID and host layout, and `value` is aligned readable `&W`.
        let status = unsafe { vt_set(ptr.cast::<c_void>(), value) };
        if status.is_ok() {
            Ok(ticks)
        } else {
            Err(status)
        }
    }
}

/// Heap-pinned per-system scratch slot storing a lazily materialized [`Host`] copy when
/// `BamHostApi::sys_host_ctx` returns a non-null runtime context pointer.
pub(crate) struct InvocationHostScratch(pub(crate) UnsafeCell<MaybeUninit<Host>>);

impl InvocationHostScratch {
    /// Creates an uninitialized per-system host scratch slot.
    #[inline]
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
    }
}

// SAFETY: `InvocationHostScratch` is only accessed by `LazyInvocationHost` while
// `SystemClosureState::closure.try_with_mut` holds its `Acquire`/`Release` lock, ensuring
// exclusive single-thread, non-reentrant access for the duration of one tick.
unsafe impl Sync for InvocationHostScratch {}

/// Heap-pinned per-query-slot scratch storage owned by [`SystemClosureState`].
pub(crate) struct SlotScratchesCell(UnsafeCell<Box<[QuerySlotScratch]>>);

impl SlotScratchesCell {
    /// Creates an empty zero-allocation scratch slice (used for 0-query systems and Kani proofs).
    #[inline]
    pub(crate) fn empty() -> Self {
        Self(UnsafeCell::new(Box::default()))
    }

    /// Allocates `count` [`QuerySlotScratch`] entries (`0` heap allocations when `count == 0`).
    #[inline]
    fn with_len(count: usize) -> Self {
        if count == 0 {
            Self::empty()
        } else {
            let vec: Vec<QuerySlotScratch> = (0..count).map(|_| QuerySlotScratch::new()).collect();
            Self(UnsafeCell::new(vec.into_boxed_slice()))
        }
    }

    /// Borrows the underlying scratch slice immutably while `try_with_mut` holds exclusive access.
    ///
    /// # Safety
    /// Caller must hold `SystemClosureState::closure.try_with_mut` exclusive access for `'a`.
    #[inline]
    unsafe fn borrow_locked(&self) -> &[QuerySlotScratch] {
        // SAFETY: `try_with_mut` guarantees exclusive non-reentrant access for the active tick,
        // and no mutable reference into the `Box<[QuerySlotScratch]>` exists.
        unsafe { &*self.0.get() }
    }
}

// SAFETY: `QuerySlotScratch` is `Send`, and `SlotScratchesCell` is only accessed via
// `borrow_locked` while `SystemClosureState::closure.try_with_mut` holds exclusive access.
unsafe impl Sync for SlotScratchesCell {}

/// Heap-pinned system callback state owned by [`GuestInstance`], pairing a pointer to the
/// instance's pinned [`Host`], query slot metadata, heap-pinned scratch slots, and a
/// [`ReentrantCell`]-guarded guest closure.
pub(crate) struct SystemClosureState<F> {
    pub(crate) host: PinnedHostPtr,
    pub(crate) total_query_count: usize,
    pub(crate) user_query_count: usize,
    pub(crate) resource_query_index: Option<usize>,
    pub(crate) host_scratch: InvocationHostScratch,
    pub(crate) slot_scratches: SlotScratchesCell,
    pub(crate) closure: ReentrantCell<F>,
}

/// Heap-pinned `run_if` predicate state owned by [`GuestInstance`], pairing a pointer to the
/// instance's pinned [`Host`] with a [`ReentrantCell`]-guarded guest predicate closure.
pub(crate) struct RunIfClosureState<R> {
    pub(crate) host: PinnedHostPtr,
    pub(crate) closure: ReentrantCell<R>,
}

/// Panic-safe, re-entrancy-guarded `extern "C"` [`BamSystemFn`] trampoline.
///
/// Borrows the instance's heap-pinned [`Host`] directly without copying it or calling
/// `BamHostApi::sys_host_ctx(sys_ctx)` eagerly; per-tick `BamHostCtx` refresh is performed lazily
/// via [`LazyInvocationHost`] only when the system actually accesses `Host`. Returns
/// [`FfiResult::ACCESS_CONFLICT`] on re-entry or [`FfiResult::PANIC_CAUGHT`] on panic.
pub(crate) unsafe extern "C" fn system_closure_trampoline<F>(
    sys_ctx: *mut BamSystemCtx,
    user_data: *mut c_void,
) -> FfiResult
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
{
    catch_guest_result(|| {
        if sys_ctx.is_null() || user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<SystemClosureState<F>>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and allocated via
        // `GuestInstance::track_allocation` for `SystemClosureState<F>`.
        let state = unsafe { &*user_data.cast::<SystemClosureState<F>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` which remains
        // alive and immutable until after `state` is dropped on `bamabi_shutdown`.
        let base_host = unsafe { state.host.as_ref() };
        let Some(result) = state.closure.try_with_mut(|func| {
            let lazy_host = LazyInvocationHost::new(base_host, sys_ctx, &state.host_scratch.0);
            // SAFETY: `state.closure.try_with_mut` holds exclusive non-reentrant access for this tick.
            let scratches = unsafe { state.slot_scratches.borrow_locked() };
            let mut idx = 0usize;
            while idx < MAX_QUERIES_PER_SYSTEM {
                let Some(scratch) = scratches.get(idx) else {
                    break;
                };
                scratch.reset_per_tick();
                idx += 1;
            }
            // SAFETY: `sys_ctx` is non-null and active for the duration of this `BamSystemFn` call.
            let mut invocation = unsafe {
                ValidatedSystemInvocation::from_raw_with_lazy_host(
                    sys_ctx,
                    &lazy_host,
                    scratches,
                    state.total_query_count,
                    state.user_query_count,
                    state.resource_query_index,
                )
            };
            func(&mut invocation)
        }) else {
            return FfiResult::ACCESS_CONFLICT;
        };
        result
    })
}

/// Const-bucketed panic-safe, re-entrancy-guarded `extern "C"` [`BamSystemFn`] trampoline passing
/// a zero-array [`ValidatedSystemSlice`] (`N ∈ {0, 1, 4, 16}`).
///
/// When `N == 0`, query counts and scratch resetting fold to compile-time constants (`0`),
/// eliminating all `sys_query` / `fetch_query_slot_raw` calls and stack query arrays.
pub(crate) unsafe extern "C" fn system_closure_slice_trampoline<const N: usize, F>(
    sys_ctx: *mut BamSystemCtx,
    user_data: *mut c_void,
) -> FfiResult
where
    F: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
{
    catch_guest_result(|| {
        if sys_ctx.is_null() || user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<SystemClosureState<F>>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and allocated via
        // `GuestInstance::track_allocation` for `SystemClosureState<F>`.
        let state = unsafe { &*user_data.cast::<SystemClosureState<F>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` which remains
        // alive and immutable until after `state` is dropped on `bamabi_shutdown`.
        let base_host = unsafe { state.host.as_ref() };
        let Some(result) = state.closure.try_with_mut(|func| {
            let lazy_host = LazyInvocationHost::new(base_host, sys_ctx, &state.host_scratch.0);
            let (clamped_total, clamped_user, clamped_res, scratches): (
                usize,
                usize,
                Option<usize>,
                &[QuerySlotScratch],
            ) = if N == 0 {
                (0, 0, None, &[])
            } else {
                // SAFETY: `state.closure.try_with_mut` holds exclusive access for this tick.
                let scratch_slice = unsafe { state.slot_scratches.borrow_locked() };
                let mut idx = 0usize;
                while idx < N {
                    let Some(scratch) = scratch_slice.get(idx) else {
                        break;
                    };
                    scratch.reset_per_tick();
                    idx += 1;
                }
                let total = state.total_query_count.min(N).min(MAX_QUERIES_PER_SYSTEM);
                let user = state.user_query_count.min(total);
                let res = state.resource_query_index.filter(|&idx| idx < total);
                (total, user, res, scratch_slice)
            };
            // SAFETY: `sys_ctx` is non-null and active for the duration of this `BamSystemFn` call.
            let mut slice = unsafe {
                ValidatedSystemSlice::from_raw_with_lazy_host(
                    sys_ctx,
                    &lazy_host,
                    scratches,
                    clamped_total,
                    clamped_user,
                    clamped_res,
                )
            };
            func(&mut slice)
        }) else {
            return FfiResult::ACCESS_CONFLICT;
        };
        result
    })
}

/// Panic-safe, re-entrancy-guarded `extern "C"` [`BamRunIfFn`] trampoline returning
/// [`FfiBool::FALSE`] on null `user_data`, re-entrant entry, or guest panic.
pub(crate) unsafe extern "C" fn run_if_closure_trampoline<R>(user_data: *mut c_void) -> FfiBool
where
    R: FnMut(&Host) -> bool + Send + 'static,
{
    catch_guest_bool(|| {
        if user_data.is_null()
            || !(user_data as usize).is_multiple_of(core::mem::align_of::<RunIfClosureState<R>>())
        {
            return FfiBool::FALSE;
        }
        // SAFETY: `user_data` is non-null, aligned, and allocated via
        // `GuestInstance::track_allocation` for `RunIfClosureState<R>`.
        let state = unsafe { &*user_data.cast::<RunIfClosureState<R>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` outliving `state`.
        let host = unsafe { state.host.as_ref() };
        match state
            .closure
            .try_with_mut(|pred| FfiBool::from_bool(pred(host)))
        {
            Some(flag) => flag,
            None => FfiBool::FALSE,
        }
    })
}

/// Allocates a standalone `(BamSystemFn, *mut c_void)` closure pair inside `instance` (used by
/// [`register_system_closure`] and direct re-entrancy / panic unit tests).
pub fn allocate_system_trampoline<F>(
    instance: &mut GuestInstance,
    total_query_count: usize,
    user_query_count: usize,
    resource_query_index: Option<usize>,
    closure: F,
) -> (BamSystemFn, *mut c_void)
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
{
    let host = PinnedHostPtr::new(instance.pinned_host_ptr());
    let scratch_len = total_query_count.min(MAX_QUERIES_PER_SYSTEM);
    let raw = instance.track_allocation(Box::new(SystemClosureState {
        host,
        total_query_count,
        user_query_count,
        resource_query_index,
        host_scratch: InvocationHostScratch::new(),
        slot_scratches: SlotScratchesCell::with_len(scratch_len),
        closure: ReentrantCell::new(closure),
    }));
    (system_closure_trampoline::<F>, raw.cast::<c_void>())
}

/// Allocates a const-bucketed `(BamSystemFn, *mut c_void)` closure pair inside `instance`
/// targeting [`ValidatedSystemSlice`], allocating `0` query scratch slots when `N == 0`.
pub fn allocate_system_slice_trampoline<const N: usize, F>(
    instance: &mut GuestInstance,
    total_query_count: usize,
    user_query_count: usize,
    resource_query_index: Option<usize>,
    closure: F,
) -> (BamSystemFn, *mut c_void)
where
    F: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
{
    let host = PinnedHostPtr::new(instance.pinned_host_ptr());
    let scratch_len = if N == 0 {
        0
    } else {
        total_query_count.min(N).min(MAX_QUERIES_PER_SYSTEM)
    };
    let raw = instance.track_allocation(Box::new(SystemClosureState {
        host,
        total_query_count,
        user_query_count,
        resource_query_index,
        host_scratch: InvocationHostScratch::new(),
        slot_scratches: SlotScratchesCell::with_len(scratch_len),
        closure: ReentrantCell::new(closure),
    }));
    (
        system_closure_slice_trampoline::<N, F>,
        raw.cast::<c_void>(),
    )
}

/// Validated `(BamRunIfFn, *mut c_void)` pair allocated inside a [`GuestInstance`].
pub struct PreparedRunIf {
    pub(crate) func: BamRunIfFn,
    pub(crate) user_data: *mut c_void,
}

impl PreparedRunIf {
    /// Allocates a panic-safe, re-entrancy-guarded `run_if` predicate inside `instance`.
    #[inline]
    pub fn new<R>(instance: &mut GuestInstance, predicate: R) -> Self
    where
        R: FnMut(&Host) -> bool + Send + 'static,
    {
        let (func, user_data) = allocate_run_if_trampoline(instance, predicate);
        Self { func, user_data }
    }

    /// Wraps a caller-managed raw `(BamRunIfFn, user_data)` pair.
    ///
    /// # Safety
    /// `func(user_data)` must be valid to invoke for the lifetime of the target `World` without
    /// unwinding across `extern "C"`.
    #[inline]
    pub const unsafe fn from_raw(func: BamRunIfFn, user_data: *mut c_void) -> Self {
        Self { func, user_data }
    }

    /// Returns the underlying `(BamRunIfFn, *mut c_void)` tuple.
    #[inline]
    pub const fn into_raw(self) -> (BamRunIfFn, *mut c_void) {
        (self.func, self.user_data)
    }
}

/// Allocates a standalone `(BamRunIfFn, *mut c_void)` closure pair inside `instance`.
pub fn allocate_run_if_trampoline<R>(
    instance: &mut GuestInstance,
    predicate: R,
) -> (BamRunIfFn, *mut c_void)
where
    R: FnMut(&Host) -> bool + Send + 'static,
{
    let host = PinnedHostPtr::new(instance.pinned_host_ptr());
    let raw = instance.track_allocation(Box::new(RunIfClosureState {
        host,
        closure: ReentrantCell::new(predicate),
    }));
    (run_if_closure_trampoline::<R>, raw.cast::<c_void>())
}

/// Allocates a panic-safe, re-entrancy-guarded ECS-aware `run_if_ctx` `(BamSystemFn, *mut c_void)`
/// closure pair inside `instance`.
///
/// The wrapped predicate `func` receives a [`ValidatedSystemInvocation`] with `query_count`
/// query slots and returns `true` to run the system (`FfiResult::OK`) or `false` to skip it
/// (`FfiResult::NOT_FOUND`, non-zero per [`BamSystemDesc::run_if_ctx`]). Re-entrant invocation
/// returns `FfiResult::ACCESS_CONFLICT` (skip) and panics are caught as `FfiResult::PANIC_CAUGHT`
/// (skip).
pub fn allocate_run_if_ctx_trampoline<F>(
    instance: &mut GuestInstance,
    query_count: usize,
    mut func: F,
) -> (BamSystemFn, *mut c_void)
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> bool + Send + 'static,
{
    allocate_system_trampoline(
        instance,
        query_count,
        query_count,
        None,
        move |inv: &mut ValidatedSystemInvocation<'_>| {
            if func(inv) {
                FfiResult::OK
            } else {
                FfiResult::NOT_FOUND
            }
        },
    )
}

/// Heap-pinned storage pairing an [`OwnedRawTermSlice`] with its [`BamQueryDesc`] for
/// [`BamSystemDesc::run_if_query`].
struct OwnedRunIfQuery {
    _terms: OwnedRawTermSlice,
    desc: BamQueryDesc,
}

// SAFETY: `desc.terms` points exclusively into `_terms` (owned via `Box::into_raw` and never
// mutated or retagged on move) and lives until `OwnedRunIfQuery` drops on `bamabi_shutdown`.
unsafe impl Send for OwnedRunIfQuery {}

/// Validates `terms` and pins a heap-owned [`BamQueryDesc`] inside `instance`, returning a stable
/// `*const BamQueryDesc` suitable for [`SystemRegistrationConfig::run_if_query`].
pub fn allocate_run_if_ctx_query(
    instance: &mut GuestInstance,
    terms: &[BamTermDesc],
) -> Result<*const BamQueryDesc, FfiResult> {
    if terms.len() > MAX_TERMS_PER_QUERY {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }
    for term in terms {
        let status = term.validate();
        if !status.is_ok() {
            return Err(status);
        }
    }
    let raw_terms = OwnedRawTermSlice::from_slice(terms);
    let desc = if raw_terms.len == 0 {
        BamQueryDesc::empty()
    } else {
        BamQueryDesc::new(raw_terms.as_ptr(), raw_terms.len)
    };
    let raw = instance.track_allocation(Box::new(OwnedRunIfQuery {
        _terms: raw_terms,
        desc,
    }));
    // SAFETY: `raw` is pinned inside `instance` until `bamabi_shutdown` (or rollback) and never
    // mutated after allocation.
    Ok(unsafe { core::ptr::addr_of!((*raw).desc) })
}

/// Descriptor assembly configuration passed from safe `SystemBuilder` to [`register_system_closure`].
pub struct SystemRegistrationConfig<'a> {
    /// NUL-terminated UTF-8 system name.
    pub name: &'a CStr,
    /// User-declared entity/resource query clauses (`<= MAX_QUERIES_PER_SYSTEM`).
    pub user_queries: &'a [&'a [BamTermDesc]],
    /// Optional per-query `ParamSet` group IDs (`0` = independent query; non-zero = mutually
    /// exclusive `ParamSet` group) parallel to `user_queries`.
    pub user_query_param_set_groups: &'a [u32],
    /// Optional system-level singleton resource terms (`resource_read` / `resource_write` on `SystemBuilder`).
    pub system_resources: &'a [BamTermDesc],
    /// Target Bevy schedule.
    pub schedule: ScheduleId,
    /// `BAM_SYSTEM_FLAG_*` bitmask.
    pub flags: u32,
    /// Named system sets that this system must execute after.
    pub after: &'a [&'a CStr],
    /// Named system sets that this system must execute before.
    pub before: &'a [&'a CStr],
    /// Named system sets (`"host::<set>"` or custom sets) that this system belongs to.
    pub in_sets: &'a [&'a CStr],
    /// Named system sets that this system executes weakly after (`after_ignore_deferred`).
    pub after_weak: &'a [&'a CStr],
    /// Named system sets that this system executes weakly before (`before_ignore_deferred`).
    pub before_weak: &'a [&'a CStr],
    /// Named system sets whose access ambiguities with this system are explicitly suppressed.
    pub ambiguous_with: &'a [&'a CStr],
    /// Optional ECS-aware run-condition callback receiving a [`BamSystemCtx`] backed by
    /// `run_if_query` (`FfiResult::OK` = run; non-zero = skip).
    pub run_if_ctx: Option<BamSystemFn>,
    /// Optional read-only [`BamQueryDesc`] declaring the ECS resources/components accessible to
    /// `run_if_ctx`.
    pub run_if_query: *const BamQueryDesc,
    /// Opaque guest-owned state passed to `run_if_ctx`.
    pub run_if_ctx_user_data: *mut c_void,
}

impl Default for SystemRegistrationConfig<'_> {
    #[inline]
    fn default() -> Self {
        Self {
            name: c"",
            user_queries: &[],
            user_query_param_set_groups: &[],
            system_resources: &[],
            schedule: ScheduleId::UPDATE,
            flags: 0,
            after: &[],
            before: &[],
            in_sets: &[],
            after_weak: &[],
            before_weak: &[],
            ambiguous_with: &[],
            run_if_ctx: None,
            run_if_query: core::ptr::null(),
            run_if_ctx_user_data: core::ptr::null_mut(),
        }
    }
}

/// Raw-pointer owner for a heap-allocated NUL-terminated C string that avoids `Box` retagging
/// when [`OwnedDescriptorStorage`] is moved into [`GuestInstance::track_allocation`].
struct OwnedRawCString(*mut c_char);

// SAFETY: Uniquely owns an immutable heap NUL-terminated string created via `CString::into_raw`
// (avoiding `Box` `Unique` retagging on move) and freed once in `Drop`.
unsafe impl Send for OwnedRawCString {}

impl OwnedRawCString {
    /// Clones `s` into an owned raw C string allocation.
    fn from_cstr(s: &CStr) -> Self {
        Self(CString::from(s).into_raw())
    }

    /// Returns the stable `*const c_char` pointer for descriptor fields.
    #[inline]
    const fn as_ptr(&self) -> *const c_char {
        self.0.cast_const()
    }
}

impl Drop for OwnedRawCString {
    /// Reclaims the owned `CString` allocation via `CString::from_raw`.
    fn drop(&mut self) {
        // SAFETY: `self.0` was created via `CString::into_raw` in `from_cstr` and is freed once.
        drop(unsafe { CString::from_raw(self.0) });
    }
}

/// Raw-pointer owner for a heap-allocated `[BamTermDesc]` slice that preserves pointer provenance
/// across moves under Miri Stacked/Tree Borrows.
struct OwnedRawTermSlice {
    ptr: *mut BamTermDesc,
    len: usize,
}

// SAFETY: Uniquely owns an immutable heap `[BamTermDesc]` slice created via `Box::into_raw`
// (avoiding `Box` `Unique` retagging on move) and freed once in `Drop` when `len > 0`.
unsafe impl Send for OwnedRawTermSlice {}

impl OwnedRawTermSlice {
    /// Copies `terms` into a pinned raw heap slice (or a dangling zero-length handle if empty).
    fn from_slice(terms: &[BamTermDesc]) -> Self {
        let len = terms.len();
        if len == 0 {
            Self {
                ptr: core::ptr::NonNull::<BamTermDesc>::dangling().as_ptr(),
                len: 0,
            }
        } else {
            let boxed: Box<[BamTermDesc]> = terms.into();
            Self {
                ptr: Box::into_raw(boxed).cast::<BamTermDesc>(),
                len,
            }
        }
    }

    /// Returns the stable `*const BamTermDesc` pointer for [`BamQueryDesc::terms`].
    #[inline]
    const fn as_ptr(&self) -> *const BamTermDesc {
        self.ptr.cast_const()
    }
}

impl Drop for OwnedRawTermSlice {
    /// Frees the raw `[BamTermDesc]` slice when `self.len > 0`.
    fn drop(&mut self) {
        if self.len > 0 {
            // SAFETY: `self.ptr` came from `Box::into_raw` of a `Box<[BamTermDesc]>` of length `self.len`.
            drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(self.ptr, self.len)) });
        }
    }
}

/// Consolidated owner for all heap strings, query descriptors, and term slices referenced by a
/// registered [`BamSystemDesc`], tracked as a single allocation inside [`GuestInstance`].
struct OwnedDescriptorStorage {
    name: OwnedRawCString,
    _term_vecs: Vec<OwnedRawTermSlice>,
    queries: Box<[BamQueryDesc]>,
    _after_names: Vec<OwnedRawCString>,
    after_ptrs: Box<[*const c_char]>,
    _before_names: Vec<OwnedRawCString>,
    before_ptrs: Box<[*const c_char]>,
    _in_set_names: Vec<OwnedRawCString>,
    in_set_ptrs: Box<[*const c_char]>,
    _after_weak_names: Vec<OwnedRawCString>,
    after_weak_ptrs: Box<[*const c_char]>,
    _before_weak_names: Vec<OwnedRawCString>,
    before_weak_ptrs: Box<[*const c_char]>,
    _ambiguous_with_names: Vec<OwnedRawCString>,
    ambiguous_with_ptrs: Box<[*const c_char]>,
}

// SAFETY: Every raw pointer inside `queries` and `*_ptrs` points exclusively into heap-allocated
// `OwnedRawTermSlice` and `OwnedRawCString` buffers owned by the same struct and never mutated
// after construction (nor retagged on move).
unsafe impl Send for OwnedDescriptorStorage {}

/// Pinned descriptor pointers and query layout counts extracted by non-generic
/// [`pin_system_descriptor_storage`].
struct PinnedSystemDescParts {
    total_query_count: usize,
    user_query_count: usize,
    resource_query_index: Option<usize>,
    name_ptr: *const c_char,
    queries_ptr: *const BamQueryDesc,
    after_ptr: *const *const c_char,
    before_ptr: *const *const c_char,
    in_sets_ptr: *const *const c_char,
    after_weak_ptr: *const *const c_char,
    before_weak_ptr: *const *const c_char,
    ambiguous_with_ptr: *const *const c_char,
}

/// Validates `config` and pins all owned descriptor strings and query term slices inside
/// `instance` (non-generic to avoid monomorphizing descriptor setup across `N` or `F`).
fn pin_system_descriptor_storage(
    instance: &mut GuestInstance,
    config: &SystemRegistrationConfig<'_>,
) -> Result<PinnedSystemDescParts, FfiResult> {
    if config.name.to_bytes().is_empty() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    if config.name.to_str().is_err() {
        return Err(FfiResult::INVALID_UTF8);
    }
    if !config.schedule.is_valid() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    if (config.flags & !BAM_SYSTEM_KNOWN_FLAGS) != 0 {
        return Err(FfiResult::UNSUPPORTED);
    }

    let has_sys_resources = !config.system_resources.is_empty();
    let total_query_count = config.user_queries.len() + usize::from(has_sys_resources);
    if total_query_count > MAX_QUERIES_PER_SYSTEM {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }
    if config.system_resources.len() > MAX_TERMS_PER_QUERY {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }

    for terms in config.user_queries {
        if terms.len() > MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        for term in *terms {
            let status = term.validate();
            if !status.is_ok() {
                return Err(status);
            }
        }
    }
    for term in config.system_resources {
        let status = term.validate();
        if !status.is_ok() {
            return Err(status);
        }
    }
    for set_name in config
        .after
        .iter()
        .chain(config.before.iter())
        .chain(config.in_sets.iter())
        .chain(config.after_weak.iter())
        .chain(config.before_weak.iter())
        .chain(config.ambiguous_with.iter())
    {
        if set_name.to_bytes().is_empty() {
            return Err(FfiResult::INVALID_ARGUMENT);
        }
        if set_name.to_str().is_err() {
            return Err(FfiResult::INVALID_UTF8);
        }
    }

    let mut term_vecs: Vec<OwnedRawTermSlice> = Vec::with_capacity(total_query_count);
    let mut query_descs: Vec<BamQueryDesc> = Vec::with_capacity(total_query_count);
    for (q_idx, terms) in config.user_queries.iter().enumerate() {
        let group = config
            .user_query_param_set_groups
            .get(q_idx)
            .copied()
            .unwrap_or(0);
        let desc = if terms.is_empty() {
            BamQueryDesc::empty()
        } else {
            let raw_terms = OwnedRawTermSlice::from_slice(terms);
            let ptr = raw_terms.as_ptr();
            let len = raw_terms.len;
            term_vecs.push(raw_terms);
            BamQueryDesc::new(ptr, len)
        };
        query_descs.push(desc.with_param_set_group(group));
    }

    let resource_query_index = if has_sys_resources {
        let idx = query_descs.len();
        let raw_res = OwnedRawTermSlice::from_slice(config.system_resources);
        let ptr = raw_res.as_ptr();
        let len = raw_res.len;
        term_vecs.push(raw_res);
        query_descs.push(BamQueryDesc::new(ptr, len));
        Some(idx)
    } else {
        None
    };

    /// Clones a slice of `&CStr` set names into owned raw C strings plus a parallel pointer array.
    fn collect_cstrings(names: &[&CStr]) -> (Vec<OwnedRawCString>, Box<[*const c_char]>) {
        let mut owned: Vec<OwnedRawCString> = Vec::with_capacity(names.len());
        let mut ptrs: Vec<*const c_char> = Vec::with_capacity(names.len());
        for set_name in names {
            let cstr = OwnedRawCString::from_cstr(set_name);
            ptrs.push(cstr.as_ptr());
            owned.push(cstr);
        }
        (owned, ptrs.into_boxed_slice())
    }

    let (after_names, after_ptrs) = collect_cstrings(config.after);
    let (before_names, before_ptrs) = collect_cstrings(config.before);
    let (in_set_names, in_set_ptrs) = collect_cstrings(config.in_sets);
    let (after_weak_names, after_weak_ptrs) = collect_cstrings(config.after_weak);
    let (before_weak_names, before_weak_ptrs) = collect_cstrings(config.before_weak);
    let (ambiguous_with_names, ambiguous_with_ptrs) = collect_cstrings(config.ambiguous_with);

    let storage = OwnedDescriptorStorage {
        name: OwnedRawCString::from_cstr(config.name),
        _term_vecs: term_vecs,
        queries: query_descs.into_boxed_slice(),
        _after_names: after_names,
        after_ptrs,
        _before_names: before_names,
        before_ptrs,
        _in_set_names: in_set_names,
        in_set_ptrs,
        _after_weak_names: after_weak_names,
        after_weak_ptrs,
        _before_weak_names: before_weak_names,
        before_weak_ptrs,
        _ambiguous_with_names: ambiguous_with_names,
        ambiguous_with_ptrs,
    };
    let raw_storage = instance.track_allocation(Box::new(storage));
    // SAFETY: `raw_storage` was just allocated by `instance.track_allocation` and remains pinned
    // on the heap until `instance` drops.
    let storage_ref = unsafe { &*raw_storage };
    let slice_ptr_or_null = |s: &[*const c_char]| {
        if s.is_empty() {
            core::ptr::null()
        } else {
            s.as_ptr()
        }
    };
    Ok(PinnedSystemDescParts {
        total_query_count,
        user_query_count: config.user_queries.len(),
        resource_query_index,
        name_ptr: storage_ref.name.as_ptr(),
        queries_ptr: if storage_ref.queries.is_empty() {
            core::ptr::null()
        } else {
            storage_ref.queries.as_ptr()
        },
        after_ptr: slice_ptr_or_null(&storage_ref.after_ptrs),
        before_ptr: slice_ptr_or_null(&storage_ref.before_ptrs),
        in_sets_ptr: slice_ptr_or_null(&storage_ref.in_set_ptrs),
        after_weak_ptr: slice_ptr_or_null(&storage_ref.after_weak_ptrs),
        before_weak_ptr: slice_ptr_or_null(&storage_ref.before_weak_ptrs),
        ambiguous_with_ptr: slice_ptr_or_null(&storage_ref.ambiguous_with_ptrs),
    })
}

/// Assembles and validates a [`BamSystemDesc`] from `parts` and an allocated `(callback_fn, user_data)` pair.
fn assemble_and_validate_system_desc(
    parts: PinnedSystemDescParts,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    callback_fn: BamSystemFn,
    user_data: *mut c_void,
) -> Result<BamSystemDesc, FfiResult> {
    let mut desc = BamSystemDesc::new(
        parts.name_ptr,
        parts.queries_ptr,
        parts.total_query_count,
        callback_fn,
        user_data,
    )
    .with_flags(config.flags)
    .with_schedule(config.schedule)
    .with_after(parts.after_ptr, config.after.len())
    .with_before(parts.before_ptr, config.before.len())
    .with_in_sets(parts.in_sets_ptr, config.in_sets.len())
    .with_after_weak(parts.after_weak_ptr, config.after_weak.len())
    .with_before_weak(parts.before_weak_ptr, config.before_weak.len())
    .with_ambiguous_with(parts.ambiguous_with_ptr, config.ambiguous_with.len());

    if let Some((run_if_fn, run_if_data)) = run_if {
        desc = desc.with_run_if(run_if_fn, run_if_data);
    }

    if let Some(run_if_ctx_fn) = config.run_if_ctx {
        desc = desc.with_run_if_ctx(
            run_if_ctx_fn,
            config.run_if_query,
            config.run_if_ctx_user_data,
        );
    }

    let validation = desc.validate();
    if !validation.is_ok() {
        return Err(validation);
    }
    Ok(desc)
}

/// Shared descriptor preparation helper factoring validation, storage pinning, and descriptor
/// assembly across both [`ValidatedSystemInvocation`] and [`ValidatedSystemSlice`] trampolines.
#[inline]
fn prepare_system_desc_with(
    instance: &mut GuestInstance,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    allocate_cb: impl FnOnce(
        &mut GuestInstance,
        usize,
        usize,
        Option<usize>,
    ) -> (BamSystemFn, *mut c_void),
) -> Result<BamSystemDesc, FfiResult> {
    let parts = pin_system_descriptor_storage(instance, config)?;
    let (callback_fn, user_data) = allocate_cb(
        instance,
        parts.total_query_count,
        parts.user_query_count,
        parts.resource_query_index,
    );
    assemble_and_validate_system_desc(parts, config, run_if, callback_fn, user_data)
}

/// Validates `config`, pins all owned strings/term slices and closure state inside `instance`,
/// and assembles a validated [`BamSystemDesc`] ready for `register_system` or
/// `register_system_handle`.
fn prepare_system_desc<F>(
    instance: &mut GuestInstance,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    closure: F,
) -> Result<BamSystemDesc, FfiResult>
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
{
    prepare_system_desc_with(instance, config, run_if, |inst, total, user, res_idx| {
        allocate_system_trampoline(inst, total, user, res_idx, closure)
    })
}

/// Validates and registers a unified [`BamSystemDesc`] on `load_ctx`, tracking all closure state
/// and owned descriptor buffers inside `instance`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn register_system_closure<F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    closure: F,
) -> FfiResult
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
{
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(register_fn) = instance.host().api().register_system else {
        return FfiResult::NULL_POINTER;
    };
    let desc = match prepare_system_desc(instance, config, run_if, closure) {
        Ok(d) => d,
        Err(err) => return err,
    };
    // SAFETY: `load_ctx` is active per caller contract and `desc` passed full validation with all
    // pointed-to strings, term slices, and closures owned by `instance`.
    unsafe { register_fn(load_ctx, &desc) }
}

/// Validates and registers a unified [`BamSystemDesc`] via `BamHostApi::register_system_handle`
/// (slot 60), returning the assigned [`BamSystem`] handle.
///
/// Returns [`FfiResult::UNSUPPORTED`] on ABI `0.2` hosts without allocating closure state.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn register_system_handle_closure<F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    closure: F,
) -> Result<BamSystem, FfiResult>
where
    F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
{
    if load_ctx.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    let register_handle_fn =
        host_slot!(instance.host().api(), register_system_handle).ok_or(FfiResult::UNSUPPORTED)?;
    let desc = prepare_system_desc(instance, config, run_if, closure)?;
    let mut out = BamSystem::INVALID;
    // SAFETY: `load_ctx` is active per caller contract, `desc` passed validation with all pointed-to
    // buffers owned by `instance`, and `&mut out` is aligned writable stack storage.
    let status = unsafe { register_handle_fn(load_ctx, &desc, &mut out) };
    if !status.is_ok() {
        return Err(status);
    }
    if !out.is_valid() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    Ok(out)
}

/// Validates and registers a unified [`BamSystemDesc`] backed by a const-bucketed
/// `system_closure_slice_trampoline<N, F>` on `load_ctx`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn register_system_slice_closure<const N: usize, F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    closure: F,
) -> FfiResult
where
    F: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
{
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(register_fn) = instance.host().api().register_system else {
        return FfiResult::NULL_POINTER;
    };
    let desc = match prepare_system_desc_with(instance, config, run_if, |inst, total, user, res| {
        allocate_system_slice_trampoline::<N, F>(inst, total, user, res, closure)
    }) {
        Ok(d) => d,
        Err(err) => return err,
    };
    // SAFETY: `load_ctx` is active per caller contract and `desc` passed full validation with all
    // pointed-to strings, term slices, and closures owned by `instance`.
    unsafe { register_fn(load_ctx, &desc) }
}

/// Validates and registers a unified [`BamSystemDesc`] backed by a const-bucketed
/// `system_closure_slice_trampoline<N, F>` via `BamHostApi::register_system_handle` (slot 60).
///
/// Returns [`FfiResult::UNSUPPORTED`] on ABI `0.2` hosts without allocating closure state.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn register_system_handle_slice_closure<const N: usize, F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    config: &SystemRegistrationConfig<'_>,
    run_if: Option<(BamRunIfFn, *mut c_void)>,
    closure: F,
) -> Result<BamSystem, FfiResult>
where
    F: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
{
    if load_ctx.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    let register_handle_fn =
        host_slot!(instance.host().api(), register_system_handle).ok_or(FfiResult::UNSUPPORTED)?;
    let desc = prepare_system_desc_with(instance, config, run_if, |inst, total, user, res| {
        allocate_system_slice_trampoline::<N, F>(inst, total, user, res, closure)
    })?;
    let mut out = BamSystem::INVALID;
    // SAFETY: `load_ctx` is active per caller contract, `desc` passed validation with all pointed-to
    // buffers owned by `instance`, and `&mut out` is aligned writable stack storage.
    let status = unsafe { register_handle_fn(load_ctx, &desc, &mut out) };
    if !status.is_ok() {
        return Err(status);
    }
    if !out.is_valid() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    Ok(out)
}

/// Raw wrapper invoking `BamHostApi::configure_schedule_set` (slot 61) after checking
/// `covers_slot`, `load_ctx`, and `desc.validate()`.
///
/// # Safety
/// - `load_ctx` must be a non-null, active [`BamLoadCtx`] during `bamabi_init`.
/// - `desc` must satisfy [`BamScheduleSetDesc::validate`] with valid `name`, `before_sets`, and
///   `after_sets` pointers for the duration of this call.
pub unsafe fn configure_schedule_set_raw(
    load_ctx: *mut BamLoadCtx,
    host: &Host,
    desc: &BamScheduleSetDesc,
) -> Result<BamSystemSet, FfiResult> {
    if load_ctx.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    let validation = desc.validate();
    if !validation.is_ok() {
        return Err(validation);
    }
    let configure_fn =
        host_slot!(host.api(), configure_schedule_set).ok_or(FfiResult::UNSUPPORTED)?;
    let mut out = BamSystemSet::INVALID;
    // SAFETY: `load_ctx` is active per caller contract, `desc` passed validation, and `&mut out`
    // is aligned writable stack storage.
    let status = unsafe { configure_fn(load_ctx, desc, &mut out) };
    if !status.is_ok() {
        return Err(status);
    }
    if !out.is_valid() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    Ok(out)
}

/// Configures a named schedule system set via `BamHostApi::configure_schedule_set` (slot 61),
/// storing an owned copy of `name` inside `instance` and returning its assigned [`BamSystemSet`].
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn configure_schedule_set(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &CStr,
    schedule: ScheduleId,
    flags: u32,
    before_sets: &[BamSystemSet],
    after_sets: &[BamSystemSet],
) -> Result<BamSystemSet, FfiResult> {
    if load_ctx.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    if name.to_bytes().is_empty() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    if name.to_str().is_err() {
        return Err(FfiResult::INVALID_UTF8);
    }
    if before_sets.iter().any(|s| !s.is_valid()) || after_sets.iter().any(|s| !s.is_valid()) {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    let before_count =
        u32::try_from(before_sets.len()).map_err(|_| FfiResult::CAPACITY_EXCEEDED)?;
    let after_count = u32::try_from(after_sets.len()).map_err(|_| FfiResult::CAPACITY_EXCEEDED)?;
    let host = *instance.host();
    if host_slot!(host.api(), configure_schedule_set).is_none() {
        return Err(FfiResult::UNSUPPORTED);
    }
    let before_ptr = if before_sets.is_empty() {
        core::ptr::null()
    } else {
        before_sets.as_ptr()
    };
    let after_ptr = if after_sets.is_empty() {
        core::ptr::null()
    } else {
        after_sets.as_ptr()
    };
    let mut probe_desc = BamScheduleSetDesc::new(name.as_ptr()).with_flags(flags);
    probe_desc.schedule = schedule;
    probe_desc.before_count = before_count;
    probe_desc.after_count = after_count;
    probe_desc.before_sets = before_ptr;
    probe_desc.after_sets = after_ptr;
    let validation = probe_desc.validate();
    if !validation.is_ok() {
        return Err(validation);
    }

    let owned_name = CString::from(name);
    let raw_name = instance.track_allocation(Box::new(owned_name));
    // SAFETY: `raw_name` stays pinned inside `instance` until `bamabi_shutdown` (or rollback).
    let name_ptr = unsafe { (*raw_name).as_ptr() };
    let mut desc = BamScheduleSetDesc::new(name_ptr).with_flags(flags);
    desc.schedule = schedule;
    desc.before_count = before_count;
    desc.after_count = after_count;
    desc.before_sets = before_ptr;
    desc.after_sets = after_ptr;
    // SAFETY: `load_ctx` is active per caller contract, `name_ptr` is owned by `instance`, and
    // `before_sets`/`after_sets` slices stay live across the synchronous host FFI call.
    unsafe { configure_schedule_set_raw(load_ctx, &host, &desc) }
}

/// Mounts a previously registered [`BamSystem`] handle into `schedule` and optional `set`
/// (`BamSystemSet::INVALID` for ungrouped mounting) via `BamHostApi::mount_system_in_schedule`
/// (slot 62).
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] during `bamabi_init`.
pub unsafe fn mount_system_in_schedule(
    load_ctx: *mut BamLoadCtx,
    host: &Host,
    system: BamSystem,
    schedule: ScheduleId,
    set: BamSystemSet,
) -> FfiResult {
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if !system.is_valid() || !schedule.is_valid() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let Some(mount_fn) = host_slot!(host.api(), mount_system_in_schedule) else {
        return FfiResult::UNSUPPORTED;
    };
    // SAFETY: `load_ctx` is active per caller contract and `system`/`schedule` passed validation.
    unsafe { mount_fn(load_ctx, system, schedule, set) }
}

/// Heap-pinned observer closure state owned by [`GuestInstance`] (or its [`RuntimeAllocTracker`]),
/// pairing a pointer to the instance's pinned [`Host`] with a [`ReentrantCell`]-guarded observer
/// callback.
pub(crate) struct ObserverTrampolineState<F> {
    pub(crate) host: PinnedHostPtr,
    pub(crate) callback: ReentrantCell<F>,
}

/// Panic-safe, re-entrancy-guarded `extern "C"` [`BamObserverFn`] trampoline for read-only custom
/// `E: AbiPod` event observers.
pub(crate) unsafe extern "C" fn observer_closure_trampoline<E, F>(
    target: FfiEntity,
    event_data: *const c_void,
    commands: *mut BamCommands,
    user_data: *mut c_void,
) -> FfiResult
where
    E: AbiPod,
    F: FnMut(FfiEntity, &E, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    catch_guest_result(|| {
        if user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<ObserverTrampolineState<F>>())
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        if core::mem::size_of::<E>() > 0
            && (event_data.is_null()
                || !(event_data as usize).is_multiple_of(core::mem::align_of::<E>()))
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and points to `ObserverTrampolineState<F>`
        // owned by `GuestInstance`.
        let state = unsafe { &*user_data.cast::<ObserverTrampolineState<F>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` outliving `state`.
        let host = unsafe { state.host.as_ref() };
        state
            .callback
            .try_with_mut(|cb| {
                let event_ref: &E = if core::mem::size_of::<E>() == 0 {
                    // SAFETY: Zero-sized `E: AbiPod` may be referenced via `NonNull::dangling()`.
                    unsafe { core::ptr::NonNull::<E>::dangling().as_ref() }
                } else {
                    // SAFETY: Verified non-null and aligned above; `try_with_mut` excludes
                    // re-entrant execution, and host guarantees `event_data` points to initialized `E`.
                    unsafe { &*event_data.cast::<E>() }
                };
                // SAFETY: `commands` is stack-scoped for this observer invocation.
                let mut cmds = unsafe { ValidatedCommands::from_raw(commands, host) };
                cb(target, event_ref, &mut cmds)
            })
            .unwrap_or(FfiResult::ACCESS_CONFLICT)
    })
}

/// Selects whether an observer's owned state is tracked in init-time [`GuestInstance::allocations`]
/// or in the thread-safe runtime [`RuntimeAllocTracker`] attached to [`Host`].
enum ObserverAllocTarget<'a> {
    Init(&'a mut GuestInstance),
    Runtime(&'a Host),
}

impl ObserverAllocTarget<'_> {
    /// Returns a stable [`PinnedHostPtr`] guaranteed to outlive the observer trampoline state.
    fn pinned_host_ptr(&self) -> Result<PinnedHostPtr, FfiResult> {
        match self {
            Self::Init(inst) => Ok(PinnedHostPtr::new(inst.pinned_host_ptr())),
            Self::Runtime(host) => host
                .pinned_host_ptr()
                .map(PinnedHostPtr::new)
                .ok_or(FfiResult::INVALID_STATE),
        }
    }

    /// Tracks `boxed` in the target lifetime container and returns its stable raw pointer.
    fn track<T: Send + 'static>(&mut self, boxed: Box<T>) -> Result<*mut T, FfiResult> {
        match self {
            Self::Init(inst) => Ok(inst.track_allocation(boxed)),
            Self::Runtime(host) => host
                .track_runtime_allocation(boxed)
                .ok_or(FfiResult::INVALID_STATE),
        }
    }
}

/// Discriminates the three observer registration shapes supported by [`build_observer_desc`].
enum ObserverKind {
    CustomRead(BamComponent, BamObserverFn),
    CustomMut(BamComponent, BamObserverMutFn),
    Lifecycle(BamEventKind, BamObserverFn),
}

/// Validates observer parameters, pins optional name/watched-component/closure allocations via
/// `alloc`, and returns a validated [`BamObserverDesc`].
#[allow(clippy::too_many_arguments)]
fn build_observer_desc<F: Send + 'static>(
    mut alloc: ObserverAllocTarget<'_>,
    name: Option<&str>,
    kind: ObserverKind,
    watched_components: &[BamComponent],
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> Result<BamObserverDesc, FfiResult> {
    match kind {
        ObserverKind::CustomRead(ev, _) | ObserverKind::CustomMut(ev, _) => {
            if !ev.is_valid() {
                return Err(FfiResult::INVALID_ARGUMENT);
            }
        }
        ObserverKind::Lifecycle(event_kind, _) => {
            if event_kind == BamEventKind::CUSTOM || !event_kind.is_valid() {
                return Err(FfiResult::INVALID_ARGUMENT);
            }
        }
    }

    let name_ptr = match name {
        Some(s) => {
            if s.is_empty() {
                return Err(FfiResult::INVALID_ARGUMENT);
            }
            let cstr = CString::new(s).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
            let raw_name = alloc.track(Box::new(cstr))?;
            // SAFETY: `raw_name` stays pinned inside `GuestInstance` until shutdown.
            unsafe { (*raw_name).as_ptr() }
        }
        None => core::ptr::null(),
    };

    let (watched_ptr, watched_len) = if watched_components.is_empty() {
        (core::ptr::null(), 0)
    } else {
        let boxed: Box<[BamComponent]> = watched_components.into();
        let len = boxed.len();
        let raw_watched = alloc.track(Box::new(boxed))?;
        // SAFETY: `raw_watched` stays pinned inside `GuestInstance` until shutdown.
        (unsafe { (*raw_watched).as_ptr() }, len)
    };

    let host = alloc.pinned_host_ptr()?;
    let raw_state = alloc.track(Box::new(ObserverTrampolineState {
        host,
        callback: ReentrantCell::new(callback),
    }))?;

    let mut desc = match kind {
        ObserverKind::CustomRead(event, cb) => {
            BamObserverDesc::new(event, cb, raw_state.cast::<c_void>())
        }
        ObserverKind::CustomMut(event, cb) => {
            BamObserverDesc::new_mut(event, cb, raw_state.cast::<c_void>())
        }
        ObserverKind::Lifecycle(event_kind, cb) => BamObserverDesc::new_lifecycle(
            event_kind,
            watched_ptr,
            watched_len,
            cb,
            raw_state.cast::<c_void>(),
        ),
    }
    .with_entity(target)
    .with_flags(flags);

    if !name_ptr.is_null() {
        desc = desc.with_name(name_ptr);
    }
    if propagate_relationship.is_valid() {
        desc = desc.with_propagate(propagate_relationship);
    }
    let validation = desc.validate();
    if !validation.is_ok() {
        return Err(validation);
    }
    Ok(desc)
}

/// Shared init-time observer registration helper that builds a [`BamObserverDesc`] via
/// [`build_observer_desc`] and submits it to `BamHostApi::register_observer`.
#[allow(clippy::too_many_arguments)]
unsafe fn submit_init_observer<F: Send + 'static>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &str,
    kind: ObserverKind,
    watched_components: &[BamComponent],
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> FfiResult {
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(register_fn) = host_slot!(instance.host().api(), register_observer) else {
        return FfiResult::UNSUPPORTED;
    };
    let desc = match build_observer_desc(
        ObserverAllocTarget::Init(instance),
        Some(name),
        kind,
        watched_components,
        target,
        flags,
        propagate_relationship,
        callback,
    ) {
        Ok(d) => d,
        Err(err) => return err,
    };
    // SAFETY: `load_ctx` is active per caller contract and `desc` passed validation with all
    // pointed-to strings, watched component lists, and closure state owned by `instance`.
    unsafe { register_fn(load_ctx, &desc) }
}

/// Validates and registers a dynamic [`BamObserverDesc`] closure on `load_ctx`, tracking its
/// heap-owned name and state inside `instance`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
pub unsafe fn register_observer_closure<E, F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &str,
    event: bamabi_sys::BamComponent,
    target: FfiEntity,
    callback: F,
) -> FfiResult
where
    E: AbiPod,
    F: FnMut(FfiEntity, &E, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    // SAFETY: Forwarding identical `load_ctx` invariant to `register_observer_closure_with_options`.
    unsafe {
        register_observer_closure_with_options::<E, F>(
            instance,
            load_ctx,
            name,
            event,
            target,
            0,
            BamComponent::INVALID,
            callback,
        )
    }
}

/// Panic-safe, re-entrancy-guarded `extern "C"` [`BamObserverMutFn`] trampoline for mutable custom
/// `E: AbiPod` event observers (`&mut E` + `&mut bool` propagation toggle).
pub(crate) unsafe extern "C" fn observer_mut_closure_trampoline<E, F>(
    target: FfiEntity,
    event_data: *mut u8,
    propagate: *mut FfiBool,
    commands: *mut BamCommands,
    user_data: *mut c_void,
) -> FfiResult
where
    E: AbiPod,
    F: FnMut(FfiEntity, &mut E, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult
        + Send
        + 'static,
{
    catch_guest_result(|| {
        if user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<ObserverTrampolineState<F>>())
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        if core::mem::size_of::<E>() > 0
            && (event_data.is_null()
                || !(event_data as usize).is_multiple_of(core::mem::align_of::<E>()))
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and points to `ObserverTrampolineState<F>`
        // owned by `GuestInstance`.
        let state = unsafe { &*user_data.cast::<ObserverTrampolineState<F>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` outliving `state`.
        let host = unsafe { state.host.as_ref() };
        // Acquire re-entrancy exclusivity BEFORE materializing `&mut E` or touching `*propagate`
        // so any re-entrant call rejects with `ACCESS_CONFLICT` without aliasing UB or state clobbering.
        state
            .callback
            .try_with_mut(|cb| {
                let event_ref: &mut E = if core::mem::size_of::<E>() == 0 {
                    // SAFETY: Zero-sized `E: AbiPod` may be mutably referenced via `NonNull::dangling()`.
                    unsafe { core::ptr::NonNull::<E>::dangling().as_mut() }
                } else {
                    // SAFETY: Verified non-null and aligned above; `try_with_mut` excludes
                    // re-entrant execution, and host guarantees exclusive access to `E` at
                    // `event_data` during `BamObserverMutFn` execution.
                    unsafe { &mut *event_data.cast::<E>() }
                };
                let mut prop_val = if propagate.is_null() {
                    false
                } else {
                    // SAFETY: When non-null, `propagate` points to an initialized `FfiBool` on the host stack.
                    unsafe { (*propagate).get() }
                };
                // SAFETY: `commands` is stack-scoped for this observer invocation.
                let mut cmds = unsafe { ValidatedCommands::from_raw(commands, host) };
                let status = cb(target, event_ref, &mut prop_val, &mut cmds);
                if !propagate.is_null() {
                    // SAFETY: `propagate` is non-null and writable for the duration of this callback.
                    unsafe {
                        *propagate = FfiBool::from_bool(prop_val);
                    }
                }
                status
            })
            .unwrap_or(FfiResult::ACCESS_CONFLICT)
    })
}

/// Panic-safe, re-entrancy-guarded `extern "C"` [`BamObserverFn`] trampoline for component
/// lifecycle (`ADD`/`INSERT`/`DISCARD`/`REMOVE`/`DESPAWN`) and scene `READY` observers.
pub(crate) unsafe extern "C" fn lifecycle_observer_closure_trampoline<F>(
    target: FfiEntity,
    _event_data: *const c_void,
    commands: *mut BamCommands,
    user_data: *mut c_void,
) -> FfiResult
where
    F: FnMut(FfiEntity, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    catch_guest_result(|| {
        if user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<ObserverTrampolineState<F>>())
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and points to `ObserverTrampolineState<F>`
        // owned by `GuestInstance`.
        let state = unsafe { &*user_data.cast::<ObserverTrampolineState<F>>() };
        // SAFETY: `state.host` points to `GuestInstance`'s heap-pinned `Host` outliving `state`.
        let host = unsafe { state.host.as_ref() };
        // SAFETY: `commands` is stack-scoped for this observer invocation.
        let mut cmds = unsafe { ValidatedCommands::from_raw(commands, host) };
        state
            .callback
            .try_with_mut(|cb| cb(target, &mut cmds))
            .unwrap_or(FfiResult::ACCESS_CONFLICT)
    })
}

/// Validates and registers a custom [`BamObserverDesc`] closure with explicit propagation flags
/// and optional `propagate_relationship` on `load_ctx`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn register_observer_closure_with_options<E, F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &str,
    event: BamComponent,
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> FfiResult
where
    E: AbiPod,
    F: FnMut(FfiEntity, &E, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    // SAFETY: Caller guarantees `load_ctx` is an active `BamLoadCtx` belonging to `instance`.
    unsafe {
        submit_init_observer(
            instance,
            load_ctx,
            name,
            ObserverKind::CustomRead(event, observer_closure_trampoline::<E, F>),
            &[],
            target,
            flags,
            propagate_relationship,
            callback,
        )
    }
}

/// Validates and registers a mutable [`BamObserverDesc`] closure (`&mut E` + `&mut bool` propagate)
/// on `load_ctx`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn register_observer_mut_closure<E, F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &str,
    event: BamComponent,
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> FfiResult
where
    E: AbiPod,
    F: FnMut(FfiEntity, &mut E, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult
        + Send
        + 'static,
{
    // SAFETY: Caller guarantees `load_ctx` is an active `BamLoadCtx` belonging to `instance`.
    unsafe {
        submit_init_observer(
            instance,
            load_ctx,
            name,
            ObserverKind::CustomMut(event, observer_mut_closure_trampoline::<E, F>),
            &[],
            target,
            flags,
            propagate_relationship,
            callback,
        )
    }
}

/// Validates and registers a component lifecycle or `Ready` [`BamObserverDesc`] closure on
/// `load_ctx`.
///
/// # Safety
/// `load_ctx` must be a non-null, active [`BamLoadCtx`] belonging to `instance` during
/// `bamabi_init`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn register_lifecycle_observer_closure<F>(
    instance: &mut GuestInstance,
    load_ctx: *mut BamLoadCtx,
    name: &str,
    event_kind: BamEventKind,
    watched_components: &[BamComponent],
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> FfiResult
where
    F: FnMut(FfiEntity, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    // SAFETY: Caller guarantees `load_ctx` is an active `BamLoadCtx` belonging to `instance`.
    unsafe {
        submit_init_observer(
            instance,
            load_ctx,
            name,
            ObserverKind::Lifecycle(event_kind, lifecycle_observer_closure_trampoline::<F>),
            watched_components,
            target,
            flags,
            propagate_relationship,
            callback,
        )
    }
}

/// Allocates a runtime read-only custom event [`BamObserverDesc`] tracked in `host`'s
/// [`RuntimeAllocTracker`] for [`ValidatedCommands::observe`].
pub(crate) fn allocate_runtime_observer_desc<E, F>(
    host: &Host,
    name: Option<&str>,
    event: BamComponent,
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> Result<BamObserverDesc, FfiResult>
where
    E: AbiPod,
    F: FnMut(FfiEntity, &E, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    build_observer_desc(
        ObserverAllocTarget::Runtime(host),
        name,
        ObserverKind::CustomRead(event, observer_closure_trampoline::<E, F>),
        &[],
        target,
        flags,
        propagate_relationship,
        callback,
    )
}

/// Allocates a runtime mutable custom event [`BamObserverDesc`] tracked in `host`'s
/// [`RuntimeAllocTracker`] for [`ValidatedCommands::observe_mut`].
pub(crate) fn allocate_runtime_observer_mut_desc<E, F>(
    host: &Host,
    name: Option<&str>,
    event: BamComponent,
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> Result<BamObserverDesc, FfiResult>
where
    E: AbiPod,
    F: FnMut(FfiEntity, &mut E, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult
        + Send
        + 'static,
{
    build_observer_desc(
        ObserverAllocTarget::Runtime(host),
        name,
        ObserverKind::CustomMut(event, observer_mut_closure_trampoline::<E, F>),
        &[],
        target,
        flags,
        propagate_relationship,
        callback,
    )
}

/// Allocates a runtime lifecycle or `Ready` [`BamObserverDesc`] tracked in `host`'s
/// [`RuntimeAllocTracker`] for [`ValidatedCommands::observe_lifecycle`].
pub(crate) fn allocate_runtime_lifecycle_observer_desc<F>(
    host: &Host,
    name: Option<&str>,
    event_kind: BamEventKind,
    watched_components: &[BamComponent],
    target: FfiEntity,
    flags: u32,
    propagate_relationship: BamComponent,
    callback: F,
) -> Result<BamObserverDesc, FfiResult>
where
    F: FnMut(FfiEntity, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
{
    build_observer_desc(
        ObserverAllocTarget::Runtime(host),
        name,
        ObserverKind::Lifecycle(event_kind, lifecycle_observer_closure_trampoline::<F>),
        watched_components,
        target,
        flags,
        propagate_relationship,
        callback,
    )
}

/// Generates a panic-safe [`BamSystemFn`] `extern "C"` wrapper around a Rust system function
/// `fn(*mut BamSystemCtx, *mut c_void) -> FfiResult`.
#[macro_export]
macro_rules! wrap_system_callback {
    ($wrapper_name:ident, $inner_fn:path) => {
        /// Panic-safe `extern "C"` [`bamabi_sys::BamSystemFn`] trampoline.
        ///
        /// # Safety
        /// `sys_ctx` must be a valid active [`bamabi_sys::BamSystemCtx`] pointer for the
        /// duration of the invocation.
        pub unsafe extern "C" fn $wrapper_name(
            sys_ctx: *mut $crate::BamSystemCtx,
            user_data: *mut ::core::ffi::c_void,
        ) -> $crate::FfiResult {
            $crate::catch_guest_result(|| {
                if sys_ctx.is_null() {
                    return $crate::FfiResult::NULL_POINTER;
                }
                // SAFETY: Caller guarantees `sys_ctx` and `user_data` satisfy `$inner_fn` contract.
                unsafe { $inner_fn(sys_ctx, user_data) }
            })
        }
    };
}

/// Generates a panic-safe [`BamRunIfFn`] `extern "C"` wrapper around a Rust predicate function
/// `fn(*mut c_void) -> FfiBool`.
#[macro_export]
macro_rules! wrap_run_condition {
    ($wrapper_name:ident, $inner_fn:path) => {
        /// Panic-safe `extern "C"` [`bamabi_sys::BamRunIfFn`] trampoline.
        ///
        /// # Safety
        /// `user_data` must satisfy the safety contract of `$inner_fn`.
        pub unsafe extern "C" fn $wrapper_name(
            user_data: *mut ::core::ffi::c_void,
        ) -> $crate::FfiBool {
            $crate::catch_guest_bool(|| {
                // SAFETY: Caller guarantees `user_data` satisfies `$inner_fn` contract.
                unsafe { $inner_fn(user_data) }
            })
        }
    };
}

// ============================================================================
// Same-Mod Dynamic `ModComponent` Verification, Borrowing & Move Commands
// ============================================================================

/// Resolves `"bam.v0.<comp_name>.vtable"` via `host.resolve_custom_symbol`, using a 256-byte
/// stack scratch buffer to avoid heap allocation on hot paths.
#[inline]
fn resolve_vtable_symbol_for_name(
    host: &Host,
    comp_name: &str,
) -> Result<*const c_void, FfiResult> {
    const PREFIX: &[u8] = b"bam.v0.";
    const SUFFIX: &[u8] = b".vtable\0";
    let total_len = PREFIX.len() + comp_name.len() + SUFFIX.len();
    if total_len <= 256 {
        let mut buf = [0u8; 256];
        buf[..PREFIX.len()].copy_from_slice(PREFIX);
        buf[PREFIX.len()..PREFIX.len() + comp_name.len()].copy_from_slice(comp_name.as_bytes());
        buf[PREFIX.len() + comp_name.len()..total_len].copy_from_slice(SUFFIX);
        let Ok(cstr) = CStr::from_bytes_with_nul(&buf[..total_len]) else {
            return Err(FfiResult::INVALID_ARGUMENT);
        };
        return host.resolve_custom_symbol(cstr);
    }
    let sym = format!("bam.v0.{comp_name}.vtable");
    let Ok(cstr) = CString::new(sym.as_bytes()) else {
        return Err(FfiResult::INVALID_ARGUMENT);
    };
    host.resolve_custom_symbol(cstr.as_c_str())
}

/// Verifies that a host column/component with `(actual_uuid, actual_size, actual_align)` matches
/// `T: ModComponent` registered by this same active mod instance (`vtable_ptr` address equality
/// proves identical Rust ABI + active `ModRegistry` ownership).
pub fn verify_same_mod_dynamic_component<T: ModComponent>(
    host: &Host,
    actual_uuid: ComponentUuid,
    actual_size: usize,
    actual_align: usize,
) -> bool {
    if actual_size != core::mem::size_of::<T>() || actual_align != core::mem::align_of::<T>() {
        return false;
    }
    let Ok((qualified_name, qualified_uuid)) =
        qualify_mod_component_identity::<T>(host.manifest_name())
    else {
        return false;
    };
    let matches_qualified = actual_uuid == qualified_uuid;
    let matches_canonical = actual_uuid == T::UUID;
    if !matches_qualified && !matches_canonical {
        return false;
    }
    let expected_vtable = T::erased_vtable_ptr();
    if matches_qualified
        && let Ok(ptr) = host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<T>(),
                crate::host::symbol_cache_kind::VTABLE_QUALIFIED,
            ),
            || resolve_vtable_symbol_for_name(host, &qualified_name),
        )
        && core::ptr::eq(ptr, expected_vtable)
    {
        return true;
    }
    if matches_canonical
        && qualified_name != T::CANONICAL_NAME
        && let Ok(ptr) = host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<T>(),
                crate::host::symbol_cache_kind::VTABLE_CANONICAL,
            ),
            || resolve_vtable_symbol_for_name(host, T::CANONICAL_NAME),
        )
        && core::ptr::eq(ptr, expected_vtable)
    {
        return true;
    }
    false
}

/// Validates that dense column `col` on `iter` holds a same-mod dynamic [`ModComponent`] `T`
/// with the requested read/write permission, returning `(chunk_len, is_optional)`.
fn validate_dynamic_dense_col<T: ModComponent>(
    iter: &ValidatedIter<'_>,
    col: usize,
    require_mutable: bool,
) -> Option<(usize, bool)> {
    let info = iter.column_info(col).ok()?;
    if !info.mode.has_entity_column_data() || info.is_resource() || info.is_sparse_set() {
        return None;
    }
    if require_mutable && (!info.mode.is_mutable() || info.is_immutable()) {
        return None;
    }
    if !verify_same_mod_dynamic_component::<T>(iter.host(), info.uuid, info.size, info.align) {
        return None;
    }
    Some((iter.entities().len(), info.mode.is_optional()))
}

/// Forms `&'b [T]` for a verified same-mod dynamic [`ModComponent`] column after checking
/// non-nullness, alignment, zero-sized handling, and `len * size_of::<T>() <= isize::MAX`.
#[inline]
unsafe fn cast_dynamic_slice<'b, T>(ptr: *const u8, len: usize) -> Option<&'b [T]> {
    let elem_size = core::mem::size_of::<T>();
    if elem_size == 0 {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: `T` is zero-sized, `ptr` is non-null (present), and `NonNull::dangling()` is
        // aligned for `T`.
        return Some(unsafe {
            core::slice::from_raw_parts(core::ptr::NonNull::<T>::dangling().as_ptr(), len)
        });
    }
    if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
        return None;
    }
    let byte_len = len.checked_mul(elem_size)?;
    if byte_len > isize::MAX as usize {
        return None;
    }
    // SAFETY: Caller verified same-mod `T` UUID + active VTable + layout and shared borrow `'b`.
    Some(unsafe { core::slice::from_raw_parts(ptr.cast::<T>(), len) })
}

/// Forms `&'b mut [T]` for a verified writable same-mod dynamic [`ModComponent`] column after
/// checking non-nullness, alignment, zero-sized handling, and `len * size_of::<T>() <= isize::MAX`.
#[inline]
unsafe fn cast_dynamic_slice_mut<'b, T>(ptr: *mut u8, len: usize) -> Option<&'b mut [T]> {
    let elem_size = core::mem::size_of::<T>();
    if elem_size == 0 {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: `T` is zero-sized and `NonNull::dangling()` forms a sound mutable ZST slice.
        return Some(unsafe {
            core::slice::from_raw_parts_mut(core::ptr::NonNull::<T>::dangling().as_ptr(), len)
        });
    }
    if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
        return None;
    }
    let byte_len = len.checked_mul(elem_size)?;
    if byte_len > isize::MAX as usize {
        return None;
    }
    // SAFETY: Caller verified writable dense column, same-mod `T` layout/VTable, and exclusive `'b`.
    Some(unsafe { core::slice::from_raw_parts_mut(ptr.cast::<T>(), len) })
}

/// Borrows dense column `col` as `&[T]` for a same-mod dynamic [`ModComponent`] (`T`), verifying
/// layout, [`ComponentUuid`], and active same-binary VTable provenance.
pub fn iter_dynamic_slice<'b, 'a, T: ModComponent>(
    iter: &'b ValidatedIter<'a>,
    col: usize,
) -> Option<&'b [T]> {
    let (len, is_optional) = validate_dynamic_dense_col::<T>(iter, col, false)?;
    if len == 0 {
        return (!is_optional).then_some(&[][..]);
    }
    let ptr = iter.column_readonly_ptr(col);
    // SAFETY: `validate_dynamic_dense_col` verified same-mod `T` identity and `&'b iter` holds shared access.
    unsafe { cast_dynamic_slice::<T>(ptr, len) }
}

/// Mutably borrows dense column `col` as `&mut [T]` for a same-mod dynamic [`ModComponent`] (`T`).
pub fn iter_dynamic_slice_mut<'b, 'a, T: ModComponent>(
    iter: &'b mut ValidatedIter<'a>,
    col: usize,
    mark_changed: bool,
) -> Option<&'b mut [T]> {
    let (len, is_optional) = validate_dynamic_dense_col::<T>(iter, col, true)?;
    if len == 0 {
        return (!is_optional).then_some(&mut [][..]);
    }
    let ptr = if mark_changed {
        iter.column_mutable_ptr(col)
    } else {
        iter.column_mutable_untracked_ptr(col)
    };
    // SAFETY: `validate_dynamic_dense_col` verified writable same-mod `T` and `&'b mut iter` holds exclusive access.
    unsafe { cast_dynamic_slice_mut::<T>(ptr, len) }
}

/// Simultaneously borrows `[FfiEntity; len]` and writable dense dynamic component slice `&mut [T]`
/// (`T: ModComponent`) for the current chunk, verifying byte-range disjointness.
pub fn iter_entities_and_dynamic_slice_mut<'b, 'a, T: ModComponent>(
    iter: &'b mut ValidatedIter<'a>,
    write_col: usize,
) -> Option<(&'b [FfiEntity], &'b mut [T])> {
    let (len, is_optional) = validate_dynamic_dense_col::<T>(iter, write_col, true)?;
    if len == 0 {
        return (!is_optional).then_some((&[][..], &mut [][..]));
    }
    let elem_size = core::mem::size_of::<T>();
    let ptr = iter.column_mutable_ptr(write_col);
    let entities = iter.entities();
    if entities.len() != len {
        return None;
    }
    if elem_size > 0 {
        if ptr.is_null() {
            return None;
        }
        let e_bytes = len.checked_mul(core::mem::size_of::<FfiEntity>())?;
        let w_bytes = len.checked_mul(elem_size)?;
        if e_bytes > isize::MAX as usize || w_bytes > isize::MAX as usize {
            return None;
        }
        let e_start = entities.as_ptr().addr();
        let w_start = ptr.addr();
        let e_end = e_start.checked_add(e_bytes)?;
        let w_end = w_start.checked_add(w_bytes)?;
        if e_start < w_end && w_start < e_end {
            return None;
        }
    }
    // SAFETY: Verified non-overlapping with `entities`, writable same-mod `T`, and exclusive `&'b mut iter`.
    let slice_mut = unsafe { cast_dynamic_slice_mut::<T>(ptr, len) }?;
    Some((entities, slice_mut))
}

impl<'a> ValidatedSystemRef<'a> {
    /// Looks up a shared `&'b T` reference for a same-mod dynamic [`ModComponent`] on `entity`
    /// using declared access on `(query_index, term_index)`.
    pub fn entity_get_dynamic_checked<'b, T: ModComponent>(
        &'b self,
        iter: &'b ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&'b T> {
        let host = self.host();
        let ptr = self.raw_entity_ptr_ro(iter, query_index, term_index, entity, |info| {
            verify_same_mod_dynamic_component::<T>(host, info.uuid, info.size, info.align)
        })?;
        // SAFETY: Host verified query membership & declared access; `verify_same_mod_dynamic_component`
        // confirmed `T`'s layout, UUID, and active same-binary VTable.
        unsafe { Self::cast_entity_ref::<T>(ptr) }
    }

    /// Looks up an exclusive `&'b mut T` reference for a same-mod dynamic [`ModComponent`] on
    /// `entity` using declared writable access on `(query_index, term_index)`.
    pub fn entity_get_dynamic_mut_checked<'b, T: ModComponent>(
        &'b mut self,
        iter: &'b mut ValidatedIter<'a>,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&'b mut T> {
        let host = self.host();
        let ptr = self.raw_entity_ptr_mut(iter, query_index, term_index, entity, |info| {
            verify_same_mod_dynamic_component::<T>(host, info.uuid, info.size, info.align)
        })?;
        // SAFETY: Host verified writable access & stamped change ticks; `T`'s layout, UUID, and
        // active same-binary VTable were verified.
        unsafe { Self::cast_entity_mut::<T>(ptr) }
    }
}

/// Verifies via `Host::component_layout_info` and [`verify_same_mod_dynamic_component`] that
/// `component` refers to same-mod dynamic type `T` before transferring ownership into host commands.
fn verify_moved_component_handle<T: ModComponent>(
    host: &Host,
    component: BamComponent,
) -> Result<(), FfiResult> {
    if !component.is_valid() {
        return Err(FfiResult::INVALID_COMPONENT);
    }
    let info = host.component_layout_info(component)?;
    if !verify_same_mod_dynamic_component::<T>(host, info.uuid, info.size, info.align) {
        return Err(FfiResult::TYPE_MISMATCH);
    }
    Ok(())
}

impl<'a> ValidatedCommands<'a> {
    /// Moves an owned non-POD (or POD) [`ModComponent`] `value` into `BamHostApi::cmd_insert` via
    /// [`ManuallyDrop`], verifying `component` matches same-mod `T` via `Host::component_layout_info`
    /// and dropping `value` on the guest heap if validation or queueing fails.
    pub fn insert_moved<T: ModComponent>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: T,
    ) -> FfiResult {
        let mut slot = ManuallyDrop::new(value);
        if !self.is_valid() {
            // SAFETY: Command buffer is null; ownership never transferred to the host.
            unsafe { ManuallyDrop::drop(&mut slot) };
            return FfiResult::NULL_POINTER;
        }
        if let Err(err) = verify_moved_component_handle::<T>(self.host(), component) {
            // SAFETY: Handle validation failed before transferring ownership to the host.
            unsafe { ManuallyDrop::drop(&mut slot) };
            return err;
        }
        let data_ptr = if core::mem::size_of::<T>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            (&raw const *slot).cast::<u8>()
        };
        // SAFETY: `verify_moved_component_handle` confirmed `component` is registered as same-mod
        // `T` with matching layout, UUID, and active guest VTable/drop_fn. On `FfiResult::OK`, the
        // host `CommandArena` copied `size_of::<T>()` bytes with ownership; on error, `slot` is
        // dropped cleanly below on the guest heap.
        let status =
            unsafe { self.insert_raw(entity, component, data_ptr, core::mem::size_of::<T>()) };
        if !status.is_ok() {
            // SAFETY: `insert_raw` failed before transferring ownership to the host.
            unsafe {
                ManuallyDrop::drop(&mut slot);
            }
        }
        status
    }

    /// Spawns a fresh entity (mod-scoped or persistent) with an owned non-POD (or POD)
    /// [`ModComponent`] `value` plus optional `extra` [`SpawnEntry`] items, validating `component`
    /// and every `extra` entry against `Host::component_layout_info` and dropping `value` on the
    /// guest heap if validation or spawning fails.
    pub fn spawn_moved<T: ModComponent>(
        &mut self,
        component: BamComponent,
        value: T,
        extra: &[SpawnEntry<'_>],
        persistent: bool,
    ) -> Result<FfiEntity, FfiResult> {
        let mut slot = ManuallyDrop::new(value);
        if !self.is_valid() {
            // SAFETY: Command buffer is null; ownership never transferred to the host.
            unsafe { ManuallyDrop::drop(&mut slot) };
            return Err(FfiResult::NULL_POINTER);
        }
        if let Err(err) = verify_moved_component_handle::<T>(self.host(), component) {
            // SAFETY: Handle validation failed before transferring ownership to the host.
            unsafe { ManuallyDrop::drop(&mut slot) };
            return Err(err);
        }
        for entry in extra {
            if let Err(err) = validate_spawn_entry(self.host(), entry) {
                // SAFETY: Extra entry validation failed before transferring ownership to the host.
                unsafe { ManuallyDrop::drop(&mut slot) };
                return Err(err);
            }
        }
        let val_ptr = if core::mem::size_of::<T>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            (&raw const *slot).cast::<u8>()
        };
        let mut comps = Vec::with_capacity(1 + extra.len());
        let mut ptrs = Vec::with_capacity(1 + extra.len());
        comps.push(component);
        ptrs.push(val_ptr);
        for entry in extra {
            comps.push(entry.component());
            ptrs.push(entry.data_ptr());
        }
        // SAFETY: `comps` and `ptrs` have matching length, `component` was verified as same-mod `T`,
        // `slot` remains live across the call, and every `extra` entry passed layout validation.
        let res = unsafe {
            if persistent {
                self.spawn_persistent_raw(&comps, &ptrs)
            } else {
                self.spawn_raw(&comps, &ptrs)
            }
        };
        if res.is_err() {
            // SAFETY: Spawn failed before transferring ownership into `CommandArena`.
            unsafe {
                ManuallyDrop::drop(&mut slot);
            }
        }
        res
    }
}
