//! Validated handle-based and batched deferred command wrappers over [`BamCommands`].

use core::{ffi::c_void, marker::PhantomData};

use bamabi_sys::{
    AbiPod, BamCommands, BamComponent, BamEventKind, BamHostApi, BamHostCtx, BamObserverDesc,
    BamTermDesc, BamTriggerDesc, FfiAssetHandle, FfiEntity, FfiResult,
};

use crate::{
    host::Host,
    trampoline::{
        LazyInvocationHost, ObserverCtxRegistrationConfig, ValidatedSystemSlice,
        allocate_runtime_lifecycle_observer_desc, allocate_runtime_observer_ctx_slice_desc,
        allocate_runtime_observer_desc, allocate_runtime_observer_mut_desc,
    },
};

/// Stack scratch capacity before falling back to a temporary `Vec` in [`ValidatedCommands::spawn`].
const INLINE_SPAWN_CAP: usize = 16;

/// Init-validated component handle statically bound to `T: AbiPod`.
///
/// Constructed via [`TypedComponent::validate`] (which verifies the handle and its host-registered
/// `(size, align)` once against `Host::component_layout_info`) or [`TypedComponent::from_raw_unchecked`],
/// enabling hot-path commands (`SpawnEntry::typed`, `SpawnBatchColumn::typed`,
/// `ValidatedCommands::insert_typed`, `ValidatedCommands::insert_if_new_typed`,
/// `ValidatedCommands::insert_resource_typed`) to skip redundant per-command `component_layout_info`
/// FFI round-trips.
#[repr(transparent)]
pub struct TypedComponent<T: AbiPod> {
    handle: BamComponent,
    _marker: PhantomData<fn() -> T>,
}

impl<T: AbiPod> Copy for TypedComponent<T> {}

impl<T: AbiPod> Clone for TypedComponent<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: AbiPod> core::fmt::Debug for TypedComponent<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TypedComponent")
            .field("handle", &self.handle)
            .finish()
    }
}

impl<T: AbiPod> PartialEq for TypedComponent<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle
    }
}

impl<T: AbiPod> Eq for TypedComponent<T> {}

impl<T: AbiPod> core::hash::Hash for TypedComponent<T> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.handle.hash(state);
    }
}

impl<T: AbiPod> TypedComponent<T> {
    /// Validates `handle` against `host.component_layout_info(handle)`, verifying `handle.is_valid()`
    /// and that the registered `(size, align)` matches `T`.
    ///
    /// Fails closed with [`FfiResult::INVALID_COMPONENT`], [`FfiResult::TYPE_MISMATCH`], or any
    /// host error (including [`FfiResult::UNSUPPORTED`] on older ABI `0.2` hosts).
    #[inline]
    pub fn validate(host: &Host, handle: BamComponent) -> Result<Self, FfiResult> {
        if !handle.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        let info = host.component_layout_info(handle)?;
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        Ok(Self {
            handle,
            _marker: PhantomData,
        })
    }

    /// Constructs a [`TypedComponent<T>`] from a raw [`BamComponent`] handle without querying
    /// `Host::component_layout_info`.
    ///
    /// # Safety
    /// `handle` must be a valid host-registered component handle whose registered byte size and
    /// alignment match `core::mem::size_of::<T>()` and `core::mem::align_of::<T>()`.
    #[inline]
    pub const unsafe fn from_raw_unchecked(handle: BamComponent) -> Self {
        Self {
            handle,
            _marker: PhantomData,
        }
    }

    /// Returns the underlying [`BamComponent`] handle.
    #[inline]
    pub const fn handle(self) -> BamComponent {
        self.handle
    }

    /// Returns the underlying [`BamComponent`] handle (alias for [`Self::handle`]).
    #[inline]
    pub const fn raw(self) -> BamComponent {
        self.handle
    }
}

/// Type-erased `(BamComponent, *const u8)` pair for batched single-archetype [`ValidatedCommands::spawn`].
///
/// Constructed safely via [`SpawnEntry::new`] / [`SpawnEntry::marker`] carrying exact byte layout
/// metadata verified against `Host::component_layout_info` before submitting to `cmd_spawn`,
/// without materializing `&[u8]` over struct padding bytes.
#[derive(Clone, Copy, Debug)]
pub struct SpawnEntry<'a> {
    component: BamComponent,
    data_ptr: *const u8,
    size: Option<usize>,
    align: Option<usize>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> SpawnEntry<'a> {
    /// Binds a pre-resolved [`BamComponent`] handle to an `AbiPod` component value for batched spawn.
    #[inline]
    pub const fn new<T: AbiPod>(component: BamComponent, value: &'a T) -> Self {
        Self {
            component,
            data_ptr: core::ptr::from_ref(value).cast::<u8>(),
            size: Some(core::mem::size_of::<T>()),
            align: Some(core::mem::align_of::<T>()),
            _marker: PhantomData,
        }
    }

    /// Binds an init-validated [`TypedComponent<T>`] handle to an `AbiPod` component value for
    /// batched spawn, skipping the per-call `Host::component_layout_info` FFI round-trip.
    #[inline]
    pub const fn typed<T: AbiPod>(component: TypedComponent<T>, value: &'a T) -> Self {
        Self {
            component: component.handle(),
            data_ptr: core::ptr::from_ref(value).cast::<u8>(),
            size: None,
            align: None,
            _marker: PhantomData,
        }
    }

    /// Binds a pre-resolved zero-sized marker [`BamComponent`] handle (`size == 0`) with a non-null
    /// dangling data pointer.
    #[inline]
    pub const fn marker(component: BamComponent) -> Self {
        Self {
            component,
            data_ptr: core::ptr::NonNull::<u8>::dangling().as_ptr(),
            size: Some(0),
            align: None,
            _marker: PhantomData,
        }
    }

    /// Constructs a raw `SpawnEntry` from a caller-managed byte pointer.
    ///
    /// # Safety
    /// If the registered size of `component` is non-zero, `data_ptr` must be non-null and readable
    /// for that many bytes for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw(component: BamComponent, data_ptr: *const u8) -> Self {
        Self {
            component,
            data_ptr,
            size: None,
            align: None,
            _marker: PhantomData,
        }
    }

    /// Returns the [`BamComponent`] handle for this entry.
    #[inline]
    pub const fn component(&self) -> BamComponent {
        self.component
    }

    /// Returns the raw payload pointer for this entry.
    #[inline]
    pub const fn data_ptr(&self) -> *const u8 {
        self.data_ptr
    }

    /// Returns the statically known byte size of this entry's payload (`None` for [`Self::from_raw`]).
    #[inline]
    pub const fn size(&self) -> Option<usize> {
        self.size
    }

    /// Returns the statically known alignment of this entry's payload (`None` for markers / raw).
    #[inline]
    pub const fn align(&self) -> Option<usize> {
        self.align
    }
}

/// Validates a [`SpawnEntry`] against `Host::component_layout_info_with_ctx` (failing closed on
/// layout mismatch or `UNSUPPORTED` when safe layout metadata is present), resolving `host_ctx`
/// lazily only when `entry.size` is `Some`.
///
/// # Safety
/// `resolve_ctx` must return a valid active [`BamHostCtx`] matching `api` for the duration of this call.
pub(crate) unsafe fn validate_spawn_entry_with_ctx(
    api: &BamHostApi,
    resolve_ctx: impl FnOnce() -> *const BamHostCtx,
    entry: &SpawnEntry<'_>,
) -> Result<(), FfiResult> {
    if !entry.component.is_valid() {
        return Err(FfiResult::INVALID_COMPONENT);
    }
    if let Some(expected_size) = entry.size {
        // SAFETY: Caller guarantees `resolve_ctx()` returns a valid active `BamHostCtx` matching `api`.
        let info =
            unsafe { Host::component_layout_info_with_ctx(api, resolve_ctx(), entry.component) }?;
        if info.size != expected_size {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(expected_align) = entry.align
            && info.align != expected_align
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if expected_size > 0 && entry.data_ptr.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
    }
    Ok(())
}

/// Validates a [`SpawnEntry`] against `host.component_layout_info(entry.component)` (failing
/// closed on layout mismatch or `UNSUPPORTED` when safe layout metadata is present).
pub(crate) fn validate_spawn_entry(host: &Host, entry: &SpawnEntry<'_>) -> Result<(), FfiResult> {
    // SAFETY: `host.host_ctx()` is a valid active `BamHostCtx` matching `host.api()`.
    unsafe { validate_spawn_entry_with_ctx(host.api(), || host.host_ctx(), entry) }
}

/// Columnar slice descriptor `(BamComponent, *const u8, len)` for multi-entity
/// [`ValidatedCommands::spawn_batch_columns`].
#[derive(Clone, Copy, Debug)]
pub struct SpawnBatchColumn<'a> {
    component: BamComponent,
    column_ptr: *const u8,
    entity_count: usize,
    elem_size: Option<usize>,
    elem_align: Option<usize>,
    validated_typed: bool,
    _marker: PhantomData<&'a ()>,
}

impl<'a> SpawnBatchColumn<'a> {
    /// Binds a contiguous slice of `values` (`T: AbiPod`) to `component` for multi-entity spawn.
    #[inline]
    pub const fn new<T: AbiPod>(component: BamComponent, values: &'a [T]) -> Self {
        let ptr = if core::mem::size_of::<T>() == 0 || values.is_empty() {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            values.as_ptr().cast::<u8>()
        };
        Self {
            component,
            column_ptr: ptr,
            entity_count: values.len(),
            elem_size: Some(core::mem::size_of::<T>()),
            elem_align: Some(core::mem::align_of::<T>()),
            validated_typed: false,
            _marker: PhantomData,
        }
    }

    /// Binds an init-validated [`TypedComponent<T>`] handle to a contiguous slice of `values`
    /// (`T: AbiPod`) for multi-entity spawn, skipping the per-call `Host::component_layout_info`
    /// and `BamHostApi::sys_host_ctx` FFI round-trips while still enforcing local `entity_count`,
    /// non-null, and `isize::MAX` byte-length bounds.
    #[inline]
    pub const fn typed<T: AbiPod>(component: TypedComponent<T>, values: &'a [T]) -> Self {
        let ptr = if core::mem::size_of::<T>() == 0 || values.is_empty() {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            values.as_ptr().cast::<u8>()
        };
        Self {
            component: component.handle(),
            column_ptr: ptr,
            entity_count: values.len(),
            elem_size: Some(core::mem::size_of::<T>()),
            elem_align: Some(core::mem::align_of::<T>()),
            validated_typed: true,
            _marker: PhantomData,
        }
    }

    /// Binds a zero-sized marker `component` repeated across `entity_count` spawned entities.
    #[inline]
    pub const fn marker(component: BamComponent, entity_count: usize) -> Self {
        Self {
            component,
            column_ptr: core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const(),
            entity_count,
            elem_size: Some(0),
            elem_align: None,
            validated_typed: false,
            _marker: PhantomData,
        }
    }

    /// Constructs a raw [`SpawnBatchColumn`] from a caller-managed contiguous column byte pointer.
    ///
    /// # Safety
    /// If the registered size `S` of `component` is non-zero and `entity_count > 0`, `column_ptr`
    /// must be non-null and readable for `entity_count * S` bytes for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw(
        component: BamComponent,
        column_ptr: *const u8,
        entity_count: usize,
    ) -> Self {
        Self {
            component,
            column_ptr,
            entity_count,
            elem_size: None,
            elem_align: None,
            validated_typed: false,
            _marker: PhantomData,
        }
    }

    /// Returns the [`BamComponent`] handle for this column.
    #[inline]
    pub const fn component(&self) -> BamComponent {
        self.component
    }

    /// Returns the raw column byte pointer.
    #[inline]
    pub const fn column_ptr(&self) -> *const u8 {
        self.column_ptr
    }

    /// Returns the number of entity rows carried in this column slice.
    #[inline]
    pub const fn entity_count(&self) -> usize {
        self.entity_count
    }

    /// Returns the statically known element byte size (`None` for [`Self::from_raw`]).
    #[inline]
    pub const fn elem_size(&self) -> Option<usize> {
        self.elem_size
    }

    /// Returns the statically known element alignment (`None` for markers / raw).
    #[inline]
    pub const fn elem_align(&self) -> Option<usize> {
        self.elem_align
    }

    /// Constructs a [`SpawnBatchColumn`] from raw parts for unit tests and Kani bounds harnesses so
    /// symbolic null pointers and `isize::MAX` byte-length overflows can be exercised directly.
    #[cfg(any(test, kani))]
    #[inline]
    pub(crate) const unsafe fn from_parts_for_kani(
        component: BamComponent,
        column_ptr: *const u8,
        entity_count: usize,
        elem_size: Option<usize>,
        elem_align: Option<usize>,
        validated_typed: bool,
    ) -> Self {
        Self {
            component,
            column_ptr,
            entity_count,
            elem_size,
            elem_align,
            validated_typed,
            _marker: PhantomData,
        }
    }
}

/// Validates a [`SpawnBatchColumn`] against `Host::component_layout_info_with_ctx` (unless
/// already init-validated via [`SpawnBatchColumn::typed`]) and checks buffer bounds before passing
/// columnar pointers to `BamHostApi::cmd_spawn_batch`, invoking `resolve_ctx` only when an FFI
/// `component_layout_info` call is required.
///
/// # Safety
/// `resolve_ctx` must return a valid active [`BamHostCtx`] matching `api` for the duration of this call.
pub(crate) unsafe fn validate_spawn_batch_column_with_ctx(
    api: &BamHostApi,
    resolve_ctx: impl FnOnce() -> *const BamHostCtx,
    col: &SpawnBatchColumn<'_>,
    entity_count: usize,
) -> FfiResult {
    if !col.component.is_valid() {
        return FfiResult::INVALID_COMPONENT;
    }
    if col.entity_count < entity_count {
        return FfiResult::INVALID_ARGUMENT;
    }
    if let Some(expected_size) = col.elem_size {
        if !col.validated_typed {
            // SAFETY: Caller guarantees `resolve_ctx()` returns a valid active `BamHostCtx` matching `api`.
            let info = match unsafe {
                Host::component_layout_info_with_ctx(api, resolve_ctx(), col.component)
            } {
                Ok(i) => i,
                Err(err) => return err,
            };
            if info.size != expected_size {
                return FfiResult::TYPE_MISMATCH;
            }
            if let Some(expected_align) = col.elem_align
                && info.align != expected_align
            {
                return FfiResult::TYPE_MISMATCH;
            }
        }
        if entity_count > 0 && expected_size > 0 {
            if col.column_ptr.is_null() {
                return FfiResult::NULL_POINTER;
            }
            if entity_count
                .checked_mul(expected_size)
                .is_none_or(|b| b > isize::MAX as usize)
            {
                return FfiResult::OUT_OF_RANGE;
            }
        }
    }
    FfiResult::OK
}

/// Validates a [`SpawnBatchColumn`] against `host.component_layout_info(col.component)` and
/// checks buffer bounds before passing columnar pointers to `BamHostApi::cmd_spawn_batch`.
///
/// Verifies that `col.component` is valid, `col.entity_count >= entity_count`, layout `(size,
/// align)` matches the host's registered metadata when statically known, non-empty columns are
/// non-null, and `entity_count * expected_size` does not overflow `isize::MAX`.
#[cfg(kani)]
pub(crate) fn validate_spawn_batch_column(
    host: &Host,
    col: &SpawnBatchColumn<'_>,
    entity_count: usize,
) -> FfiResult {
    // SAFETY: `host.host_ctx()` is a valid active `BamHostCtx` matching `host.api()`.
    unsafe {
        validate_spawn_batch_column_with_ctx(host.api(), || host.host_ctx(), col, entity_count)
    }
}

/// Validated deferred command buffer wrapper bound to an active system invocation.
pub struct ValidatedCommands<'a> {
    raw: *mut BamCommands,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
}

impl<'a> ValidatedCommands<'a> {
    /// Wraps a raw [`BamCommands`] pointer obtained from `BamHostApi::sys_commands`.
    ///
    /// A null `raw` pointer constructs a disabled buffer whose command methods fail cleanly with
    /// [`FfiResult::NULL_POINTER`].
    ///
    /// # Safety
    /// If `raw` is non-null, it must point to the active [`BamCommands`] belonging to the current
    /// system invocation on `host` and must not outlive `'a`.
    #[inline]
    pub const unsafe fn from_raw(raw: *mut BamCommands, host: &'a Host) -> Self {
        Self {
            raw,
            host,
            lazy_host: None,
        }
    }

    /// Wraps a raw [`BamCommands`] pointer paired with an optional lazy per-tick host resolver.
    ///
    /// # Safety
    /// If `raw` is non-null, it must point to the active [`BamCommands`] belonging to the current
    /// system invocation on `host` and must not outlive `'a`.
    #[inline]
    pub(crate) const unsafe fn from_raw_with_lazy_host(
        raw: *mut BamCommands,
        host: &'a Host,
        lazy_host: Option<&'a LazyInvocationHost<'a>>,
    ) -> Self {
        Self {
            raw,
            host,
            lazy_host,
        }
    }

    /// Constructs a disabled command buffer (`raw = null`) that rejects operations with
    /// [`FfiResult::NULL_POINTER`].
    #[inline]
    pub const fn empty(host: &'a Host) -> Self {
        Self {
            raw: core::ptr::null_mut(),
            host,
            lazy_host: None,
        }
    }

    /// Returns `true` if this command buffer holds a non-null [`BamCommands`] handle.
    #[inline]
    pub fn is_valid(&self) -> bool {
        !self.raw.is_null()
    }

    /// Returns the underlying `*mut BamCommands` pointer.
    #[inline]
    pub const fn as_raw(&self) -> *mut BamCommands {
        self.raw
    }

    /// Returns the init-time base [`Host`] table without invoking `BamHostApi::sys_host_ctx`.
    #[inline]
    pub const fn base_host(&self) -> &'a Host {
        self.host
    }

    /// Resolves and caches the active per-tick [`BamHostCtx`] pointer at most once per invocation
    /// without materializing a full [`Host`] copy.
    #[inline]
    pub(crate) fn resolve_ctx(&self) -> *const BamHostCtx {
        match self.lazy_host {
            Some(lazy) => lazy.resolve_ctx(),
            None => self.host.host_ctx(),
        }
    }

    /// Returns the active [`Host`] table for this system invocation, lazily resolving
    /// `BamHostApi::sys_host_ctx` on demand when backed by a trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        match self.lazy_host {
            Some(lazy) => lazy.resolve(),
            None => self.host,
        }
    }

    /// Reserves a fresh empty entity atomically and returns its [`FfiEntity`] identifier.
    #[inline]
    pub fn spawn_empty(&mut self) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let Some(spawn_empty_fn) = self.host.api().cmd_spawn_empty else {
            return Err(FfiResult::NULL_POINTER);
        };
        let mut out = FfiEntity::NULL;
        // SAFETY: `self.raw` is a valid active `BamCommands` for `'a` and `out` is writable stack storage.
        let status = unsafe { spawn_empty_fn(self.raw, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Shared implementation for [`Self::spawn`] and [`Self::spawn_persistent`].
    ///
    /// Validates every [`SpawnEntry`] via [`validate_spawn_entry`], packs handles and data
    /// pointers into stack scratch arrays when `entries.len() <= INLINE_SPAWN_CAP` (zero heap
    /// allocation on hot paths) or falls back to temporary `Vec`s for larger archetypes, and
    /// forwards to [`Self::spawn_raw`] or [`Self::spawn_persistent_raw`].
    #[inline]
    fn spawn_entries_impl(
        &mut self,
        entries: &[SpawnEntry<'_>],
        persistent: bool,
    ) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if entries.is_empty() {
            return if persistent {
                // SAFETY: Empty slices require no component payload pointers.
                unsafe { self.spawn_persistent_raw(&[], &[]) }
            } else {
                self.spawn_empty()
            };
        }
        if entries.len() <= INLINE_SPAWN_CAP {
            let mut comps =
                [const { core::mem::MaybeUninit::<BamComponent>::uninit() }; INLINE_SPAWN_CAP];
            let mut ptrs =
                [const { core::mem::MaybeUninit::<*const u8>::uninit() }; INLINE_SPAWN_CAP];
            for (i, entry) in entries.iter().enumerate() {
                // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
                unsafe {
                    validate_spawn_entry_with_ctx(self.host.api(), || self.resolve_ctx(), entry)?;
                }
                comps[i].write(entry.component);
                ptrs[i].write(entry.data_ptr);
            }
            // SAFETY: The loop above initialized `comps[0..entries.len()]` and
            // `ptrs[0..entries.len()]`, `MaybeUninit<T>` is `#[repr(transparent)]`, and each safe
            // `SpawnEntry` passed `validate_spawn_entry_with_ctx` while raw entries satisfy `from_raw`.
            unsafe {
                let c = core::slice::from_raw_parts(
                    comps.as_ptr().cast::<BamComponent>(),
                    entries.len(),
                );
                let p =
                    core::slice::from_raw_parts(ptrs.as_ptr().cast::<*const u8>(), entries.len());
                if persistent {
                    self.spawn_persistent_raw(c, p)
                } else {
                    self.spawn_raw(c, p)
                }
            }
        } else {
            let mut comps = Vec::with_capacity(entries.len());
            let mut ptrs = Vec::with_capacity(entries.len());
            for entry in entries {
                // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
                unsafe {
                    validate_spawn_entry_with_ctx(self.host.api(), || self.resolve_ctx(), entry)?;
                }
                comps.push(entry.component);
                ptrs.push(entry.data_ptr);
            }
            // SAFETY: `comps` and `ptrs` have identical length and validated live payloads.
            unsafe {
                if persistent {
                    self.spawn_persistent_raw(&comps, &ptrs)
                } else {
                    self.spawn_raw(&comps, &ptrs)
                }
            }
        }
    }

    /// Reserves an entity and batch-inserts `entries` in a single archetype placement via
    /// `BamHostApi::cmd_spawn`, validating each safe [`SpawnEntry`]'s layout against
    /// `Host::component_layout_info`.
    #[inline]
    pub fn spawn(&mut self, entries: &[SpawnEntry<'_>]) -> Result<FfiEntity, FfiResult> {
        self.spawn_entries_impl(entries, false)
    }

    /// Raw batched entity spawn forwarding parallel `components` and `data_ptrs` slices to
    /// `BamHostApi::cmd_spawn`.
    ///
    /// # Safety
    /// For every `i` in `0..components.len()`, if the registered byte size of `components[i]` is
    /// non-zero, `data_ptrs[i]` must be non-null and readable for that registered byte size.
    pub unsafe fn spawn_raw(
        &mut self,
        components: &[BamComponent],
        data_ptrs: &[*const u8],
    ) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if components.len() != data_ptrs.len() {
            return Err(FfiResult::INVALID_ARGUMENT);
        }
        let Some(spawn_fn) = self.host.api().cmd_spawn else {
            return Err(FfiResult::NULL_POINTER);
        };
        let count = components.len();
        let comps_ptr = if count == 0 {
            core::ptr::null()
        } else {
            components.as_ptr()
        };
        let ptrs_ptr = if count == 0 {
            core::ptr::null()
        } else {
            data_ptrs.as_ptr()
        };
        let mut out = FfiEntity::NULL;
        // SAFETY: `self.raw` is valid, `comps_ptr`/`ptrs_ptr` cover `count` elements, and caller
        // guarantees each `data_ptrs[i]` matches `components[i]`'s registered size.
        let status = unsafe { spawn_fn(self.raw, comps_ptr, ptrs_ptr, count, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Queues deferred despawning of `entity`.
    #[inline]
    pub fn despawn(&mut self, entity: FfiEntity) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        let Some(despawn_fn) = self.host.api().cmd_despawn else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is an active `BamCommands` handle for `'a`.
        unsafe { despawn_fn(self.raw, entity) }
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) onto `entity` using pre-resolved
    /// `component` handle.
    #[inline]
    pub fn insert<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: &T,
    ) -> FfiResult {
        let data_ptr = core::ptr::from_ref(value).cast::<u8>();
        let data_len = core::mem::size_of::<T>();
        // SAFETY: `value` is a live `&T` (`T: AbiPod`) readable via raw byte copy for `size_of::<T>()`
        // bytes without forming `&[u8]` over any padding bytes.
        unsafe { self.insert_raw(entity, component, data_ptr, data_len) }
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) onto `entity` using an init-validated
    /// [`TypedComponent<T>`] handle without querying `Host::component_layout_info`.
    #[inline]
    pub fn insert_typed<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: TypedComponent<T>,
        value: &T,
    ) -> FfiResult {
        let data_ptr = core::ptr::from_ref(value).cast::<u8>();
        let data_len = core::mem::size_of::<T>();
        // SAFETY: `component` was validated against `T`'s layout and `value` is a live `&T`
        // (`T: AbiPod`) readable for `size_of::<T>()` bytes without forming `&[u8]` over padding.
        unsafe { self.insert_raw(entity, component.handle(), data_ptr, data_len) }
    }

    /// Queues deferred raw byte insertion (`data_ptr[0..data_len]`) for `component` onto `entity`.
    ///
    /// # Safety
    /// If `data_len > 0`, `data_ptr` must be non-null and readable for `data_len` bytes matching
    /// the registered component layout (and ownership invariants if `component` has a `drop_fn`).
    #[inline]
    pub unsafe fn insert_raw(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        data_ptr: *const u8,
        data_len: usize,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !component.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        if data_len > 0 && data_ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(insert_fn) = self.host.api().cmd_insert else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is active and caller guarantees `data_ptr` is readable for `data_len` bytes.
        unsafe { insert_fn(self.raw, entity, component, data_ptr, data_len) }
    }

    /// Queues deferred insertion of host-constructed `T::default()` for `component` onto `entity`.
    #[inline]
    pub fn insert_default(&mut self, entity: FfiEntity, component: BamComponent) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !component.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(insert_default_fn) = self.host.api().cmd_insert_default else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is active and `entity`/`component` were checked non-zero.
        unsafe { insert_default_fn(self.raw, entity, component) }
    }

    /// Queues deferred removal of `component` from `entity`.
    #[inline]
    pub fn remove(&mut self, entity: FfiEntity, component: BamComponent) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !component.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(remove_fn) = self.host.api().cmd_remove else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is active and `entity`/`component` were checked non-zero.
        unsafe { remove_fn(self.raw, entity, component) }
    }

    /// Queues deferred `world.trigger(...)` or `world.trigger_dynamic*` for pre-resolved `event`.
    ///
    /// Pass [`FfiEntity::NULL`] for global event triggers or a valid target [`FfiEntity`] for
    /// entity-targeted triggers.
    ///
    /// # Safety
    /// If `data_len > 0`, `data_ptr` must be non-null and point to `data_len` initialized readable bytes.
    pub unsafe fn trigger_raw(
        &mut self,
        event: BamComponent,
        target: FfiEntity,
        data_ptr: *const u8,
        data_len: usize,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !event.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        if data_len > 0 && data_ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(trigger_fn) = host_slot!(self.host.api(), cmd_trigger) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and caller guarantees `data_ptr` is readable for `data_len` bytes.
        unsafe { trigger_fn(self.raw, event, target, data_ptr, data_len) }
    }

    /// Queues a deferred event trigger of `value` (`T: AbiPod`) under pre-resolved handle `event`.
    #[inline]
    pub fn trigger<T: AbiPod>(
        &mut self,
        event: BamComponent,
        target: FfiEntity,
        value: &T,
    ) -> FfiResult {
        let data_ptr = if core::mem::size_of::<T>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            core::ptr::from_ref(value).cast::<u8>()
        };
        // SAFETY: `value` is `T: AbiPod` and valid for `size_of::<T>()` initialized bytes.
        unsafe { self.trigger_raw(event, target, data_ptr, core::mem::size_of::<T>()) }
    }

    /// Reserves an entity that survives mod unload (`BamHostApi::cmd_spawn_persistent`) and
    /// batch-inserts `entries`, validating each safe [`SpawnEntry`]'s layout against
    /// `Host::component_layout_info`.
    #[inline]
    pub fn spawn_persistent(&mut self, entries: &[SpawnEntry<'_>]) -> Result<FfiEntity, FfiResult> {
        self.spawn_entries_impl(entries, true)
    }

    /// Raw persistent batched entity spawn forwarding parallel `components` and `data_ptrs` slices
    /// to `BamHostApi::cmd_spawn_persistent`.
    ///
    /// # Safety
    /// For every `i` in `0..components.len()`, if the registered byte size of `components[i]` is
    /// non-zero, `data_ptrs[i]` must be non-null and readable for that registered byte size.
    pub unsafe fn spawn_persistent_raw(
        &mut self,
        components: &[BamComponent],
        data_ptrs: &[*const u8],
    ) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if components.len() != data_ptrs.len() {
            return Err(FfiResult::INVALID_ARGUMENT);
        }
        let spawn_fn =
            host_slot!(self.host.api(), cmd_spawn_persistent).ok_or(FfiResult::UNSUPPORTED)?;
        let count = components.len();
        let comps_ptr = if count == 0 {
            core::ptr::null()
        } else {
            components.as_ptr()
        };
        let ptrs_ptr = if count == 0 {
            core::ptr::null()
        } else {
            data_ptrs.as_ptr()
        };
        let mut out = FfiEntity::NULL;
        // SAFETY: `self.raw` is valid, `comps_ptr`/`ptrs_ptr` cover `count` elements, and caller
        // guarantees each `data_ptrs[i]` matches `components[i]`'s registered size.
        let status = unsafe { spawn_fn(self.raw, comps_ptr, ptrs_ptr, count, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Spawns `entity_count` entities in a single columnar batch via `BamHostApi::cmd_spawn_batch`,
    /// validating each safe [`SpawnBatchColumn`]'s layout against `Host::component_layout_info`
    /// and optionally writing the reserved [`FfiEntity`] handles into `out_entities`.
    pub fn spawn_batch_columns(
        &mut self,
        columns: &[SpawnBatchColumn<'_>],
        entity_count: usize,
        out_entities: Option<&mut [FfiEntity]>,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if let Some(ref out_slice) = out_entities
            && out_slice.len() < entity_count
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        for col in columns {
            // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
            let status = unsafe {
                validate_spawn_batch_column_with_ctx(
                    self.host.api(),
                    || self.resolve_ctx(),
                    col,
                    entity_count,
                )
            };
            if !status.is_ok() {
                return status;
            }
        }
        let out_ptr = match out_entities {
            Some(slice) if entity_count > 0 => slice.as_mut_ptr(),
            _ => core::ptr::null_mut(),
        };
        if columns.len() <= INLINE_SPAWN_CAP {
            let mut comps = [BamComponent::INVALID; INLINE_SPAWN_CAP];
            let mut ptrs = [core::ptr::null::<u8>(); INLINE_SPAWN_CAP];
            for (i, col) in columns.iter().enumerate() {
                comps[i] = col.component;
                ptrs[i] = col.column_ptr;
            }
            // SAFETY: Each `SpawnBatchColumn` verified `col.entity_count >= entity_count`, layout,
            // and valid readable column bytes.
            unsafe {
                self.spawn_batch_raw(
                    &comps[..columns.len()],
                    &ptrs[..columns.len()],
                    entity_count,
                    out_ptr,
                )
            }
        } else {
            let mut comps = Vec::with_capacity(columns.len());
            let mut ptrs = Vec::with_capacity(columns.len());
            for col in columns {
                comps.push(col.component);
                ptrs.push(col.column_ptr);
            }
            // SAFETY: `comps` and `ptrs` match `columns` and cover `entity_count` validated rows.
            unsafe { self.spawn_batch_raw(&comps, &ptrs, entity_count, out_ptr) }
        }
    }

    /// Raw columnar multi-entity spawn forwarding directly to `BamHostApi::cmd_spawn_batch`.
    ///
    /// # Safety
    /// - For every `i` in `0..components.len()`, if `entity_count > 0` and the registered size `S`
    ///   of `components[i]` is non-zero, `column_ptrs[i]` must be non-null and readable for
    ///   `entity_count * S` bytes.
    /// - If `out_entities` is non-null and `entity_count > 0`, it must be writable for
    ///   `entity_count` [`FfiEntity`] elements.
    pub unsafe fn spawn_batch_raw(
        &mut self,
        components: &[BamComponent],
        column_ptrs: &[*const u8],
        entity_count: usize,
        out_entities: *mut FfiEntity,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if components.len() != column_ptrs.len() {
            return FfiResult::INVALID_ARGUMENT;
        }
        let Some(batch_fn) = host_slot!(self.host.api(), cmd_spawn_batch) else {
            return FfiResult::UNSUPPORTED;
        };
        let comp_count = components.len();
        let comps_ptr = if comp_count == 0 {
            core::ptr::null()
        } else {
            components.as_ptr()
        };
        let cols_ptr = if comp_count == 0 {
            core::ptr::null()
        } else {
            column_ptrs.as_ptr()
        };
        // SAFETY: `self.raw` is valid and caller guarantees `column_ptrs` and `out_entities` bounds.
        unsafe {
            batch_fn(
                self.raw,
                comps_ptr,
                cols_ptr,
                comp_count,
                entity_count,
                out_entities,
            )
        }
    }

    /// Queues deferred bulk despawning of all entities matching filter-only `terms`
    /// (`FILTER_ONLY` / `WITHOUT` / `ALLOW`) via `BamHostApi::cmd_despawn_all`.
    pub fn despawn_all(&mut self, terms: &[BamTermDesc]) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if terms.is_empty() {
            return FfiResult::INVALID_ARGUMENT;
        }
        for term in terms {
            let status = term.validate();
            if !status.is_ok() {
                return status;
            }
        }
        let Some(despawn_all_fn) = host_slot!(self.host.api(), cmd_despawn_all) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and `terms` is a valid slice of validated `BamTermDesc`s.
        unsafe { despawn_all_fn(self.raw, terms.as_ptr(), terms.len()) }
    }

    /// Queues deferred conditional insertion of `value` (`T: AbiPod`) onto `entity` only if
    /// `component` is not already present (`BamHostApi::cmd_insert_if_new`).
    #[inline]
    pub fn insert_if_new<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: &T,
    ) -> FfiResult {
        let data_ptr = core::ptr::from_ref(value).cast::<u8>();
        let data_len = core::mem::size_of::<T>();
        // SAFETY: `value` is a live `&T` (`T: AbiPod`) readable for `size_of::<T>()` bytes.
        unsafe { self.insert_if_new_raw(entity, component, data_ptr, data_len) }
    }

    /// Queues deferred conditional insertion of `value` (`T: AbiPod`) onto `entity` only if
    /// `component` is not already present, using an init-validated [`TypedComponent<T>`] handle
    /// without querying `Host::component_layout_info`.
    #[inline]
    pub fn insert_if_new_typed<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: TypedComponent<T>,
        value: &T,
    ) -> FfiResult {
        let data_ptr = core::ptr::from_ref(value).cast::<u8>();
        let data_len = core::mem::size_of::<T>();
        // SAFETY: `component` was validated against `T`'s layout and `value` is a live `&T`
        // (`T: AbiPod`) readable for `size_of::<T>()` bytes.
        unsafe { self.insert_if_new_raw(entity, component.handle(), data_ptr, data_len) }
    }

    /// Raw deferred conditional insertion (`cmd_insert_if_new`).
    ///
    /// # Safety
    /// If `data_len > 0`, `data_ptr` must be non-null and readable for `data_len` bytes matching
    /// the registered component layout.
    pub unsafe fn insert_if_new_raw(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        data_ptr: *const u8,
        data_len: usize,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !component.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        if data_len > 0 && data_ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(insert_fn) = host_slot!(self.host.api(), cmd_insert_if_new) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and caller guarantees `data_ptr` covers `data_len` bytes.
        unsafe { insert_fn(self.raw, entity, component, data_ptr, data_len) }
    }

    /// Raw extensible trigger dispatch forwarding `desc` to `BamHostApi::cmd_trigger_ex`.
    ///
    /// # Safety
    /// `desc` must satisfy [`BamTriggerDesc::validate`] and its pointers (`data_ptr`,
    /// `watched_components`) must remain valid for the duration of this call.
    pub unsafe fn trigger_ex_raw(&mut self, desc: &BamTriggerDesc) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let validation = desc.validate();
        if !validation.is_ok() {
            return validation;
        }
        let Some(trigger_ex_fn) = host_slot!(self.host.api(), cmd_trigger_ex) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and `desc` passed validation per caller contract.
        unsafe { trigger_ex_fn(self.raw, desc) }
    }

    /// Queues an extended custom event trigger (`value: &T`) with optional `flags` (such as
    /// `BAM_TRIGGER_FLAG_PROPAGATE`) and `propagate_relationship`.
    pub fn trigger_ex<T: AbiPod>(
        &mut self,
        event: BamComponent,
        target: FfiEntity,
        value: &T,
        flags: u32,
        propagate_relationship: BamComponent,
    ) -> FfiResult {
        let mut copy = *value;
        let data_ptr = if core::mem::size_of::<T>() == 0 {
            core::ptr::null_mut()
        } else {
            core::ptr::from_mut(&mut copy).cast::<u8>()
        };
        let mut desc = BamTriggerDesc::new(event, data_ptr, core::mem::size_of::<T>())
            .with_entity(target)
            .with_flags(flags);
        if propagate_relationship.is_valid() {
            desc = desc.with_propagate(propagate_relationship);
        }
        // SAFETY: `copy` lives on the stack across `trigger_ex_raw`, which copies payload bytes
        // into the host command arena before returning.
        unsafe { self.trigger_ex_raw(&desc) }
    }

    /// Queues a synthetic lifecycle or `Ready` trigger via `BamHostApi::cmd_trigger_ex`.
    pub fn trigger_lifecycle(
        &mut self,
        kind: BamEventKind,
        target: FfiEntity,
        watched_components: &[BamComponent],
    ) -> FfiResult {
        let comps_ptr = if watched_components.is_empty() {
            core::ptr::null()
        } else {
            watched_components.as_ptr()
        };
        let desc = BamTriggerDesc::new(BamComponent::INVALID, core::ptr::null_mut(), 0)
            .with_event_kind(kind)
            .with_entity(target)
            .with_watched_components(comps_ptr, watched_components.len());
        // SAFETY: `watched_components` slice stays live on the stack for the duration of `trigger_ex_raw`.
        unsafe { self.trigger_ex_raw(&desc) }
    }

    /// Raw runtime observer spawn forwarding `desc` to `BamHostApi::cmd_observe` and returning
    /// the spawned observer [`FfiEntity`].
    ///
    /// # Safety
    /// `desc` must satisfy [`BamObserverDesc::validate`], and its callbacks, `user_data`, `name`,
    /// and `watched_components` must remain valid until the observer entity is despawned or the
    /// owning mod shuts down.
    pub unsafe fn observe_raw(&mut self, desc: &BamObserverDesc) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let validation = desc.validate();
        if !validation.is_ok() {
            return Err(validation);
        }
        let observe_fn = host_slot!(self.host.api(), cmd_observe).ok_or(FfiResult::UNSUPPORTED)?;
        let mut out = FfiEntity::NULL;
        // SAFETY: `self.raw` is active, `desc` passed validation, and `out` is writable stack storage.
        let status = unsafe { observe_fn(self.raw, desc, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Spawns a runtime read-only custom event observer via `BamHostApi::cmd_observe`, tracking
    /// its closure state inside the owning [`crate::GuestInstance`].
    #[allow(clippy::too_many_arguments)]
    pub fn observe<E, F>(
        &mut self,
        name: Option<&str>,
        event: BamComponent,
        target: FfiEntity,
        flags: u32,
        propagate_relationship: BamComponent,
        callback: F,
    ) -> Result<FfiEntity, FfiResult>
    where
        E: AbiPod,
        F: FnMut(FfiEntity, &E, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
    {
        let desc = allocate_runtime_observer_desc::<E, F>(
            self.host,
            name,
            event,
            target,
            flags,
            propagate_relationship,
            callback,
        )?;
        // SAFETY: `desc`'s closure state and optional `name` were pinned in `GuestInstance` via
        // `Host::track_runtime_allocation`.
        unsafe { self.observe_raw(&desc) }
    }

    /// Spawns a runtime mutable custom event observer (`&mut E` + `&mut bool` propagate) via
    /// `BamHostApi::cmd_observe`, tracking its closure state inside the owning [`crate::GuestInstance`].
    #[allow(clippy::too_many_arguments)]
    pub fn observe_mut<E, F>(
        &mut self,
        name: Option<&str>,
        event: BamComponent,
        target: FfiEntity,
        flags: u32,
        propagate_relationship: BamComponent,
        callback: F,
    ) -> Result<FfiEntity, FfiResult>
    where
        E: AbiPod,
        F: FnMut(FfiEntity, &mut E, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult
            + Send
            + 'static,
    {
        let desc = allocate_runtime_observer_mut_desc::<E, F>(
            self.host,
            name,
            event,
            target,
            flags,
            propagate_relationship,
            callback,
        )?;
        // SAFETY: `desc`'s closure state and optional `name` were pinned in `GuestInstance`.
        unsafe { self.observe_raw(&desc) }
    }

    /// Spawns a runtime lifecycle or `Ready` observer via `BamHostApi::cmd_observe`, tracking its
    /// closure state and watched component list inside the owning [`crate::GuestInstance`].
    #[allow(clippy::too_many_arguments)]
    pub fn observe_lifecycle<F>(
        &mut self,
        name: Option<&str>,
        event_kind: BamEventKind,
        watched_components: &[BamComponent],
        target: FfiEntity,
        flags: u32,
        propagate_relationship: BamComponent,
        callback: F,
    ) -> Result<FfiEntity, FfiResult>
    where
        F: FnMut(FfiEntity, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
    {
        let desc = allocate_runtime_lifecycle_observer_desc(
            self.host,
            name,
            event_kind,
            watched_components,
            target,
            flags,
            propagate_relationship,
            callback,
        )?;
        // SAFETY: `desc`'s closure state, `name`, and `watched_components` were pinned in `GuestInstance`.
        unsafe { self.observe_raw(&desc) }
    }

    /// Spawns a runtime context-aware custom event observer (`BamObserverCtxFn` +
    /// `BamObserverDesc::queries`) backed by `observer_ctx_slice_trampoline<N, E, F>` via
    /// `BamHostApi::cmd_observe`, tracking all owned descriptor buffers and closure state inside
    /// the owning [`crate::GuestInstance`].
    pub fn observe_ctx_slice<const N: usize, E, F>(
        &mut self,
        config: &ObserverCtxRegistrationConfig<'_>,
        callback: F,
    ) -> Result<FfiEntity, FfiResult>
    where
        E: AbiPod,
        F: FnMut(FfiEntity, &E, &mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
    {
        let desc =
            allocate_runtime_observer_ctx_slice_desc::<N, E, F>(self.host, config, callback)?;
        // SAFETY: `desc`'s closure state, `name`, `watched_components`, and `queries` were pinned in
        // `GuestInstance` via `Host::track_runtime_allocation`.
        unsafe { self.observe_raw(&desc) }
    }

    /// Spawns a BSN or dynamic scene instance from `scene_handle` (optionally parented under
    /// `parent` when `!parent.is_null()`) via `BamHostApi::cmd_spawn_scene`, returning the
    /// reserved scene root [`FfiEntity`].
    pub fn spawn_scene(
        &mut self,
        scene_handle: &FfiAssetHandle,
        parent: FfiEntity,
    ) -> Result<FfiEntity, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let handle_status = scene_handle.validate();
        if !handle_status.is_ok() {
            return Err(handle_status);
        }
        if scene_handle.is_none() {
            return Err(FfiResult::INVALID_ARGUMENT);
        }
        let spawn_scene_fn =
            host_slot!(self.host.api(), cmd_spawn_scene).ok_or(FfiResult::UNSUPPORTED)?;
        let mut out = FfiEntity::NULL;
        // SAFETY: `self.raw` is active and `scene_handle` / `&mut out` are aligned stack references.
        let status = unsafe { spawn_scene_fn(self.raw, scene_handle, parent, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Reborrows this command buffer for a shorter exclusive lifetime `'b` (used by scoped
    /// transactions without aliasing `&mut ValidatedCommands`).
    #[inline]
    pub const fn reborrow<'b>(&'b mut self) -> ValidatedCommands<'b> {
        ValidatedCommands {
            raw: self.raw,
            host: self.host,
            lazy_host: self.lazy_host,
        }
    }

    /// Captures an opaque command-buffer checkpoint token via `BamHostApi::cmd_checkpoint`
    /// (slot 63), returning [`FfiResult::UNSUPPORTED`] if the host does not support ABI `0.3.0`
    /// command checkpoints.
    #[inline]
    pub fn checkpoint(&mut self) -> Result<usize, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let cp_fn = host_slot!(self.host.api(), cmd_checkpoint).ok_or(FfiResult::UNSUPPORTED)?;
        let mut token = 0usize;
        // SAFETY: `self.raw` is an active `BamCommands` for `'a` and `&mut token` is writable stack storage.
        let status = unsafe { cp_fn(self.raw, &mut token) };
        if status.is_ok() {
            Ok(token)
        } else {
            Err(status)
        }
    }

    /// Rolls back all deferred commands queued after `checkpoint` via `BamHostApi::cmd_rollback`
    /// (slot 64), returning [`FfiResult::UNSUPPORTED`] if the host does not support ABI `0.3.0`
    /// command rollback.
    #[inline]
    pub fn rollback(&mut self, checkpoint: usize) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(rb_fn) = host_slot!(self.host.api(), cmd_rollback) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is an active `BamCommands` for `'a` under `&mut self`.
        unsafe { rb_fn(self.raw, checkpoint) }
    }

    /// Queues deferred insertion of a typed relationship edge `entity --[relation]--> target`
    /// carrying raw payload `data[0..data_size]` via `BamHostApi::cmd_insert_relation` (slot 65).
    ///
    /// # Safety
    /// If `data_size > 0`, `data` must be non-null and readable for `data_size` initialized bytes
    /// matching the registered payload layout of `relation`.
    pub unsafe fn insert_relation_raw(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
        data: *const c_void,
        data_size: usize,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() || target.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !relation.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        if data_size > 0 && data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(insert_rel_fn) = host_slot!(self.host.api(), cmd_insert_relation) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and caller guarantees `data` is readable for `data_size` bytes.
        unsafe { insert_rel_fn(self.raw, entity, relation, target, data, data_size) }
    }

    /// Queues deferred insertion of a marker/tag relationship edge `entity --[relation]--> target`
    /// (`data = null`, `data_size = 0`) via `BamHostApi::cmd_insert_relation`.
    #[inline]
    pub fn insert_relation(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> FfiResult {
        // SAFETY: `data_size == 0`, so `null` payload pointer is valid per `cmd_insert_relation` contract.
        unsafe { self.insert_relation_raw(entity, relation, target, core::ptr::null(), 0) }
    }

    /// Queues deferred insertion of a payload-carrying relationship edge
    /// `entity --[relation(payload)]--> target` (`T: AbiPod`) via `BamHostApi::cmd_insert_relation`.
    #[inline]
    pub fn insert_relation_with_payload<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
        payload: &T,
    ) -> FfiResult {
        let data_size = core::mem::size_of::<T>();
        let data_ptr = if data_size == 0 {
            core::ptr::null()
        } else {
            core::ptr::from_ref(payload).cast::<c_void>()
        };
        // SAFETY: `payload` is a live `&T` (`T: AbiPod`) readable for `size_of::<T>()` bytes.
        unsafe { self.insert_relation_raw(entity, relation, target, data_ptr, data_size) }
    }

    /// Queues deferred removal of a typed relationship edge `entity --[relation]--> target` via
    /// `BamHostApi::cmd_remove_relation` (slot 66).
    #[inline]
    pub fn remove_relation(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if entity.is_null() || target.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        if !relation.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(remove_rel_fn) = host_slot!(self.host.api(), cmd_remove_relation) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and `entity`/`target`/`relation` were validated non-null/valid.
        unsafe { remove_rel_fn(self.raw, entity, relation, target) }
    }

    /// Queues deferred cloning of all cloneable components from `src_entity` onto `dst_entity`
    /// via `BamHostApi::cmd_clone_entity` (slot 70).
    #[inline]
    pub fn clone_entity(&mut self, src_entity: FfiEntity, dst_entity: FfiEntity) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if src_entity.is_null() || dst_entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        let Some(clone_fn) = host_slot!(self.host.api(), cmd_clone_entity) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is an active `BamCommands` handle for `'a` and both entities were
        // verified non-null.
        unsafe { clone_fn(self.raw, src_entity, dst_entity) }
    }

    /// Queues deferred insertion or replacement of singleton `resource` (`R: AbiPod`) via
    /// `BamHostApi::cmd_insert_resource` (slot 71), validating `resource` layout against
    /// `Host::component_layout_info`.
    #[inline]
    pub fn insert_resource<R: AbiPod>(&mut self, resource: BamComponent, value: &R) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !resource.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(insert_res_fn) = host_slot!(self.host.api(), cmd_insert_resource) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.resolve_ctx()` returns a valid active `BamHostCtx` matching `self.host.api()`.
        let info = match unsafe {
            Host::component_layout_info_with_ctx(self.host.api(), self.resolve_ctx(), resource)
        } {
            Ok(info) => info,
            Err(err) => return err,
        };
        if info.size != core::mem::size_of::<R>() || info.align != core::mem::align_of::<R>() {
            return FfiResult::TYPE_MISMATCH;
        }
        let data_len = core::mem::size_of::<R>();
        let data_ptr = if data_len == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            core::ptr::from_ref(value).cast::<u8>()
        };
        // SAFETY: `self.raw` is active, `component_layout_info` verified `R`'s exact size and
        // alignment, and `value` is a live `&R` (`R: AbiPod`) readable for `data_len` bytes.
        unsafe { insert_res_fn(self.raw, resource, data_ptr, data_len) }
    }

    /// Queues deferred insertion or replacement of singleton `resource` using an init-validated
    /// [`TypedComponent<R>`] handle via `BamHostApi::cmd_insert_resource` (slot 71), skipping
    /// the per-command `Host::component_layout_info` FFI round-trip.
    #[inline]
    pub fn insert_resource_typed<R: AbiPod>(
        &mut self,
        resource: TypedComponent<R>,
        value: &R,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let handle = resource.handle();
        if !handle.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(insert_res_fn) = host_slot!(self.host.api(), cmd_insert_resource) else {
            return FfiResult::UNSUPPORTED;
        };
        let data_len = core::mem::size_of::<R>();
        let data_ptr = if data_len == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            core::ptr::from_ref(value).cast::<u8>()
        };
        // SAFETY: `self.raw` is active, `TypedComponent<R>` guarantees `handle` matches `R`'s
        // registered layout, and `value` is a live `&R` (`R: AbiPod`) readable for `data_len` bytes.
        unsafe { insert_res_fn(self.raw, handle, data_ptr, data_len) }
    }

    /// Queues deferred removal of singleton `resource` from the `World` via
    /// `BamHostApi::cmd_remove_resource` (slot 72).
    #[inline]
    pub fn remove_resource(&mut self, resource: BamComponent) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !resource.is_valid() {
            return FfiResult::INVALID_COMPONENT;
        }
        let Some(remove_res_fn) = host_slot!(self.host.api(), cmd_remove_resource) else {
            return FfiResult::UNSUPPORTED;
        };
        // SAFETY: `self.raw` is active and `resource` was validated non-zero.
        unsafe { remove_res_fn(self.raw, resource) }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        ffi::{c_char, c_void},
        mem::{MaybeUninit, align_of, size_of},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use bamabi_sys::{
        ABI_VERSION, BamChangeTicks, BamColumnInfo, BamCommands, BamComponent, BamHostApi,
        BamHostCtx, BamIter, BamLayoutInfo, BamLoadCtx, BamSystemCtx, BamSystemDesc, ComponentDesc,
        ComponentUuid, FfiBool, FfiEntity, FfiResult,
    };

    use super::*;
    use crate::{GuestInstance, allocate_system_slice_trampoline};

    struct BatchMockCounters {
        sys_host_ctx_calls: AtomicUsize,
        layout_info_calls: AtomicUsize,
        spawn_batch_calls: AtomicUsize,
        last_entity_count: AtomicUsize,
        last_comp_count: AtomicUsize,
    }

    unsafe extern "C" fn mock_get_proc(_: *const BamHostCtx, _: *const c_char) -> *const c_void {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_last_error(_: *const BamHostCtx, _: *mut c_char, _: usize) -> usize {
        0
    }
    unsafe extern "C" fn mock_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
        load_ctx.cast::<BamHostCtx>()
    }
    unsafe extern "C" fn mock_reg_comp(
        _: *mut BamLoadCtx,
        _: *const ComponentDesc,
        _: *mut BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_res_comp(
        _: *const BamHostCtx,
        _: *const c_char,
        _: *const ComponentUuid,
        _: *mut BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_reg_sys(_: *mut BamLoadCtx, _: *const BamSystemDesc) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        // SAFETY: `sys_ctx` points to `BatchMockCounters`.
        let st = unsafe { &*sys_ctx.cast::<BatchMockCounters>() };
        st.sys_host_ctx_calls.fetch_add(1, Ordering::Relaxed);
        sys_ctx.cast::<BamHostCtx>()
    }
    unsafe extern "C" fn mock_sys_query(
        _: *mut BamSystemCtx,
        _: usize,
        _: *mut *mut BamIter,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_sys_commands(
        sys_ctx: *mut BamSystemCtx,
        out_cmds: *mut *mut BamCommands,
    ) -> FfiResult {
        // SAFETY: `out_cmds` is writable stack storage and `sys_ctx` points to `BatchMockCounters`.
        unsafe { *out_cmds = sys_ctx.cast::<BamCommands>() };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_next_chunk(_: *mut BamIter, _: *mut usize) -> FfiBool {
        FfiBool::FALSE
    }
    unsafe extern "C" fn mock_iter_reset(_: *mut BamIter) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_iter_entities(_: *const BamIter) -> *const FfiEntity {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_col_ro(_: *const BamIter, _: usize) -> *const u8 {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_col_mut(_: *mut BamIter, _: usize) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn mock_mark(_: *mut BamIter, _: usize, _: usize, _: usize) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_rows_ro(_: *mut BamIter, _: usize) -> *const *const u8 {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_rows_mut(_: *mut BamIter, _: usize) -> *const *mut u8 {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_col_info(
        _: *const BamIter,
        _: usize,
        _: *mut BamColumnInfo,
    ) -> FfiResult {
        FfiResult::NOT_FOUND
    }
    unsafe extern "C" fn mock_res_ptr(_: *const BamIter, _: usize) -> *const u8 {
        core::ptr::null()
    }
    unsafe extern "C" fn mock_res_mut(_: *mut BamIter, _: usize) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn mock_spawn_empty(_: *mut BamCommands, _: *mut FfiEntity) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_spawn(
        _: *mut BamCommands,
        _: *const BamComponent,
        _: *const *const u8,
        _: usize,
        _: *mut FfiEntity,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_despawn(_: *mut BamCommands, _: FfiEntity) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_insert(
        _: *mut BamCommands,
        _: FfiEntity,
        _: BamComponent,
        _: *const u8,
        _: usize,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_insert_default(
        _: *mut BamCommands,
        _: FfiEntity,
        _: BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn mock_remove(
        _: *mut BamCommands,
        _: FfiEntity,
        _: BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }

    /// Unused helper verifying [`BamChangeTicks`] stays imported cleanly if needed.
    const _: usize = size_of::<BamChangeTicks>();

    #[test]
    fn spawn_batch_column_typed_skips_ffi_checks_and_enforces_local_bounds() {
        unsafe extern "C" fn mock_layout_info(
            ctx: *const BamHostCtx,
            comp: BamComponent,
            out: *mut BamLayoutInfo,
        ) -> FfiResult {
            // SAFETY: `ctx` points to `BatchMockCounters`.
            let st = unsafe { &*ctx.cast::<BatchMockCounters>() };
            st.layout_info_calls.fetch_add(1, Ordering::Relaxed);
            let info = if comp == BamComponent::from_raw(2) {
                BamLayoutInfo {
                    size: 0,
                    align: 1,
                    uuid: ComponentUuid::from_bytes([2; 16]),
                    ..BamLayoutInfo::empty()
                }
            } else {
                BamLayoutInfo {
                    size: size_of::<u64>(),
                    align: align_of::<u64>(),
                    uuid: ComponentUuid::from_bytes([1; 16]),
                    ..BamLayoutInfo::empty()
                }
            };
            // SAFETY: `out` is writable stack storage.
            unsafe { *out = info };
            FfiResult::OK
        }

        unsafe extern "C" fn mock_spawn_batch(
            cmds: *mut BamCommands,
            _comps: *const BamComponent,
            _col_ptrs: *const *const u8,
            comp_count: usize,
            entity_count: usize,
            out_entities: *mut FfiEntity,
        ) -> FfiResult {
            // SAFETY: `cmds` points to `BatchMockCounters`.
            let st = unsafe { &*cmds.cast::<BatchMockCounters>() };
            st.spawn_batch_calls.fetch_add(1, Ordering::Relaxed);
            st.last_comp_count.store(comp_count, Ordering::Relaxed);
            st.last_entity_count.store(entity_count, Ordering::Relaxed);
            if !out_entities.is_null() {
                for i in 0..entity_count {
                    // SAFETY: Caller supplies `out_entities` with at least `entity_count` slots.
                    unsafe { *out_entities.add(i) = FfiEntity::from_bits((i + 100) as u64) };
                }
            }
            FfiResult::OK
        }

        // SAFETY: `BamHostApi` is `#[repr(C)]` of `u32` and `Option<fn>` slots.
        let mut api: BamHostApi = unsafe { MaybeUninit::zeroed().assume_init() };
        api.struct_size = size_of::<BamHostApi>() as u32;
        api.abi_version = ABI_VERSION;
        api.get_proc_address = Some(mock_get_proc);
        api.last_error = Some(mock_last_error);
        api.load_host_ctx = Some(mock_load_host_ctx);
        api.register_component = Some(mock_reg_comp);
        api.resolve_component = Some(mock_res_comp);
        api.register_system = Some(mock_reg_sys);
        api.sys_host_ctx = Some(mock_sys_host_ctx);
        api.sys_query = Some(mock_sys_query);
        api.sys_commands = Some(mock_sys_commands);
        api.iter_next_chunk = Some(mock_next_chunk);
        api.iter_reset = Some(mock_iter_reset);
        api.iter_entities = Some(mock_iter_entities);
        api.iter_column_readonly = Some(mock_col_ro);
        api.iter_column_mutable = Some(mock_col_mut);
        api.iter_column_mutable_untracked = Some(mock_col_mut);
        api.iter_mark_changed_range = Some(mock_mark);
        api.iter_column_rows_readonly = Some(mock_rows_ro);
        api.iter_column_rows_mutable = Some(mock_rows_mut);
        api.iter_column_info = Some(mock_col_info);
        api.res_ptr = Some(mock_res_ptr);
        api.res_mut_ptr = Some(mock_res_mut);
        api.cmd_spawn_empty = Some(mock_spawn_empty);
        api.cmd_spawn = Some(mock_spawn);
        api.cmd_despawn = Some(mock_despawn);
        api.cmd_insert = Some(mock_insert);
        api.cmd_insert_default = Some(mock_insert_default);
        api.cmd_remove = Some(mock_remove);
        api.component_layout_info = Some(mock_layout_info);
        api.cmd_spawn_batch = Some(mock_spawn_batch);

        let counters = BatchMockCounters {
            sys_host_ctx_calls: AtomicUsize::new(0),
            layout_info_calls: AtomicUsize::new(0),
            spawn_batch_calls: AtomicUsize::new(0),
            last_entity_count: AtomicUsize::new(0),
            last_comp_count: AtomicUsize::new(0),
        };

        // SAFETY: `api` and `&counters` stay alive for the test.
        let init_host = unsafe {
            Host::from_raw(
                &raw const api,
                core::ptr::from_ref(&counters).cast::<BamHostCtx>(),
            )
        }
        .unwrap();
        let mut instance = GuestInstance::new(init_host, "typed_batch_mod");

        // Init-time TypedComponent validation (2 layout_info calls).
        let typed_u64 =
            TypedComponent::<u64>::validate(instance.host(), BamComponent::from_raw(1)).unwrap();
        let typed_zst =
            TypedComponent::<[u8; 0]>::validate(instance.host(), BamComponent::from_raw(2))
                .unwrap();
        assert_eq!(counters.layout_info_calls.load(Ordering::Relaxed), 2);
        counters.layout_info_calls.store(0, Ordering::Relaxed);

        let (sys_fn, sys_ud) =
            allocate_system_slice_trampoline::<0, _>(&mut instance, 0, 0, None, move |inv| {
                let mut cmds = inv.take_lazy_commands().resolve();
                let vals = [11u64, 22u64, 33u64];
                let zsts: [[u8; 0]; 3] = [[], [], []];
                let mut out = [FfiEntity::NULL; 3];

                // 1. Valid POD + ZST typed batch spawns 3 entities with 0 layout/sys_host_ctx calls.
                let st = cmds.spawn_batch_columns(
                    &[
                        SpawnBatchColumn::typed(typed_u64, &vals),
                        SpawnBatchColumn::typed(typed_zst, &zsts),
                    ],
                    3,
                    Some(&mut out),
                );
                assert_eq!(st, FfiResult::OK);
                assert_eq!(
                    out,
                    [
                        FfiEntity::from_bits(100),
                        FfiEntity::from_bits(101),
                        FfiEntity::from_bits(102),
                    ]
                );

                // 2. Empty slice (`entity_count == 0`) with `SpawnBatchColumn::typed` succeeds.
                let empty_u64: [u64; 0] = [];
                assert_eq!(
                    cmds.spawn_batch_columns(
                        &[SpawnBatchColumn::typed(typed_u64, &empty_u64)],
                        0,
                        None,
                    ),
                    FfiResult::OK
                );

                // 3. Under-sized column slice (`col.entity_count < entity_count`) -> INVALID_ARGUMENT.
                assert_eq!(
                    cmds.spawn_batch_columns(
                        &[SpawnBatchColumn::typed(typed_u64, &vals)],
                        4,
                        None,
                    ),
                    FfiResult::INVALID_ARGUMENT
                );

                // 4. Invalid component handle wrapped unchecked -> INVALID_COMPONENT.
                // SAFETY: Intentionally testing invalid-handle fail-closed guard.
                let bad_typed =
                    unsafe { TypedComponent::<u64>::from_raw_unchecked(BamComponent::INVALID) };
                assert_eq!(
                    cmds.spawn_batch_columns(&[SpawnBatchColumn::typed(bad_typed, &vals)], 3, None),
                    FfiResult::INVALID_COMPONENT
                );

                // 5. Synthetic null non-empty typed column -> NULL_POINTER with 0 FFI calls.
                // SAFETY: Synthetic null pointer is rejected before dereferencing.
                let null_typed = unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        typed_u64.handle(),
                        core::ptr::null(),
                        2,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        true,
                    )
                };
                assert_eq!(
                    cmds.spawn_batch_columns(&[null_typed], 2, None),
                    FfiResult::NULL_POINTER
                );

                // 6. Synthetic `isize::MAX` byte-length overflow on typed column -> OUT_OF_RANGE.
                let over_half = (isize::MAX as usize / size_of::<u64>()) + 1;
                // SAFETY: Overflow check rejects before `cmd_spawn_batch` or dereference.
                let overflow_typed = unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        typed_u64.handle(),
                        vals.as_ptr().cast::<u8>(),
                        over_half,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        true,
                    )
                };
                assert_eq!(
                    cmds.spawn_batch_columns(&[overflow_typed], over_half, None),
                    FfiResult::OUT_OF_RANGE
                );

                // 7. Synthetic `checked_mul` wrap on typed column -> OUT_OF_RANGE.
                let wrap_count = usize::MAX / 2;
                // SAFETY: Overflow check rejects before `cmd_spawn_batch` or dereference.
                let wrap_typed = unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        typed_u64.handle(),
                        vals.as_ptr().cast::<u8>(),
                        wrap_count,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        true,
                    )
                };
                assert_eq!(
                    cmds.spawn_batch_columns(&[wrap_typed], wrap_count, None),
                    FfiResult::OUT_OF_RANGE
                );

                FfiResult::OK
            });

        let sys_ctx = core::ptr::from_ref(&counters)
            .cast_mut()
            .cast::<BamSystemCtx>();
        // SAFETY: `sys_ctx` and `sys_ud` remain valid for this synchronous call.
        assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
        assert_eq!(counters.layout_info_calls.load(Ordering::Relaxed), 0);
        assert_eq!(counters.sys_host_ctx_calls.load(Ordering::Relaxed), 0);
        assert_eq!(counters.spawn_batch_calls.load(Ordering::Relaxed), 2);
        assert_eq!(counters.last_comp_count.load(Ordering::Relaxed), 1);
        assert_eq!(counters.last_entity_count.load(Ordering::Relaxed), 0);

        // Contrast: `SpawnBatchColumn::new` DOES invoke `sys_host_ctx` and `component_layout_info`.
        let (untyped_fn, untyped_ud) =
            allocate_system_slice_trampoline::<0, _>(&mut instance, 0, 0, None, move |inv| {
                let mut cmds = inv.take_lazy_commands().resolve();
                let vals = [1u64, 2u64];
                cmds.spawn_batch_columns(
                    &[SpawnBatchColumn::new(BamComponent::from_raw(1), &vals)],
                    2,
                    None,
                )
            });
        // SAFETY: `sys_ctx` and `untyped_ud` remain valid for this synchronous call.
        assert_eq!(unsafe { untyped_fn(sys_ctx, untyped_ud) }, FfiResult::OK);
        assert_eq!(counters.layout_info_calls.load(Ordering::Relaxed), 1);
        assert_eq!(counters.sys_host_ctx_calls.load(Ordering::Relaxed), 1);
        assert_eq!(counters.spawn_batch_calls.load(Ordering::Relaxed), 3);
    }
}
