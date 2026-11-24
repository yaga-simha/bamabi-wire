//! Opaque query iterator handle, column/resource introspection, and iteration FFI function types.

use core::{
    ffi::c_void,
    marker::{PhantomData, PhantomPinned},
};

use crate::{
    access::AccessMode,
    commands::BamCommands,
    primitives::{BamComponent, ComponentUuid, FfiBool, FfiEntity, FfiResult},
};

/// Opaque C handle representing an active query chunk and singleton resource accessor (`typedef struct BamIter BamIter;`).
#[repr(C)]
pub struct BamIter {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// Column or component uses `StorageType::SparseSet` rather than dense `Table` storage.
pub const BAM_STORAGE_FLAG_SPARSE_SET: u32 = 1 << 0;

/// Column or component is registered as immutable (mutable access is rejected).
pub const BAM_STORAGE_FLAG_IMMUTABLE: u32 = 1 << 1;

/// Term refers to a singleton Bevy `Resource` rather than a per-entity component column.
pub const BAM_STORAGE_FLAG_RESOURCE: u32 = 1 << 2;

/// Term refers to a buffered Bevy `Messages<M>` stream.
pub const BAM_STORAGE_FLAG_MESSAGE: u32 = 1 << 3;

/// Registered type is a triggerable Bevy `Event` (`world.trigger` / dynamic observer).
pub const BAM_STORAGE_FLAG_EVENT: u32 = 1 << 4;

/// Registered type is an entity-targeted Bevy `EntityEvent`.
pub const BAM_STORAGE_FLAG_ENTITY_EVENT: u32 = 1 << 5;

/// Dense `Table` column maintains a Bevy 0.20 per-table `summary_tick` for fast tick-filter skipping.
pub const BAM_STORAGE_FLAG_SUMMARY_TICK: u32 = 1 << 6;

/// Maximum change-tick relative age matching `bevy_ecs::change_detection::MAX_CHANGE_AGE`.
pub const BAM_MAX_CHANGE_AGE: u32 = u32::MAX - (2 * 518_400_000 - 1);

/// Snapshot of Bevy `ComponentTicks` (`added`, `changed`) alongside the enclosing system's
/// `(last_run, this_run)` tick window (`typedef struct BamChangeTicks BamChangeTicks;`).
///
/// Layout: `16` bytes, `4`-byte alignment, zero implicit padding on 32-bit and 64-bit targets.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct BamChangeTicks {
    /// Tick when the resource or component was added (`offset 0`).
    pub added: u32,
    /// Tick when the resource or component was last changed (`offset 4`).
    pub changed: u32,
    /// Calling system's `last_run` tick (`offset 8`).
    pub last_run: u32,
    /// Calling system's `this_run` tick (`offset 12`).
    pub this_run: u32,
}

impl BamChangeTicks {
    /// Constructs a [`BamChangeTicks`] snapshot from explicit `(added, changed, last_run, this_run)` ticks.
    #[inline]
    #[must_use]
    pub const fn new(added: u32, changed: u32, last_run: u32, this_run: u32) -> Self {
        Self {
            added,
            changed,
            last_run,
            this_run,
        }
    }

    /// Evaluates wraparound-safe Bevy change detection: returns `true` iff `tick` occurred strictly
    /// after `last_run` relative to `this_run`, clamping ages at [`BAM_MAX_CHANGE_AGE`].
    #[inline]
    pub const fn is_tick_newer(tick: u32, last_run: u32, this_run: u32) -> bool {
        let ticks_since_insert = this_run.wrapping_sub(tick);
        let clamped_insert = if ticks_since_insert < BAM_MAX_CHANGE_AGE {
            ticks_since_insert
        } else {
            BAM_MAX_CHANGE_AGE
        };
        let ticks_since_system = this_run.wrapping_sub(last_run);
        let clamped_system = if ticks_since_system < BAM_MAX_CHANGE_AGE {
            ticks_since_system
        } else {
            BAM_MAX_CHANGE_AGE
        };
        clamped_system > clamped_insert
    }

    /// Returns `true` if the resource or component was added since the system's `last_run` tick.
    #[inline]
    pub const fn is_added(&self) -> bool {
        Self::is_tick_newer(self.added, self.last_run, self.this_run)
    }

    /// Returns `true` if the resource or component was changed since the system's `last_run` tick.
    #[inline]
    pub const fn is_changed(&self) -> bool {
        Self::is_tick_newer(self.changed, self.last_run, self.this_run)
    }
}

const _: () = {
    assert!(core::mem::size_of::<BamChangeTicks>() == 16);
    assert!(core::mem::align_of::<BamChangeTicks>() == 4);
    assert!(core::mem::offset_of!(BamChangeTicks, added) == 0);
    assert!(core::mem::offset_of!(BamChangeTicks, changed) == 4);
    assert!(core::mem::offset_of!(BamChangeTicks, last_run) == 8);
    assert!(core::mem::offset_of!(BamChangeTicks, this_run) == 12);
};

/// Metadata returned by [`BamIterColumnInfoFn`] (`iter_column_info`) for guest type-confusion checks.
///
/// # Per-System Invariance Contract
/// For any registered system ([`BamSystemDesc`](crate::BamSystemDesc)), the [`BamColumnInfo`]
/// (`component`, `mode`, `uuid`, `size`, `align`, `storage_flags`) written by `iter_column_info`
/// for a given `(query_index, term_index)` is **invariant across all chunks and across all
/// [`BamSystemFn`](crate::BamSystemFn) ticks** for the entire lifetime of that registered system
/// (upheld across multi-mod reloads by [`BamMigrateComponentColumnFn`](crate::BamMigrateComponentColumnFn)'s
/// cross-mod `FfiResult::ACCESS_CONFLICT` exclusivity guard). Guests may therefore validate and
/// cache `BamColumnInfo` once per registered query slot.
///
/// Layout:
/// - 64-bit: `48` bytes, `8`-byte alignment, zero implicit padding.
/// - 32-bit: `40` bytes, `4`-byte alignment, zero implicit padding.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BamColumnInfo {
    /// Resolved 1-based [`BamComponent`] handle for this term (`offset 0`).
    pub component: BamComponent,
    /// Declared [`AccessMode`] for this term (`offset 4`).
    pub mode: AccessMode,
    /// Registered 128-bit [`ComponentUuid`] (`offset 8`, 16 bytes).
    pub uuid: ComponentUuid,
    /// Element byte size (`size_of::<T>()`, `offset 24`).
    pub size: usize,
    /// Element byte alignment (`align_of::<T>()`, `offset 32` on 64-bit / `28` on 32-bit).
    pub align: usize,
    /// Bitmask of `BAM_STORAGE_FLAG_*` (`offset 40` on 64-bit / `32` on 32-bit).
    pub storage_flags: u32,
    /// Reserved padding word (`0`, `offset 44` on 64-bit / `36` on 32-bit).
    pub reserved0: u32,
}

impl BamColumnInfo {
    /// Returns `true` if `BAM_STORAGE_FLAG_SPARSE_SET` is set.
    #[inline]
    pub const fn is_sparse_set(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_SPARSE_SET) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_IMMUTABLE` is set.
    #[inline]
    pub const fn is_immutable(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_IMMUTABLE) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_RESOURCE` is set.
    #[inline]
    pub const fn is_resource(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_RESOURCE) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_MESSAGE` is set.
    #[inline]
    pub const fn is_message(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_MESSAGE) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_EVENT` is set.
    #[inline]
    pub const fn is_event(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_EVENT) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_ENTITY_EVENT` is set.
    #[inline]
    pub const fn is_entity_event(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_ENTITY_EVENT) != 0
    }

    /// Returns `true` if `BAM_STORAGE_FLAG_SUMMARY_TICK` is set.
    #[inline]
    pub const fn is_summary_tick(&self) -> bool {
        (self.storage_flags & BAM_STORAGE_FLAG_SUMMARY_TICK) != 0
    }

    /// Returns `true` if `(uuid, size, align)` match the expected guest type metadata.
    #[inline]
    pub fn matches_layout(&self, expected_uuid: ComponentUuid, size: usize, align: usize) -> bool {
        self.uuid == expected_uuid && self.size == size && self.align == align
    }
}

const _: () = {
    #[cfg(target_pointer_width = "64")]
    {
        assert!(core::mem::size_of::<BamColumnInfo>() == 48);
        assert!(core::mem::align_of::<BamColumnInfo>() == 8);
        assert!(core::mem::offset_of!(BamColumnInfo, component) == 0);
        assert!(core::mem::offset_of!(BamColumnInfo, mode) == 4);
        assert!(core::mem::offset_of!(BamColumnInfo, uuid) == 8);
        assert!(core::mem::offset_of!(BamColumnInfo, size) == 24);
        assert!(core::mem::offset_of!(BamColumnInfo, align) == 32);
        assert!(core::mem::offset_of!(BamColumnInfo, storage_flags) == 40);
        assert!(core::mem::offset_of!(BamColumnInfo, reserved0) == 44);
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert!(core::mem::size_of::<BamColumnInfo>() == 40);
        assert!(core::mem::align_of::<BamColumnInfo>() == 4);
        assert!(core::mem::offset_of!(BamColumnInfo, component) == 0);
        assert!(core::mem::offset_of!(BamColumnInfo, mode) == 4);
        assert!(core::mem::offset_of!(BamColumnInfo, uuid) == 8);
        assert!(core::mem::offset_of!(BamColumnInfo, size) == 24);
        assert!(core::mem::offset_of!(BamColumnInfo, align) == 28);
        assert!(core::mem::offset_of!(BamColumnInfo, storage_flags) == 32);
        assert!(core::mem::offset_of!(BamColumnInfo, reserved0) == 36);
    }
};

/// Advances `iter` to the next non-empty chunk, writing the chunk length into `*out_len` and
/// returning [`FfiBool::TRUE`] while chunks remain or [`FfiBool::FALSE`] when exhausted.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - `out_len` must be a valid writable pointer to a `usize` (or null, which returns [`FfiBool::FALSE`]).
pub type BamIterNextChunkFn =
    unsafe extern "C" fn(iter: *mut BamIter, out_len: *mut usize) -> FfiBool;

/// Advances `iter` to the next non-empty chunk and populates `*out_len`, `*out_entities` (when
/// `out_entities` is non-null), and `out_col_ptrs[0..col_count]` in a single FFI call.
///
/// For each `term_index` in `0..col_count`:
/// - Dense `READ` / `CHANGED` / `ADDED` / `HAS` / `OPTIONAL_READ`: writes the read-only byte
///   pointer (or null if absent on `OPTIONAL_READ`, or if sparse-set/non-column).
/// - Dense `WRITE` / `OPTIONAL_WRITE`: writes the mutable byte pointer (or null if absent on
///   `OPTIONAL_WRITE`), marking the chunk's change ticks iff `(mark_changed_mask & (1u32 << term_index)) != 0`.
/// - Pure filters (`FILTER_ONLY`, `WITHOUT`, `ALLOW`), `RESOURCE_*`, `MESSAGE_*`, `REMOVED_READ`,
///   or `SparseSet` columns: writes null.
///
/// Returns [`FfiBool::TRUE`] while chunks remain or [`FfiBool::FALSE`] when exhausted.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - `out_len` must be a valid writable pointer to `usize` (or null, returning `FfiBool::FALSE`).
/// - If `out_entities` is non-null, it must point to writable `*const FfiEntity` storage.
/// - If `col_count > 0`, `out_col_ptrs` must point to `col_count` writable `*mut u8` slots.
pub type BamIterNextChunkBatchFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    out_len: *mut usize,
    out_entities: *mut *const FfiEntity,
    out_col_ptrs: *mut *mut u8,
    col_count: usize,
    mark_changed_mask: u32,
) -> FfiBool;

/// Resets `iter` back to its initial chunk so nested query loops can re-traverse without allocation.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] belonging to the running system invocation.
pub type BamIterResetFn = unsafe extern "C" fn(iter: *mut BamIter) -> FfiResult;

/// Returns a pointer to the contiguous `[FfiEntity; chunk_len]` slice for the current chunk, or null.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk.
pub type BamIterEntitiesFn = unsafe extern "C" fn(iter: *const BamIter) -> *const FfiEntity;

/// Returns a read-only byte pointer to dense column `term_index` in the current chunk, or null
/// if out of bounds, absent on an optional term, sparse-set stored, or non-column filter/resource.
///
/// For [`AccessMode::HAS`](crate::AccessMode::HAS) terms on an active chunk, returns a non-null
/// pointer to a 1-byte [`crate::FfiBool`] (`1` when the component is present in the chunk's
/// archetype, `0` when absent) for both `Table` and `SparseSet` components without borrowing `&T`.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk.
pub type BamIterColumnReadonlyFn =
    unsafe extern "C" fn(iter: *const BamIter, term_index: usize) -> *const u8;

/// Returns an exclusive mutable byte pointer to dense column `term_index` in the current chunk,
/// marking all rows in the chunk as changed at most once per chunk.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk, and the caller must
/// not create aliasing references to the same column.
pub type BamIterColumnMutableFn =
    unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *mut u8;

/// Returns an exclusive mutable byte pointer to dense column `term_index` in the current chunk
/// without automatically marking change ticks, for pairing with [`BamIterMarkChangedRangeFn`].
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk, and the caller must
/// not create aliasing references to the same column.
pub type BamIterColumnMutableUntrackedFn =
    unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *mut u8;

/// Marks rows `start_row..start_row + row_count` in writable column `term_index` as changed.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk.
pub type BamIterMarkChangedRangeFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    start_row: usize,
    row_count: usize,
) -> FfiResult;

/// Materializes and returns a pointer to `[ *const u8; chunk_len ]` of per-row read-only component
/// pointers for `StorageType::SparseSet` columns.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk.
pub type BamIterColumnRowsReadonlyFn =
    unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *const *const u8;

/// Materializes and returns a pointer to `[ *mut u8; chunk_len ]` of per-row mutable component
/// pointers for `StorageType::SparseSet` columns, marking matched rows changed.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk, and the caller must
/// not create aliasing mutable borrows across rows.
pub type BamIterColumnRowsMutableFn =
    unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *const *mut u8;

/// Writes type, layout, and storage metadata for `term_index` (entity column or resource term)
/// into `*out_info`.
///
/// # Per-System Invariance Contract
/// For any registered system ([`BamSystemDesc`](crate::BamSystemDesc)), the [`BamColumnInfo`]
/// (`component`, `mode`, `uuid`, `size`, `align`, `storage_flags`) returned by `iter_column_info`
/// for a given `(query_index, term_index)` is **invariant across all chunks and across all
/// [`BamSystemFn`](crate::BamSystemFn) ticks** for the lifetime of that registered system.
///
/// Consequently, guests (including `bamabi_guest_sys::QuerySlotScratch` and raw pointer casts in
/// C99/C++17/Zig guests) may validate and cache `BamColumnInfo` once per registered query slot.
/// Hosts **MUST NOT** mutate any observable `BamColumnInfo` property mid-lifetime; see
/// [`BamMigrateComponentColumnFn`](crate::BamMigrateComponentColumnFn) for the cross-mod
/// `FfiResult::ACCESS_CONFLICT` exclusivity rule during schema migrations.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`].
/// - `out_info` must point to writable [`BamColumnInfo`] storage.
pub type BamIterColumnInfoFn = unsafe extern "C" fn(
    iter: *const BamIter,
    term_index: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult;

/// Returns a read-only pointer to the singleton `Resource` declared at `term_index`, or null if
/// `term_index` is not a `RESOURCE_READ`/`RESOURCE_WRITE` term or the resource is absent in the `World`.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] belonging to the running system invocation.
pub type BamResPtrFn = unsafe extern "C" fn(iter: *const BamIter, term_index: usize) -> *const u8;

/// Returns an exclusive mutable pointer to the singleton `Resource` declared with `RESOURCE_WRITE`
/// at `term_index` (marking its Bevy change tick), or null if unauthorized or absent.
///
/// # Safety
/// `iter` must point to an active [`BamIter`] belonging to the running system invocation, and
/// the caller must not create aliasing references to the resource.
pub type BamResMutPtrFn = unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *mut u8;

/// Task callback invoked on worker threads by [`BamParForEachChunkFn`].
///
/// Each task receives an isolated sub-range [`BamIter`] whose `iter_next_chunk` yields only the
/// disjoint chunks assigned to that task.
///
/// # Safety
/// - `iter` is valid strictly for the duration of the task callback invocation on that worker thread.
/// - The callback must be thread-safe (`Sync` over shared `user_data`) and must never unwind across `extern "C"`.
pub type BamChunkTaskFn =
    unsafe extern "C" fn(iter: *mut BamIter, user_data: *mut c_void) -> FfiResult;

/// Partitions matched tables/archetype runs of `iter` across the host `ComputeTaskPool`, invoking
/// `chunk_fn(sub_iter, user_data)` on disjoint sub-range iterators in parallel.
///
/// # Safety
/// - `iter` must point to an active top-level [`BamIter`] belonging to the running system invocation.
/// - `chunk_fn` and `user_data` must be safe to invoke concurrently across worker threads.
pub type BamParForEachChunkFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    user_data: *mut c_void,
) -> FfiResult;

/// Reads up to `max_count` unread POD messages from the Bevy `Messages<M>` stream declared at
/// `term_index` (`MESSAGE_READ` or `MESSAGE_WRITE`) into `out_buf` (`max_count * size_of::<M>()`
/// writable bytes), advancing the running system's per-term message cursor and writing the count
/// of messages copied into `*out_read`.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - `out_read` must point to writable `usize` storage.
/// - When `max_count > 0` and element size `> 0`, `out_buf` must point to at least
///   `max_count * elem_size` writable bytes.
pub type BamMsgReadBatchFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult;

/// Writes one POD message payload (`data_ptr[0..data_len]`) into the Bevy `Messages<M>` stream
/// declared with `MESSAGE_WRITE` at `term_index`.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - When `data_len > 0`, `data_ptr` must point to `data_len` readable bytes representing a valid `M`.
pub type BamMsgSendFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult;

/// Writes `count` contiguous POD message payloads from `in_buf` (`count * size_of::<M>()` readable
/// bytes) into the Bevy `Messages<M>` stream declared with `MESSAGE_WRITE` at `term_index`.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - When `count > 0` and element size `> 0`, `in_buf` must point to `count * elem_size` readable bytes.
pub type BamMsgWriteBatchFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    in_buf: *const u8,
    count: usize,
) -> FfiResult;

/// Queries Bevy change-detection ticks (`added`, `changed`, `last_run`, `this_run`) for the
/// singleton `Resource` declared at `term_index` (`RESOURCE_READ` or `RESOURCE_WRITE`), writing
/// them into `*out_ticks`.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - `out_ticks` must point to writable [`BamChangeTicks`] storage.
pub type BamResTicksFn = unsafe extern "C" fn(
    iter: *const BamIter,
    term_index: usize,
    out_ticks: *mut BamChangeTicks,
) -> FfiResult;

/// Reads up to `max_count` removed-component [`FfiEntity`] identifiers from the lifecycle
/// `RemovedComponents<T>` stream declared with [`AccessMode::REMOVED_READ`] at `term_index` into
/// `out_entities[0..max_count]`, advancing the per-system removal cursor and writing the count of
/// entities copied into `*out_read`.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] belonging to the running system invocation.
/// - `out_read` must point to writable `usize` storage.
/// - When `max_count > 0`, `out_entities` must point to at least `max_count` writable [`FfiEntity`] slots.
pub type BamIterReadRemovedFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    out_entities: *mut FfiEntity,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult;

/// Retrieves per-row `added` and `changed` tick array pointers (`[u32; chunk_len]`) alongside the
/// enclosing system's `(last_run, this_run)` tick window for dense column `term_index` in the
/// current chunk.
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] positioned at a valid chunk.
/// - Non-null out-pointers (`out_added`, `out_changed`, `out_last_run`, `out_this_run`) must point
///   to valid writable storage of their respective types.
pub type BamIterColumnTicksFn = unsafe extern "C" fn(
    iter: *const BamIter,
    term_index: usize,
    out_added: *mut *const u32,
    out_changed: *mut *const u32,
    out_last_run: *mut u32,
    out_this_run: *mut u32,
) -> FfiResult;

/// Contiguous row range `[start_row .. start_row + row_count)` within the current table chunk
/// (`typedef struct BamRowRange BamRowRange;`).
///
/// Layout:
/// - 64-bit: `16` bytes, `8`-byte alignment, zero implicit padding.
/// - 32-bit: `8` bytes, `4`-byte alignment, zero implicit padding.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct BamRowRange {
    /// Zero-based starting row index inside the current chunk (`offset 0`).
    pub start_row: usize,
    /// Number of contiguous rows starting at `start_row` (`offset 8` / `4`).
    pub row_count: usize,
}

impl BamRowRange {
    /// Constructs a [`BamRowRange`] spanning `start_row .. start_row + row_count`.
    #[inline]
    #[must_use]
    pub const fn new(start_row: usize, row_count: usize) -> Self {
        Self {
            start_row,
            row_count,
        }
    }

    /// Returns the exclusive end row `start_row + row_count`, or `None` on overflow.
    #[inline]
    #[must_use]
    pub const fn end_row(&self) -> Option<usize> {
        self.start_row.checked_add(self.row_count)
    }

    /// Returns `true` if `row_count == 0`.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.row_count == 0
    }
}

// SAFETY: `BamRowRange` is a `#[repr(C)]` struct of two `usize` scalars with zero implicit padding,
// `Copy + Send + Sync + 'static`, no drop glue, and every bit pattern valid within the process.
unsafe impl crate::traits::RawInsertable for BamRowRange {}
// SAFETY: On each target ABI, `BamRowRange` has zero uninitialized padding bytes, no interior
// mutability, and matches C `{ size_t start_row; size_t row_count; }`.
unsafe impl crate::traits::AbiPod for BamRowRange {}

const _: () = {
    #[cfg(target_pointer_width = "64")]
    {
        assert!(core::mem::size_of::<BamRowRange>() == 16);
        assert!(core::mem::align_of::<BamRowRange>() == 8);
        assert!(core::mem::offset_of!(BamRowRange, start_row) == 0);
        assert!(core::mem::offset_of!(BamRowRange, row_count) == 8);
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert!(core::mem::size_of::<BamRowRange>() == 8);
        assert!(core::mem::align_of::<BamRowRange>() == 4);
        assert!(core::mem::offset_of!(BamRowRange, start_row) == 0);
        assert!(core::mem::offset_of!(BamRowRange, row_count) == 4);
    }
};

/// Zero-allocation `#![no_std]` run coalescer merging adjacent dirty row indices into
/// [`BamRowRange`] spans for batch change-tick marking (`BamHostApi::iter_mark_changed_ranges`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyRunCoalescer {
    current: Option<BamRowRange>,
}

impl DirtyRunCoalescer {
    /// Creates an empty [`DirtyRunCoalescer`].
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self { current: None }
    }

    /// Feeds one ascending row index into the coalescer.
    ///
    /// If `row` extends or falls within the active run, returns `None`. If `row` begins a new
    /// disjoint run, returns the completed previous [`BamRowRange`] and starts a new run at `row`.
    #[inline]
    pub fn push_row(&mut self, row: usize) -> Option<BamRowRange> {
        let Some(mut active) = self.current else {
            self.current = Some(BamRowRange::new(row, 1));
            return None;
        };
        if let Some(end) = active.end_row() {
            if row == end
                && let Some(next_count) = active.row_count.checked_add(1)
            {
                active.row_count = next_count;
                self.current = Some(active);
                return None;
            }
            if row >= active.start_row && row < end {
                return None;
            }
        }
        self.current = Some(BamRowRange::new(row, 1));
        Some(active)
    }

    /// Feeds a [`BamRowRange`] into the coalescer, merging adjacent/overlapping runs.
    ///
    /// Empty ranges (`row_count == 0`) are ignored. When `range` cannot merge with the active run,
    /// the prior run is returned and `range` becomes the new active run.
    #[inline]
    pub fn push_range(&mut self, range: BamRowRange) -> Option<BamRowRange> {
        if range.is_empty() {
            return None;
        }
        let Some(active) = self.current else {
            self.current = Some(range);
            return None;
        };
        if let (Some(a_end), Some(r_end)) = (active.end_row(), range.end_row())
            && range.start_row <= a_end
            && active.start_row <= r_end
        {
            let merged_start = active.start_row.min(range.start_row);
            let merged_end = a_end.max(r_end);
            self.current = Some(BamRowRange::new(merged_start, merged_end - merged_start));
            return None;
        }
        self.current = Some(range);
        Some(active)
    }

    /// Returns the currently buffered run without flushing it.
    #[inline]
    #[must_use]
    pub const fn current(&self) -> Option<BamRowRange> {
        self.current
    }

    /// Flushes and resets the final pending [`BamRowRange`], if any.
    #[inline]
    pub const fn finish(&mut self) -> Option<BamRowRange> {
        self.current.take()
    }

    /// Coalesces a slice of ascending row indices into caller-owned `out` storage without
    /// allocation, returning `Ok(written)` on success or `Err(total_runs)` if `out` was too small.
    pub fn coalesce_into(rows: &[usize], out: &mut [BamRowRange]) -> Result<usize, usize> {
        let mut state = Self::new();
        let mut total = 0usize;
        let mut overflow = false;
        for &row in rows {
            if let Some(done) = state.push_row(row) {
                if let Some(slot) = out.get_mut(total) {
                    *slot = done;
                } else {
                    overflow = true;
                }
                total += 1;
            }
        }
        if let Some(last) = state.finish() {
            if let Some(slot) = out.get_mut(total) {
                *slot = last;
            } else {
                overflow = true;
            }
            total += 1;
        }
        if overflow { Err(total) } else { Ok(total) }
    }
}

/// Extensible tuning parameters for `BamHostApi::par_for_each_chunk_ex`
/// (`typedef struct BamParOptions BamParOptions;`).
///
/// Layout:
/// - 64-bit: `24` bytes, `8`-byte alignment, zero implicit padding.
/// - 32-bit: `16` bytes, `4`-byte alignment, zero implicit padding.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BamParOptions {
    /// Size of this struct in bytes (`offset 0`).
    pub struct_size: u32,
    /// Bitflags (`must be 0` in ABI `0.4`, `offset 4`).
    pub flags: u32,
    /// Preferred rows per worker sub-chunk (`0` uses host default, `offset 8`).
    pub chunk_size: usize,
    /// Maximum worker partitions (`0` uses task-pool default, `offset 16` / `12`).
    pub max_partitions: usize,
}

impl BamParOptions {
    /// Minimum valid size of [`BamParOptions`] in ABI `0.4` (`24` on 64-bit, `16` on 32-bit).
    pub const MIN_V0_4_SIZE: usize = core::mem::size_of::<Self>();

    /// Constructs default parallel chunk options stamped with `struct_size = size_of::<Self>()`.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            flags: 0,
            chunk_size: 0,
            max_partitions: 0,
        }
    }

    /// Sets `chunk_size` hint.
    #[inline]
    #[must_use]
    pub const fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Sets `max_partitions` limit.
    #[inline]
    #[must_use]
    pub const fn with_max_partitions(mut self, max_partitions: usize) -> Self {
        self.max_partitions = max_partitions;
        self
    }

    /// Validates `struct_size` and unknown `flags`.
    #[inline]
    #[must_use]
    pub const fn validate(&self) -> FfiResult {
        if (self.struct_size as usize) < Self::MIN_V0_4_SIZE {
            return FfiResult::INVALID_VERSION;
        }
        if self.flags != 0 {
            return FfiResult::UNSUPPORTED;
        }
        FfiResult::OK
    }
}

impl Default for BamParOptions {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: `BamParOptions` begins with `struct_size: u32` at offset 0 and contains only integer scalars.
unsafe impl crate::extensible::ExtensibleStruct for BamParOptions {
    const MIN_V0_2_SIZE: usize = Self::MIN_V0_4_SIZE;
}

const _: () = {
    #[cfg(target_pointer_width = "64")]
    {
        assert!(core::mem::size_of::<BamParOptions>() == 24);
        assert!(core::mem::align_of::<BamParOptions>() == 8);
        assert!(core::mem::offset_of!(BamParOptions, struct_size) == 0);
        assert!(core::mem::offset_of!(BamParOptions, flags) == 4);
        assert!(core::mem::offset_of!(BamParOptions, chunk_size) == 8);
        assert!(core::mem::offset_of!(BamParOptions, max_partitions) == 16);
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert!(core::mem::size_of::<BamParOptions>() == 16);
        assert!(core::mem::align_of::<BamParOptions>() == 4);
        assert!(core::mem::offset_of!(BamParOptions, struct_size) == 0);
        assert!(core::mem::offset_of!(BamParOptions, flags) == 4);
        assert!(core::mem::offset_of!(BamParOptions, chunk_size) == 8);
        assert!(core::mem::offset_of!(BamParOptions, max_partitions) == 12);
    }
};

/// Extended parallel chunk-task callback receiving a thread-local [`BamCommands`] queue and
/// `partition_index` (`typedef BamResult (*BamChunkTaskExFn)(BamIter*, BamCommands*, size_t, void*);`).
///
/// # Safety
/// - `iter` and `commands` point to host-managed partition handles valid only during the invocation.
/// - Must never unwind across the `extern "C"` boundary.
pub type BamChunkTaskExFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    commands: *mut BamCommands,
    partition_index: usize,
    user_data: *mut c_void,
) -> FfiResult;

/// Returns `[ *mut u8; chunk_len ]` row pointers for a mutable sparse-set column at `term_index`
/// without marking change ticks (`typedef uint8_t* const* (*BamIterColumnRowsMutableUntrackedFn)(BamIter*, size_t);`).
///
/// # Safety
/// `iter` must point to an active [`BamIter`] positioned at a valid chunk whose `term_index`
/// was declared with mutable access.
pub type BamIterColumnRowsMutableUntrackedFn =
    unsafe extern "C" fn(iter: *mut BamIter, term_index: usize) -> *const *mut u8;

/// Marks all [`BamRowRange`] spans in `ranges_ptr[0..range_count]` as changed on `term_index`
/// (`typedef BamResult (*BamIterMarkChangedRangesFn)(BamIter*, size_t, const BamRowRange*, size_t);`).
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] positioned at a valid chunk.
/// - When `range_count > 0`, `ranges_ptr` must point to `range_count` readable [`BamRowRange`] entries.
pub type BamIterMarkChangedRangesFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    ranges_ptr: *const BamRowRange,
    range_count: usize,
) -> FfiResult;

/// Invokes host-driven parallel chunk iteration with explicit [`BamParOptions`] and per-partition
/// `commands` support (`typedef BamResult (*BamParForEachChunkExFn)(BamIter*, BamChunkTaskExFn, void*, const BamParOptions*);`).
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] owning exclusive query access.
/// - `chunk_fn` must be thread-safe (`Send + Sync` captures in `user_data`).
/// - If `options` is non-null, it must point to a valid [`BamParOptions`].
pub type BamParForEachChunkExFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskExFn>,
    user_data: *mut c_void,
    options: *const BamParOptions,
) -> FfiResult;

/// Borrows the contiguous underlying `Messages<M>` read slice `(out_ptr, out_count)` in zero-copy
/// form (`typedef BamResult (*BamMsgReadSliceFn)(BamIter*, size_t, const uint8_t**, size_t*);`).
///
/// # Safety
/// - `iter` must point to an active [`BamIter`] with `MESSAGE_READ` at `term_index`.
/// - `out_ptr` and `out_count` must point to valid writable pointers.
pub type BamMsgReadSliceFn = unsafe extern "C" fn(
    iter: *mut BamIter,
    term_index: usize,
    out_ptr: *mut *const u8,
    out_count: *mut usize,
) -> FfiResult;
