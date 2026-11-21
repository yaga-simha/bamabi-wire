//! Layer-0 concrete C-ABI `0.3.0` (`0x0003_0000`) host world and 73-slot [`BamHostApi`] table.
//!
//! Provides zero-Bevy, zero-external-dependency dense SoA chunk storage, singleton resource
//! storage with [`BamChangeTicks`], a rollback-capable deferred [`CommandQueue`], symbol
//! interning, and strict ABI validation matching `bamabi_sys` and `include/bamabi.h`.

use core::{
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_void},
    mem::MaybeUninit,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering},
};
use std::{collections::HashMap, sync::Mutex};

use bamabi_sys::{
    ABI_VERSION, AbiPod, AccessMode, BAM_COMPONENT_FLAG_IMMUTABLE, BAM_COMPONENT_FLAG_SPARSE_SET,
    BAM_COMPONENT_FLAG_SUMMARY_TICK, BAM_STORAGE_FLAG_IMMUTABLE, BAM_STORAGE_FLAG_MESSAGE,
    BAM_STORAGE_FLAG_RESOURCE, BAM_STORAGE_FLAG_SPARSE_SET, BAM_STORAGE_FLAG_SUMMARY_TICK,
    BamChangeTicks, BamChunkTaskFn, BamColumnInfo, BamCommands, BamComponent, BamHostApi,
    BamHostCtx, BamIter, BamLayoutInfo, BamLoadCtx, BamObserverDesc, BamQueryDesc, BamRelationDesc,
    BamRowMigrateFn, BamRunIfFn, BamScheduleSetDesc, BamSymbol, BamSystem, BamSystemCtx,
    BamSystemDesc, BamSystemFn, BamSystemSet, BamTermDesc, BamTriggerDesc, ComponentDesc,
    ComponentDropFn, ComponentHookFn, ComponentUuid, FfiAssetHandle, FfiAssetLoadState, FfiBool,
    FfiEntity, FfiResult, MAX_QUERIES_PER_SYSTEM, RawInsertable, ScheduleId, StrView,
    read_descriptor,
};

/// Canonical component name for entity positions.
pub(crate) const POSITION_NAME: &str = "bench::Position";
/// Canonical component name for entity velocities.
pub(crate) const VELOCITY_NAME: &str = "bench::Velocity";
/// Canonical resource name for simulation parameters.
pub(crate) const SIM_PARAMS_NAME: &str = "bench::SimParams";

/// Deterministic 128-bit UUID for `"bench::Position"` (`size = 16`, `align = 4`, `schema_hash = 0`).
pub(crate) const POSITION_UUID: ComponentUuid = ComponentUuid::from_parts(POSITION_NAME, 16, 4, 0);
/// Deterministic 128-bit UUID for `"bench::Velocity"` (`size = 16`, `align = 4`, `schema_hash = 0`).
pub(crate) const VELOCITY_UUID: ComponentUuid = ComponentUuid::from_parts(VELOCITY_NAME, 16, 4, 0);
/// Deterministic 128-bit UUID for `"bench::SimParams"` (`size = 16`, `align = 4`, `schema_hash = 0`).
pub(crate) const SIM_PARAMS_UUID: ComponentUuid =
    ComponentUuid::from_parts(SIM_PARAMS_NAME, 16, 4, 0);

/// Fixed integration timestep shared across native baseline and all polyglot guests.
pub(crate) const BENCH_DT: f32 = 0.016;

/// Shared 4D POD position component (`"bench::Position"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub(crate) struct Position {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) z: f32,
    pub(crate) w: f32,
}

// SAFETY: `Position` is `#[repr(C)]`, `Copy + Send + Sync + 'static`, zero implicit padding, and no drop glue.
unsafe impl RawInsertable for Position {}
// SAFETY: `Position` consists solely of four `f32` fields with deterministic C layout.
unsafe impl AbiPod for Position {}

/// Shared 4D POD velocity component (`"bench::Velocity"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub(crate) struct Velocity {
    pub(crate) vx: f32,
    pub(crate) vy: f32,
    pub(crate) vz: f32,
    pub(crate) vw: f32,
}

// SAFETY: `Velocity` is `#[repr(C)]`, `Copy + Send + Sync + 'static`, zero implicit padding, and no drop glue.
unsafe impl RawInsertable for Velocity {}
// SAFETY: `Velocity` consists solely of four `f32` fields with deterministic C layout.
unsafe impl AbiPod for Velocity {}

/// Shared POD singleton simulation parameters resource (`"bench::SimParams"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub(crate) struct SimParams {
    pub(crate) dt: f32,
    pub(crate) damping: f32,
    pub(crate) tick: u32,
    pub(crate) _pad: u32,
}

// SAFETY: `SimParams` is `#[repr(C)]`, `Copy + Send + Sync + 'static`, zero implicit padding, and no drop glue.
unsafe impl RawInsertable for SimParams {}
// SAFETY: `SimParams` has fixed-width C primitives and explicit `_pad` with zero implicit padding.
unsafe impl AbiPod for SimParams {}

const _: () = {
    assert!(core::mem::size_of::<Position>() == 16);
    assert!(core::mem::align_of::<Position>() == 4);
    assert!(core::mem::offset_of!(Position, x) == 0);
    assert!(core::mem::offset_of!(Position, y) == 4);
    assert!(core::mem::offset_of!(Position, z) == 8);
    assert!(core::mem::offset_of!(Position, w) == 12);

    assert!(core::mem::size_of::<Velocity>() == 16);
    assert!(core::mem::align_of::<Velocity>() == 4);
    assert!(core::mem::offset_of!(Velocity, vx) == 0);
    assert!(core::mem::offset_of!(Velocity, vy) == 4);
    assert!(core::mem::offset_of!(Velocity, vz) == 8);
    assert!(core::mem::offset_of!(Velocity, vw) == 12);

    assert!(core::mem::size_of::<SimParams>() == 16);
    assert!(core::mem::align_of::<SimParams>() == 4);
    assert!(core::mem::offset_of!(SimParams, dt) == 0);
    assert!(core::mem::offset_of!(SimParams, damping) == 4);
    assert!(core::mem::offset_of!(SimParams, tick) == 8);
    assert!(core::mem::offset_of!(SimParams, _pad) == 12);
};

/// Static byte values returned by `iter_column_readonly` for `AccessMode::HAS` terms.
static FFI_BOOL_FALSE_BYTE: u8 = FfiBool::FALSE.0;
static FFI_BOOL_TRUE_BYTE: u8 = FfiBool::TRUE.0;

/// Metadata for a registered component, resource, or relation in [`HostWorld`].
#[derive(Clone)]
pub(crate) struct RegisteredComp {
    pub(crate) _handle: BamComponent,
    pub(crate) _name: String,
    pub(crate) uuid: ComponentUuid,
    pub(crate) schema_fingerprint: u64,
    pub(crate) size: usize,
    pub(crate) align: usize,
    pub(crate) flags: u32,
    pub(crate) is_resource: bool,
    pub(crate) _has_default: bool,
    pub(crate) _drop_fn: Option<ComponentDropFn>,
    pub(crate) _vtable_ptr: *const c_void,
    pub(crate) _on_add: Option<ComponentHookFn>,
    pub(crate) _on_remove: Option<ComponentHookFn>,
    pub(crate) _hook_user_data: *mut c_void,
}

/// Compact layout entry mirrored into [`CommandQueue`] for lock-free, aliasing-free command validation.
#[derive(Clone, Copy)]
pub(crate) struct CompCommandMeta {
    pub(crate) size: usize,
    pub(crate) has_default: bool,
}

/// Summary of committed and rolled-back operations recorded in [`CommandQueue`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CommandStats {
    pub(crate) committed_spawns: usize,
    pub(crate) committed_inserts: usize,
    pub(crate) checkpoints: u64,
    pub(crate) rollbacks: u64,
    pub(crate) rolled_back_ops: u64,
    pub(crate) last_insert_component: BamComponent,
    pub(crate) last_insert_len: usize,
}

/// Deferred command entry queued on [`CommandQueue`].
#[derive(Clone)]
pub(crate) enum RecordedCmd {
    SpawnEmpty {
        _entity: FfiEntity,
    },
    Spawn {
        _entity: FfiEntity,
        inline_inserts: usize,
    },
    SpawnBatch {
        entity_count: usize,
        component_count: usize,
    },
    Despawn {
        _entity: FfiEntity,
    },
    DespawnAll,
    Insert {
        _entity: FfiEntity,
        component: BamComponent,
        byte_len: usize,
        _inline_bytes: [u8; 32],
    },
    InsertIfNew {
        _entity: FfiEntity,
        component: BamComponent,
        byte_len: usize,
        _inline_bytes: [u8; 32],
    },
    InsertDefault {
        _entity: FfiEntity,
        _component: BamComponent,
    },
    Remove {
        _entity: FfiEntity,
        _component: BamComponent,
    },
    Trigger,
    Observe,
    InsertRelation,
    RemoveRelation,
    CloneEntity {
        _src: FfiEntity,
        _dst: FfiEntity,
    },
    InsertResource {
        _resource: BamComponent,
        _byte_len: usize,
    },
    RemoveResource {
        _resource: BamComponent,
    },
}

/// Deferred command queue backing `*mut BamCommands`.
pub(crate) struct CommandQueue {
    pub(crate) ops: Vec<RecordedCmd>,
    pub(crate) comp_meta: Vec<CompCommandMeta>,
    pub(crate) checkpoints: u64,
    pub(crate) rollbacks: u64,
    pub(crate) rolled_back_ops: u64,
    pub(crate) next_entity_bits: u64,
}

impl CommandQueue {
    fn new() -> Self {
        Self {
            ops: Vec::with_capacity(256),
            comp_meta: Vec::new(),
            checkpoints: 0,
            rollbacks: 0,
            rolled_back_ops: 0,
            next_entity_bits: 100,
        }
    }

    fn meta_for(&self, comp: BamComponent) -> Option<CompCommandMeta> {
        let idx = (comp.to_raw() as usize).checked_sub(1)?;
        self.comp_meta.get(idx).copied()
    }

    pub(crate) fn clear(&mut self) {
        self.ops.clear();
        self.checkpoints = 0;
        self.rollbacks = 0;
        self.rolled_back_ops = 0;
    }

    pub(crate) fn reserve_ops(&mut self, min_additional: usize) {
        if self.ops.capacity() < min_additional {
            self.ops.reserve(min_additional - self.ops.len());
        }
    }

    /// Directly pushes a spawn operation (used by the native Rust baseline).
    pub(crate) fn push_native_spawn(&mut self) -> FfiEntity {
        let e = FfiEntity::from_bits(self.next_entity_bits);
        self.next_entity_bits += 1;
        self.ops.push(RecordedCmd::Spawn {
            _entity: e,
            inline_inserts: 0,
        });
        e
    }

    /// Directly pushes a POD component insertion (used by the native Rust baseline).
    pub(crate) fn push_native_insert_pod<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: &T,
    ) -> FfiResult {
        let Some(meta) = self.meta_for(component) else {
            return FfiResult::INVALID_COMPONENT;
        };
        let len = core::mem::size_of::<T>();
        if meta.size != len {
            return FfiResult::TYPE_MISMATCH;
        }
        let mut inline_bytes = [0u8; 32];
        if len > 0 && len <= inline_bytes.len() {
            // SAFETY: `value` is a live `&T` (`T: AbiPod`) valid for `len` readable bytes and
            // `inline_bytes` has capacity `>= len`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    core::ptr::from_ref(value).cast::<u8>(),
                    inline_bytes.as_mut_ptr(),
                    len,
                );
            }
        }
        self.ops.push(RecordedCmd::Insert {
            _entity: entity,
            component,
            byte_len: len,
            _inline_bytes: inline_bytes,
        });
        FfiResult::OK
    }

    /// Captures a command-queue checkpoint token (used by the native Rust baseline).
    pub(crate) fn checkpoint(&mut self) -> usize {
        self.checkpoints += 1;
        self.ops.len()
    }

    /// Rolls back commands queued after `token` (used by the native Rust baseline).
    pub(crate) fn rollback(&mut self, token: usize) -> FfiResult {
        if token > self.ops.len() {
            return FfiResult::OUT_OF_RANGE;
        }
        self.rollbacks += 1;
        self.rolled_back_ops += (self.ops.len() - token) as u64;
        self.ops.truncate(token);
        FfiResult::OK
    }

    pub(crate) fn stats(&self) -> CommandStats {
        let mut stats = CommandStats {
            checkpoints: self.checkpoints,
            rollbacks: self.rollbacks,
            rolled_back_ops: self.rolled_back_ops,
            ..CommandStats::default()
        };
        for op in &self.ops {
            match *op {
                RecordedCmd::SpawnEmpty { .. } => stats.committed_spawns += 1,
                RecordedCmd::Spawn { inline_inserts, .. } => {
                    stats.committed_spawns += 1;
                    stats.committed_inserts += inline_inserts;
                }
                RecordedCmd::SpawnBatch {
                    entity_count,
                    component_count,
                } => {
                    stats.committed_spawns += entity_count;
                    stats.committed_inserts += entity_count * component_count;
                }
                RecordedCmd::Insert {
                    component,
                    byte_len,
                    ..
                }
                | RecordedCmd::InsertIfNew {
                    component,
                    byte_len,
                    ..
                } => {
                    stats.committed_inserts += 1;
                    stats.last_insert_component = component;
                    stats.last_insert_len = byte_len;
                }
                _ => {}
            }
        }
        stats
    }
}

/// Dense + sparse archetype chunk using `UnsafeCell` storage so simultaneous read/write column
/// pointers across distinct terms never violate Rust's aliasing rules.
pub(crate) struct ChunkStorage {
    pub(crate) entities: Vec<FfiEntity>,
    pub(crate) dense_columns: Vec<Option<Vec<UnsafeCell<u64>>>>,
    pub(crate) chunk_marked_changed: Vec<UnsafeCell<bool>>,
    pub(crate) marked_rows: Vec<UnsafeCell<Vec<usize>>>,
    pub(crate) sparse_columns: Vec<Option<Vec<Option<Vec<UnsafeCell<u64>>>>>>,
    pub(crate) added_ticks: Vec<u32>,
    pub(crate) changed_ticks: Vec<u32>,
}

impl ChunkStorage {
    pub(crate) fn new(entities: Vec<FfiEntity>, comp_capacity: usize) -> Self {
        let tick_len = entities.len();
        let mut dense_columns = Vec::with_capacity(comp_capacity);
        let mut chunk_marked_changed = Vec::with_capacity(comp_capacity);
        let mut marked_rows = Vec::with_capacity(comp_capacity);
        let mut sparse_columns = Vec::with_capacity(comp_capacity);
        for _ in 0..comp_capacity {
            dense_columns.push(None);
            chunk_marked_changed.push(UnsafeCell::new(false));
            marked_rows.push(UnsafeCell::new(Vec::new()));
            sparse_columns.push(None);
        }
        Self {
            entities,
            dense_columns,
            chunk_marked_changed,
            marked_rows,
            sparse_columns,
            added_ticks: vec![1; tick_len],
            changed_ticks: vec![1; tick_len],
        }
    }

    fn ensure_comp_slot(&mut self, idx: usize) {
        while self.dense_columns.len() <= idx {
            self.dense_columns.push(None);
            self.chunk_marked_changed.push(UnsafeCell::new(false));
            self.marked_rows.push(UnsafeCell::new(Vec::new()));
            self.sparse_columns.push(None);
        }
    }

    pub(crate) fn insert_dense<T: Copy>(&mut self, comp: BamComponent, values: &[T]) {
        assert_eq!(values.len(), self.entities.len());
        let idx = (comp.to_raw() as usize)
            .checked_sub(1)
            .expect("valid component handle");
        self.ensure_comp_slot(idx);
        let byte_len = core::mem::size_of_val(values);
        let word_len = byte_len.div_ceil(8).max(1);
        let words: Vec<UnsafeCell<u64>> = (0..word_len).map(|_| UnsafeCell::new(0)).collect();
        if byte_len > 0 {
            let dst = UnsafeCell::raw_get(words.as_ptr()).cast::<u8>();
            // SAFETY: `words` owns `word_len * 8 >= byte_len` writable, 8-byte aligned bytes and
            // `values` provides `byte_len` readable bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), dst, byte_len);
            }
        }
        self.dense_columns[idx] = Some(words);
    }

    pub(crate) fn overwrite_dense<T: Copy>(&mut self, comp: BamComponent, values: &[T]) {
        assert_eq!(values.len(), self.entities.len());
        let idx = (comp.to_raw() as usize)
            .checked_sub(1)
            .expect("valid component handle");
        if let Some(Some(words)) = self.dense_columns.get(idx) {
            let byte_len = core::mem::size_of_val(values);
            if byte_len > 0 {
                debug_assert!(words.len() * 8 >= byte_len);
                let dst = UnsafeCell::raw_get(words.as_ptr()).cast::<u8>();
                // SAFETY: `words` was sized for `entities.len()` `T` elements and we hold `&mut self`.
                unsafe {
                    core::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), dst, byte_len);
                }
            }
        } else {
            self.insert_dense(comp, values);
        }
    }

    pub(crate) fn read_dense<T: Copy>(&self, comp: BamComponent) -> Vec<T> {
        let count = self.entities.len();
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T: Copy` requires no memory reads.
            return (0..count).map(|_| unsafe { core::mem::zeroed() }).collect();
        }
        let idx = (comp.to_raw() as usize)
            .checked_sub(1)
            .expect("valid component handle");
        let words = self.dense_columns[idx]
            .as_ref()
            .expect("missing dense column");
        let mut out = Vec::<T>::with_capacity(count);
        let src = UnsafeCell::raw_get(words.as_ptr()).cast::<T>();
        // SAFETY: `words` holds `count` initialized `T` values and `out` has capacity `count`.
        unsafe {
            core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), count);
            out.set_len(count);
        }
        out
    }

    /// Returns a raw mutable pointer to dense column `comp` (used by native baseline).
    pub(crate) fn dense_col_mut_ptr<T: Copy>(&self, comp: BamComponent) -> *mut T {
        let idx = (comp.to_raw() as usize) - 1;
        let words = self.dense_columns[idx]
            .as_ref()
            .expect("missing dense column");
        UnsafeCell::raw_get(words.as_ptr()).cast::<T>()
    }

    /// Borrows dense column `comp` directly as a shared `&[T]` slice (used by native baseline).
    ///
    /// # Safety
    /// Caller must ensure `comp` is stored as `T` with `entities.len()` initialized elements and no
    /// mutable borrows of `comp` in this chunk exist simultaneously.
    pub(crate) unsafe fn dense_slice_unchecked<T: Copy>(&self, comp: BamComponent) -> &[T] {
        let idx = (comp.to_raw() as usize) - 1;
        let words = self.dense_columns[idx]
            .as_ref()
            .expect("missing dense column");
        let ptr = UnsafeCell::raw_get(words.as_ptr()).cast::<T>();
        // SAFETY: Caller guarantees `words` holds `entities.len()` initialized `T` elements and no
        // active mutable borrow exists.
        unsafe { core::slice::from_raw_parts(ptr, self.entities.len()) }
    }

    pub(crate) fn mark_changed(&self, comp: BamComponent) {
        if let Some(cell) = comp
            .to_raw()
            .checked_sub(1)
            .and_then(|idx| self.chunk_marked_changed.get(idx as usize))
        {
            // SAFETY: Single-writer cell mutated only via raw pointer without creating `&mut ChunkStorage`.
            unsafe {
                *cell.get() = true;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_marked_changed(&self, comp: BamComponent) -> bool {
        comp.to_raw()
            .checked_sub(1)
            .and_then(|idx| self.chunk_marked_changed.get(idx as usize))
            .is_some_and(|cell| {
                // SAFETY: Reading `UnsafeCell<bool>` outside active mutations.
                unsafe { *cell.get() }
            })
    }

    fn has_component(&self, comp: BamComponent) -> bool {
        let Some(idx) = comp.to_raw().checked_sub(1) else {
            return false;
        };
        let idx = idx as usize;
        self.dense_columns.get(idx).is_some_and(Option::is_some)
            || self.sparse_columns.get(idx).is_some_and(Option::is_some)
    }
}

/// Singleton resource slot storing 8-byte-aligned payload words and change-detection ticks.
pub(crate) struct ResourceSlot {
    pub(crate) words: Vec<UnsafeCell<u64>>,
    pub(crate) ticks: UnsafeCell<BamChangeTicks>,
}

/// Per-query iterator state backing `*mut BamIter`.
pub(crate) struct IterState {
    pub(crate) world: *const HostWorld,
    pub(crate) sys_idx: usize,
    pub(crate) query_idx: usize,
    pub(crate) terms: Vec<BamTermDesc>,
    pub(crate) start_chunk_idx: usize,
    pub(crate) next_chunk_idx: usize,
    pub(crate) end_chunk_idx: Option<usize>,
    pub(crate) active_chunk_idx: Option<usize>,
    pub(crate) in_par_iter: bool,
    pub(crate) ro_rows_scratch: Vec<*const u8>,
    pub(crate) mut_rows_scratch: Vec<*mut u8>,
}

/// Stack-allocated per-system execution frame backing `*mut BamSystemCtx`.
#[repr(C)]
struct SystemFrame {
    world: *const HostWorld,
    shared: *const HostSharedState,
    commands: *mut CommandQueue,
    sys_idx: usize,
    iter_ptrs: [*mut IterState; MAX_QUERIES_PER_SYSTEM],
    query_count: usize,
}

/// Registered guest system descriptor and pre-allocated query iterators.
pub(crate) struct RegisteredSystem {
    pub(crate) name: String,
    pub(crate) queries: Vec<Vec<BamTermDesc>>,
    pub(crate) iters: Box<[UnsafeCell<IterState>]>,
    pub(crate) schedule: ScheduleId,
    pub(crate) _flags: u32,
    pub(crate) callback: BamSystemFn,
    pub(crate) user_data: *mut c_void,
    pub(crate) run_if: Option<BamRunIfFn>,
    pub(crate) run_if_user_data: *mut c_void,
}

/// Symbol interner state protected by a mutex inside [`HostSharedState`].
struct SymbolTable {
    map: HashMap<Box<str>, BamSymbol>,
    list: Vec<Box<str>>,
}

impl SymbolTable {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            list: Vec::new(),
        }
    }

    fn intern(&mut self, s: &str) -> BamSymbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = BamSymbol::from_raw(self.list.len() as u32);
        let owned: Box<str> = s.into();
        self.list.push(owned.clone());
        self.map.insert(owned, sym);
        sym
    }
}

/// Thread-safe shared host context backing `*const BamHostCtx`.
pub(crate) struct HostSharedState {
    world: AtomicPtr<HostWorld>,
    last_error: Mutex<String>,
    faults: Mutex<Vec<String>>,
    proc_symbols: Mutex<HashMap<String, usize>>,
    symbol_table: Mutex<SymbolTable>,
    named_objects: Mutex<HashMap<String, FfiEntity>>,
    intern_symbol_calls: AtomicU64,
    res_ticks_calls: AtomicU64,
    msg_send_calls: AtomicU64,
    diagnostic_logs: AtomicU64,
}

impl HostSharedState {
    fn new() -> Self {
        Self {
            world: AtomicPtr::new(core::ptr::null_mut()),
            last_error: Mutex::new(String::new()),
            faults: Mutex::new(Vec::new()),
            proc_symbols: Mutex::new(HashMap::new()),
            symbol_table: Mutex::new(SymbolTable::new()),
            named_objects: Mutex::new(HashMap::new()),
            intern_symbol_calls: AtomicU64::new(0),
            res_ticks_calls: AtomicU64::new(0),
            msg_send_calls: AtomicU64::new(0),
            diagnostic_logs: AtomicU64::new(0),
        }
    }

    pub(crate) fn intern_symbol_str(&self, sym: &str) -> BamSymbol {
        self.intern_symbol_calls.fetch_add(1, Ordering::Relaxed);
        let mut guard = self
            .symbol_table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.intern(sym)
    }
}

/// Concrete Layer-0 C-ABI `0.3.0` host ECS world for benchmarking and verifying polyglot guests.
pub(crate) struct HostWorld {
    pub(crate) api: BamHostApi,
    pub(crate) shared: HostSharedState,
    pub(crate) components: Vec<RegisteredComp>,
    pub(crate) name_to_handle: HashMap<String, BamComponent>,
    pub(crate) uuid_to_handle: HashMap<ComponentUuid, BamComponent>,
    pub(crate) resources: Vec<Option<ResourceSlot>>,
    pub(crate) messages: UnsafeCell<HashMap<BamComponent, Vec<Vec<MaybeUninit<u8>>>>>,
    pub(crate) msg_cursors: UnsafeCell<HashMap<(usize, usize, usize), usize>>,
    pub(crate) chunks: Vec<ChunkStorage>,
    pub(crate) systems: Vec<RegisteredSystem>,
    pub(crate) schedule_sets: Vec<String>,
    pub(crate) commands: UnsafeCell<CommandQueue>,
    pub(crate) pos_handle: BamComponent,
    pub(crate) vel_handle: BamComponent,
    pub(crate) sim_params_handle: BamComponent,
}

/// Owning wrapper around a heap-pinned [`HostWorld`] preserving stable pointer provenance across
/// all `BamLoadCtx`, `BamHostCtx`, `BamSystemCtx`, `BamIter`, and `BamCommands` FFI handles.
pub(crate) struct OwnedHostWorld {
    raw: *mut HostWorld,
}

impl Drop for OwnedHostWorld {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `self.raw` was allocated via `Box::into_raw` in `HostWorld::new` and no
            // FFI callbacks or pointers outlive `OwnedHostWorld`.
            unsafe {
                drop(Box::from_raw(self.raw));
            }
        }
    }
}

impl core::ops::Deref for OwnedHostWorld {
    type Target = HostWorld;

    fn deref(&self) -> &Self::Target {
        // SAFETY: `self.raw` is non-null, initialized, and valid until `Drop`.
        unsafe { &*self.raw }
    }
}

impl core::ops::DerefMut for OwnedHostWorld {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `self.raw` is non-null, initialized, and exclusively borrowed via `&mut self`.
        unsafe { &mut *self.raw }
    }
}

impl OwnedHostWorld {
    /// Returns the raw `*const BamHostApi` table pointer.
    pub(crate) const fn api_ptr(&self) -> *const BamHostApi {
        // SAFETY: `self.raw` points to a valid `HostWorld`; `addr_of!` avoids intermediate `&HostWorld`.
        unsafe { core::ptr::addr_of!((*self.raw).api) }
    }

    /// Returns the `*mut BamLoadCtx` pointer for `bamabi_init`.
    pub(crate) const fn load_ctx_ptr(&self) -> *mut BamLoadCtx {
        self.raw.cast::<BamLoadCtx>()
    }

    /// Returns the `*const BamHostCtx` pointer for direct diagnostics or symbol tests.
    #[cfg(test)]
    pub(crate) const fn host_ctx_ptr(&self) -> *const BamHostCtx {
        // SAFETY: `self.raw` points to a valid `HostWorld`; `addr_of!` projects to `shared`.
        unsafe { core::ptr::addr_of!((*self.raw).shared).cast::<BamHostCtx>() }
    }

    /// Runs registered system `sys_idx` using a stack-allocated [`SystemFrame`] and pre-allocated
    /// per-query [`IterState`] slots (zero heap allocations per tick).
    pub(crate) fn run_system(&self, sys_idx: usize) -> FfiResult {
        let world_ptr = self.raw;
        // SAFETY: No guest callback is active yet; we inspect `systems[sys_idx]` and reset its
        // pre-allocated `IterState` cursors before invoking the `extern "C"` callback.
        let (run_if, run_if_data, callback, user_data, query_count, iter_ptrs) = unsafe {
            let world_ref = &*world_ptr;
            let sys = &world_ref.systems[sys_idx];
            let mut ptrs = [core::ptr::null_mut::<IterState>(); MAX_QUERIES_PER_SYSTEM];
            for (q_idx, iter_cell) in sys.iters.iter().enumerate() {
                let iter_ptr = iter_cell.get();
                (*iter_ptr).world = world_ptr;
                (*iter_ptr).sys_idx = sys_idx;
                (*iter_ptr).query_idx = q_idx;
                (*iter_ptr).start_chunk_idx = 0;
                (*iter_ptr).next_chunk_idx = 0;
                (*iter_ptr).end_chunk_idx = None;
                (*iter_ptr).active_chunk_idx = None;
                (*iter_ptr).in_par_iter = false;
                ptrs[q_idx] = iter_ptr;
            }
            (
                sys.run_if,
                sys.run_if_user_data,
                sys.callback,
                sys.user_data,
                sys.iters.len(),
                ptrs,
            )
        };

        if let Some(pred) = run_if {
            // SAFETY: Guest registered `pred` and `run_if_data` during `bamabi_init`.
            let should_run = unsafe { pred(run_if_data) };
            if !should_run.get() {
                return FfiResult::OK;
            }
        }

        // SAFETY: Projecting disjoint raw pointers to `shared` and `commands` without holding `&mut HostWorld`.
        let (shared_ptr, commands_ptr) = unsafe {
            (
                core::ptr::addr_of!((*world_ptr).shared),
                (*world_ptr).commands.get(),
            )
        };
        let mut frame = SystemFrame {
            world: world_ptr,
            shared: shared_ptr,
            commands: commands_ptr,
            sys_idx,
            iter_ptrs,
            query_count,
        };
        let sys_ctx = (&raw mut frame).cast::<BamSystemCtx>();
        // SAFETY: `sys_ctx` points to a valid `SystemFrame` alive for this call and `callback`/`user_data`
        // were registered by the loaded guest module.
        unsafe { callback(sys_ctx, user_data) }
    }
}

impl HostWorld {
    /// Allocates and initializes a new [`OwnedHostWorld`] pre-populated with the shared benchmark
    /// components (`"bench::Position"`, `"bench::Velocity"`) and `"bench::SimParams"` resource.
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new() -> OwnedHostWorld {
        let raw = Box::into_raw(Box::new(Self {
            api: make_host_api(),
            shared: HostSharedState::new(),
            components: Vec::new(),
            name_to_handle: HashMap::new(),
            uuid_to_handle: HashMap::new(),
            resources: Vec::new(),
            messages: UnsafeCell::new(HashMap::new()),
            msg_cursors: UnsafeCell::new(HashMap::new()),
            chunks: Vec::new(),
            systems: Vec::new(),
            schedule_sets: Vec::new(),
            commands: UnsafeCell::new(CommandQueue::new()),
            pos_handle: BamComponent::INVALID,
            vel_handle: BamComponent::INVALID,
            sim_params_handle: BamComponent::INVALID,
        }));
        let mut owned = OwnedHostWorld { raw };
        // SAFETY: `raw` was just initialized via `Box::into_raw`; initialize `shared.world` back-pointer
        // before handing out `BamHostCtx` or `BamLoadCtx`.
        unsafe {
            (*raw).shared.world.store(raw, Ordering::Relaxed);
        }
        owned.pos_handle =
            owned.expose_component::<Position>(POSITION_NAME, POSITION_UUID, 0, true);
        owned.vel_handle =
            owned.expose_component::<Velocity>(VELOCITY_NAME, VELOCITY_UUID, 0, true);
        owned.sim_params_handle = owned.insert_resource(
            SIM_PARAMS_NAME,
            SIM_PARAMS_UUID,
            SimParams {
                dt: BENCH_DT,
                damping: 0.98,
                tick: 1,
                _pad: 0,
            },
        );
        owned
    }

    /// Registers a host-provided component type `T` and returns its 1-based [`BamComponent`] handle.
    pub(crate) fn expose_component<T: 'static>(
        &mut self,
        name: &str,
        uuid: ComponentUuid,
        flags: u32,
        has_default: bool,
    ) -> BamComponent {
        let raw_id = (self.components.len() + 1) as u32;
        let handle = BamComponent::from_raw(raw_id);
        let size = core::mem::size_of::<T>();
        self.components.push(RegisteredComp {
            _handle: handle,
            _name: name.to_owned(),
            uuid,
            schema_fingerprint: 0,
            size,
            align: core::mem::align_of::<T>(),
            flags,
            is_resource: false,
            _has_default: has_default,
            _drop_fn: None,
            _vtable_ptr: core::ptr::null(),
            _on_add: None,
            _on_remove: None,
            _hook_user_data: core::ptr::null_mut(),
        });
        self.name_to_handle.insert(name.to_owned(), handle);
        if !uuid.is_zero() {
            self.uuid_to_handle.insert(uuid, handle);
        }
        self.resources.push(None);
        self.commands
            .get_mut()
            .comp_meta
            .push(CompCommandMeta { size, has_default });
        handle
    }

    /// Inserts or updates singleton resource `R` in the host world.
    pub(crate) fn insert_resource<R: Copy + 'static>(
        &mut self,
        name: &str,
        uuid: ComponentUuid,
        value: R,
    ) -> BamComponent {
        let handle = self
            .name_to_handle
            .get(name)
            .copied()
            .unwrap_or_else(|| self.expose_component::<R>(name, uuid, 0, false));
        let idx = (handle.to_raw() as usize) - 1;
        self.components[idx].is_resource = true;
        let size = core::mem::size_of::<R>();
        let word_len = size.div_ceil(8).max(1);
        let words: Vec<UnsafeCell<u64>> = (0..word_len).map(|_| UnsafeCell::new(0)).collect();
        if size > 0 {
            let dst = UnsafeCell::raw_get(words.as_ptr()).cast::<u8>();
            // SAFETY: `words` provides `word_len * 8 >= size` writable aligned bytes and `value` is initialized.
            unsafe {
                core::ptr::copy_nonoverlapping(core::ptr::from_ref(&value).cast::<u8>(), dst, size);
            }
        }
        while self.resources.len() <= idx {
            self.resources.push(None);
        }
        self.resources[idx] = Some(ResourceSlot {
            words,
            ticks: UnsafeCell::new(BamChangeTicks::new(1, 1, 0, 1)),
        });
        handle
    }

    /// Populates `entity_count` dense entities split across chunks of `chunk_size` with deterministic
    /// initial [`Position`] and [`Velocity`] values.
    pub(crate) fn setup_dense_entities(&mut self, entity_count: usize, chunk_size: usize) {
        self.chunks.clear();
        if entity_count == 0 {
            return;
        }
        let effective_chunk = chunk_size.max(1);
        let comp_cap = self.components.len().max(4);
        let mut start = 0usize;
        while start < entity_count {
            let end = (start + effective_chunk).min(entity_count);
            let len = end - start;
            let mut entities = Vec::with_capacity(len);
            let mut positions = Vec::with_capacity(len);
            let mut velocities = Vec::with_capacity(len);
            for idx in start..end {
                entities.push(FfiEntity::from_bits((idx as u64) + 1));
                positions.push(initial_position(idx));
                velocities.push(initial_velocity(idx));
            }
            let mut chunk = ChunkStorage::new(entities, comp_cap);
            chunk.insert_dense(self.pos_handle, &positions);
            chunk.insert_dense(self.vel_handle, &velocities);
            self.chunks.push(chunk);
            start = end;
        }
    }

    /// Resets all chunk [`Position`] columns back to deterministic initial values without reallocating.
    pub(crate) fn reset_dense_positions(&mut self) {
        let pos_handle = self.pos_handle;
        let mut idx = 0usize;
        let mut scratch = Vec::new();
        for chunk in &mut self.chunks {
            let len = chunk.entities.len();
            scratch.clear();
            scratch.reserve(len);
            for _ in 0..len {
                scratch.push(initial_position(idx));
                idx += 1;
            }
            chunk.overwrite_dense(pos_handle, &scratch);
            if let Some(cell) = pos_handle
                .to_raw()
                .checked_sub(1)
                .and_then(|i| chunk.chunk_marked_changed.get_mut(i as usize))
            {
                *cell.get_mut() = false;
            }
        }
    }

    /// Reads back all [`Position`] values across every chunk in entity order.
    pub(crate) fn read_all_positions(&self) -> Vec<Position> {
        let mut out = Vec::new();
        for chunk in &self.chunks {
            out.extend_from_slice(&chunk.read_dense::<Position>(self.pos_handle));
        }
        out
    }

    /// Finds the system index matching `name`.
    pub(crate) fn find_system(&self, name: &str) -> Option<usize> {
        self.systems.iter().position(|s| s.name == name)
    }

    /// Clears registered systems and schedule sets prior to `dlclose` / `FreeLibrary`.
    pub(crate) fn clear_systems_for_unload(&mut self) {
        self.systems.clear();
        self.schedule_sets.clear();
    }

    /// Pre-reserves command queue capacity before a timed batch.
    pub(crate) fn reserve_command_capacity(&mut self, min_ops: usize) {
        self.commands.get_mut().reserve_ops(min_ops);
    }

    /// Clears recorded commands and checkpoint/rollback counters while retaining capacity.
    pub(crate) fn reset_commands(&mut self) {
        self.commands.get_mut().clear();
    }

    /// Returns a snapshot of committed spawns/inserts and checkpoint/rollback counts.
    pub(crate) fn command_stats(&self) -> CommandStats {
        // SAFETY: Called outside active guest FFI system callbacks.
        unsafe { (&*self.commands.get()).stats() }
    }

    /// Borrows [`CommandQueue`] mutably outside active guest callbacks (used by native baseline).
    pub(crate) fn commands_mut(&mut self) -> &mut CommandQueue {
        self.commands.get_mut()
    }

    /// Resets `intern_symbol` call counter.
    pub(crate) fn reset_intern_symbol_calls(&self) {
        self.shared.intern_symbol_calls.store(0, Ordering::Relaxed);
    }

    /// Returns total `intern_symbol` calls observed since last reset.
    pub(crate) fn intern_symbol_calls(&self) -> u64 {
        self.shared.intern_symbol_calls.load(Ordering::Relaxed)
    }

    /// Resets `res_ticks` call counter.
    pub(crate) fn reset_res_ticks_calls(&self) {
        self.shared.res_ticks_calls.store(0, Ordering::Relaxed);
    }

    /// Returns total `res_ticks` calls observed since last reset.
    pub(crate) fn res_ticks_calls(&self) -> u64 {
        self.shared.res_ticks_calls.load(Ordering::Relaxed)
    }

    /// Returns `true` if the guest reported any fault via `report_guest_fault`.
    pub(crate) fn has_fault(&self) -> bool {
        !self
            .shared
            .faults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Returns a copy of any faults reported by the guest.
    pub(crate) fn faults(&self) -> Vec<String> {
        self.shared
            .faults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns the most recent diagnostic error recorded on this world.
    pub(crate) fn last_error_string(&self) -> String {
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Borrows a resource slot's [`SimParams`] and [`BamChangeTicks`] (used by native baseline).
    pub(crate) fn read_sim_params_with_ticks(&self) -> Option<(SimParams, BamChangeTicks)> {
        let idx = (self.sim_params_handle.to_raw() as usize).checked_sub(1)?;
        let slot = self.resources.get(idx)?.as_ref()?;
        let ptr = UnsafeCell::raw_get(slot.words.as_ptr()).cast::<SimParams>();
        // SAFETY: `sim_params_handle` was initialized with `SimParams` (`16` bytes, 8-byte aligned
        // `words`) and ticks are valid outside concurrent writes.
        unsafe { Some((core::ptr::read(ptr), *slot.ticks.get())) }
    }

    /// Looks up registered component metadata by 1-based handle.
    pub(crate) fn comp_meta(&self, handle: BamComponent) -> Option<&RegisteredComp> {
        let idx = (handle.to_raw() as usize).checked_sub(1)?;
        self.components.get(idx)
    }

    /// Records a diagnostic error string on `shared.last_error`.
    fn set_last_error(&self, msg: &str) {
        let mut guard = self
            .shared
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clear();
        guard.push_str(msg);
    }
}

/// Deterministic bounded initial [`Position`] for entity `idx`.
#[inline]
pub(crate) fn initial_position(idx: usize) -> Position {
    let f = (idx % 97) as f32 * 0.125;
    Position {
        x: f,
        y: -f * 0.5,
        z: ((idx % 31) as f32) * 0.25,
        w: 1.0,
    }
}

/// Deterministic bounded initial [`Velocity`] for entity `idx`.
#[inline]
pub(crate) fn initial_velocity(idx: usize) -> Velocity {
    Velocity {
        vx: ((idx % 17) as f32) * 0.05 + 0.1,
        vy: ((idx % 13) as f32) * 0.04 - 0.2,
        vz: ((idx % 19) as f32) * 0.03 + 0.05,
        vw: ((idx % 7) as f32) * 0.01,
    }
}

fn chunk_matches_terms(chunk: &ChunkStorage, terms: &[BamTermDesc]) -> bool {
    terms.iter().all(|term| {
        if term.mode.is_resource()
            || term.mode.is_message()
            || term.mode.is_removed_read()
            || term.mode.is_allow()
            || term.mode.is_has()
        {
            return true;
        }
        let present = chunk.has_component(term.component);
        if term.mode.is_without() {
            !present
        } else if term.mode.is_optional() {
            true
        } else {
            present
        }
    })
}

fn storage_flags_for_meta(meta: &RegisteredComp, mode: Option<AccessMode>) -> u32 {
    let mut storage_flags = 0u32;
    if (meta.flags & BAM_COMPONENT_FLAG_SPARSE_SET) != 0 {
        storage_flags |= BAM_STORAGE_FLAG_SPARSE_SET;
    }
    if (meta.flags & BAM_COMPONENT_FLAG_IMMUTABLE) != 0 {
        storage_flags |= BAM_STORAGE_FLAG_IMMUTABLE;
    }
    if (meta.flags & BAM_COMPONENT_FLAG_SUMMARY_TICK) != 0 {
        storage_flags |= BAM_STORAGE_FLAG_SUMMARY_TICK;
    }
    if meta.is_resource || mode.is_some_and(AccessMode::is_resource) {
        storage_flags |= BAM_STORAGE_FLAG_RESOURCE;
    }
    if mode.is_some_and(AccessMode::is_message) {
        storage_flags |= BAM_STORAGE_FLAG_MESSAGE;
    }
    storage_flags
}

// ============================================================================
// All 68 C-ABI `0.3.0` Host VTable Functions
// ============================================================================

/// Slot 0: `get_proc_address`.
unsafe extern "C" fn host_get_proc_address(
    ctx: *const BamHostCtx,
    name: *const c_char,
) -> *const c_void {
    if ctx.is_null() || name.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `ctx` points to `HostSharedState` and `name` is a NUL-terminated C string.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    // SAFETY: Caller guarantees `name` is valid NUL-terminated C string.
    let Ok(key) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        return core::ptr::null();
    };
    let guard = shared
        .proc_symbols
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .get(key)
        .copied()
        .map_or(core::ptr::null(), |addr| addr as *const c_void)
}

/// Slot 1: `last_error`.
unsafe extern "C" fn host_last_error(
    ctx: *const BamHostCtx,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    if ctx.is_null() {
        return 0;
    }
    // SAFETY: `ctx` points to an active `HostSharedState`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    let msg = shared
        .last_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = msg.as_bytes();
    if !buf.is_null() && cap > 0 {
        let copy_len = bytes.len().min(cap - 1);
        // SAFETY: `buf` has at least `cap` writable bytes per `BamLastErrorFn` contract.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), copy_len);
            *buf.add(copy_len) = 0;
        }
    }
    bytes.len()
}

/// Slot 2: `load_host_ctx`.
unsafe extern "C" fn host_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
    if load_ctx.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `load_ctx` points to an active `HostWorld` during `bamabi_init`.
    let world_ptr = load_ctx.cast::<HostWorld>();
    // SAFETY: `addr_of!` projects to `shared` without creating an aliasing reference.
    unsafe { core::ptr::addr_of!((*world_ptr).shared).cast::<BamHostCtx>() }
}

/// Slot 3: `register_component`.
unsafe extern "C" fn host_register_component(
    load_ctx: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out_comp: *mut BamComponent,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_comp.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `ComponentDesc` prefix of at least `struct_size` bytes.
    let d = match unsafe { read_descriptor::<ComponentDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d,
        Err(err) => return err,
    };
    let valid = d.validate();
    if !valid.is_ok() {
        return valid;
    }
    // SAFETY: `d.validate()` verified `d.name` is non-null; guest keeps string live for call.
    let Some(cstr) = (unsafe { d.name_cstr() }) else {
        return FfiResult::NULL_POINTER;
    };
    let Ok(name_str) = cstr.to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    if name_str.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }

    // SAFETY: `load_ctx` points to an exclusively held `HostWorld` during `bamabi_init`.
    let world = unsafe { &mut *load_ctx.cast::<HostWorld>() };
    if let Some(&existing) = world.name_to_handle.get(name_str) {
        let Some(meta) = world.comp_meta(existing) else {
            return FfiResult::INTERNAL_ERROR;
        };
        if meta.size != d.size
            || meta.align != d.align
            || (!d.uuid.is_zero() && !meta.uuid.is_zero() && meta.uuid != d.uuid)
        {
            world.set_last_error(&format!(
                "component layout or UUID mismatch for `{name_str}`"
            ));
            return FfiResult::TYPE_MISMATCH;
        }
        // SAFETY: `out_comp` is non-null writable `BamComponent` storage.
        unsafe {
            *out_comp = existing;
        }
        return FfiResult::OK;
    }

    let raw_id = (world.components.len() + 1) as u32;
    let handle = BamComponent::from_raw(raw_id);
    world.components.push(RegisteredComp {
        _handle: handle,
        _name: name_str.to_owned(),
        uuid: d.uuid,
        schema_fingerprint: d.schema_fingerprint,
        size: d.size,
        align: d.align,
        flags: d.flags,
        is_resource: false,
        _has_default: false,
        _drop_fn: d.drop_fn,
        _vtable_ptr: d.vtable_ptr,
        _on_add: d.on_add,
        _on_remove: d.on_remove,
        _hook_user_data: d.hook_user_data,
    });
    world.name_to_handle.insert(name_str.to_owned(), handle);
    if !d.uuid.is_zero() {
        world.uuid_to_handle.insert(d.uuid, handle);
    }
    world.resources.push(None);
    world.commands.get_mut().comp_meta.push(CompCommandMeta {
        size: d.size,
        has_default: false,
    });
    // SAFETY: `out_comp` is non-null writable `BamComponent` storage.
    unsafe {
        *out_comp = handle;
    }
    FfiResult::OK
}

/// Helper locating `HostWorld` from `*const BamHostCtx` (which points to `HostWorld::shared`).
///
/// # Safety
/// `ctx` must point to the `shared` field of a live `HostWorld`.
#[inline]
unsafe fn world_from_host_ctx<'a>(ctx: *const BamHostCtx) -> &'a HostWorld {
    // SAFETY: `ctx` points to `HostWorld::shared` of a live `HostWorld`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    let world_ptr = shared.world.load(Ordering::Relaxed);
    // SAFETY: `HostWorld::new` initializes `shared.world` before exposing `BamHostCtx`, and
    // `HostWorld` outlives the context pointer.
    unsafe { &*world_ptr }
}

/// Slot 4: `resolve_component`.
unsafe extern "C" fn host_resolve_component(
    ctx: *const BamHostCtx,
    name: *const c_char,
    uuid: *const ComponentUuid,
    out_comp: *mut BamComponent,
) -> FfiResult {
    if ctx.is_null() || out_comp.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if name.is_null() && uuid.is_null() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `ctx` points to `HostWorld::shared` of a live `HostWorld`.
    let world = unsafe { world_from_host_ctx(ctx) };
    if !name.is_null() {
        // SAFETY: Caller guarantees `name` is a NUL-terminated C string.
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else {
            return FfiResult::INVALID_UTF8;
        };
        let Some(&handle) = world.name_to_handle.get(name_str) else {
            world.set_last_error(&format!("component `{name_str}` not found"));
            return FfiResult::NOT_FOUND;
        };
        if !uuid.is_null() {
            // SAFETY: Caller guarantees `uuid` points to a readable `ComponentUuid`.
            let expected = unsafe { *uuid };
            let Some(meta) = world.comp_meta(handle) else {
                return FfiResult::INTERNAL_ERROR;
            };
            if !meta.uuid.is_zero() && !expected.is_zero() && meta.uuid != expected {
                world.set_last_error(&format!("UUID mismatch resolving component `{name_str}`"));
                return FfiResult::TYPE_MISMATCH;
            }
        }
        // SAFETY: `out_comp` is non-null writable `BamComponent` storage.
        unsafe {
            *out_comp = handle;
        }
        return FfiResult::OK;
    }

    // SAFETY: Caller guarantees `uuid` is non-null and readable.
    let expected = unsafe { *uuid };
    let Some(&handle) = world.uuid_to_handle.get(&expected) else {
        world.set_last_error("component UUID not found");
        return FfiResult::NOT_FOUND;
    };
    // SAFETY: `out_comp` is non-null writable `BamComponent` storage.
    unsafe {
        *out_comp = handle;
    }
    FfiResult::OK
}

/// Shared implementation for `register_system` and `register_system_handle`.
unsafe fn register_system_impl(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> Result<BamSystem, FfiResult> {
    if load_ctx.is_null() || desc.is_null() {
        return Err(FfiResult::NULL_POINTER);
    }
    // SAFETY: `desc` points to a readable `BamSystemDesc` prefix of `struct_size` bytes.
    let d = unsafe { read_descriptor::<BamSystemDesc>(desc.cast::<c_void>()) }?;
    let status = d.validate();
    if !status.is_ok() {
        return Err(status);
    }
    // SAFETY: `d.validate()` verified `d.name` is non-null.
    let Ok(sys_name) = (unsafe { CStr::from_ptr(d.name) }).to_str() else {
        return Err(FfiResult::INVALID_UTF8);
    };
    if sys_name.is_empty() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }

    let mut queries = Vec::with_capacity(d.query_count);
    for q_idx in 0..d.query_count {
        // SAFETY: `d.validate()` verified `d.queries` is non-null for `d.query_count > 0`.
        let q_ptr = unsafe { d.queries.add(q_idx) };
        // SAFETY: `q_ptr` points to a readable `BamQueryDesc`.
        let q = unsafe { read_descriptor::<BamQueryDesc>(q_ptr.cast::<c_void>()) }?;
        let q_status = q.validate();
        if !q_status.is_ok() {
            return Err(q_status);
        }
        let terms = if q.term_count == 0 {
            Vec::new()
        } else {
            // SAFETY: `q.validate()` verified `q.terms` is non-null for `q.term_count > 0`.
            let slice = unsafe { core::slice::from_raw_parts(q.terms, q.term_count) };
            for term in slice {
                let t_status = term.validate();
                if !t_status.is_ok() {
                    return Err(t_status);
                }
            }
            slice.to_vec()
        };
        queries.push(terms);
    }

    // Validate ordering/set C string pointer arrays if provided.
    for (arr, count) in [
        (d.after, d.after_count),
        (d.before, d.before_count),
        (d.in_sets, d.in_set_count),
        (d.after_weak, d.after_weak_count),
        (d.before_weak, d.before_weak_count),
        (d.ambiguous_with, d.ambiguous_with_count),
    ] {
        for i in 0..count {
            // SAFETY: `d.validate()` verified `arr` is non-null whenever `count > 0`.
            let ptr = unsafe { *arr.add(i) };
            if ptr.is_null() {
                return Err(FfiResult::NULL_POINTER);
            }
            // SAFETY: Non-null C string pointer in ordering array.
            if unsafe { CStr::from_ptr(ptr) }.to_str().is_err() {
                return Err(FfiResult::INVALID_UTF8);
            }
        }
    }

    // SAFETY: `load_ctx` points to an exclusively held `HostWorld` during `bamabi_init`.
    let world = unsafe { &mut *load_ctx.cast::<HostWorld>() };
    let sys_idx = world.systems.len();
    let iters: Vec<UnsafeCell<IterState>> = queries
        .iter()
        .enumerate()
        .map(|(q_idx, terms)| {
            UnsafeCell::new(IterState {
                world: core::ptr::null(),
                sys_idx,
                query_idx: q_idx,
                terms: terms.clone(),
                start_chunk_idx: 0,
                next_chunk_idx: 0,
                end_chunk_idx: None,
                active_chunk_idx: None,
                in_par_iter: false,
                ro_rows_scratch: Vec::new(),
                mut_rows_scratch: Vec::new(),
            })
        })
        .collect();

    let Some(callback) = d.callback else {
        return Err(FfiResult::NULL_POINTER);
    };
    world.systems.push(RegisteredSystem {
        name: sys_name.to_owned(),
        queries,
        iters: iters.into_boxed_slice(),
        schedule: d.schedule,
        _flags: d.flags,
        callback,
        user_data: d.user_data,
        run_if: d.run_if,
        run_if_user_data: d.run_if_user_data,
    });
    Ok(BamSystem::from_raw(sys_idx as u32))
}

/// Slot 5: `register_system`.
unsafe extern "C" fn host_register_system(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> FfiResult {
    // SAFETY: Caller contract matches `register_system_impl`.
    match unsafe { register_system_impl(load_ctx, desc) } {
        Ok(_) => FfiResult::OK,
        Err(err) => err,
    }
}

/// Slot 6: `sys_host_ctx`.
unsafe extern "C" fn host_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
    if sys_ctx.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `sys_ctx` points to the stack-allocated `SystemFrame` for the active system call.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    frame.shared.cast::<BamHostCtx>()
}

/// Slot 7: `sys_query`.
unsafe extern "C" fn host_sys_query(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    out_iter: *mut *mut BamIter,
) -> FfiResult {
    if sys_ctx.is_null() || out_iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    if query_index >= frame.query_count {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: `out_iter` is non-null writable `*mut BamIter` storage.
    unsafe {
        *out_iter = frame.iter_ptrs[query_index].cast::<BamIter>();
    }
    FfiResult::OK
}

/// Slot 8: `sys_commands`.
unsafe extern "C" fn host_sys_commands(
    sys_ctx: *mut BamSystemCtx,
    out_commands: *mut *mut BamCommands,
) -> FfiResult {
    if sys_ctx.is_null() || out_commands.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    // SAFETY: `out_commands` is non-null writable `*mut BamCommands` storage.
    unsafe {
        *out_commands = frame.commands.cast::<BamCommands>();
    }
    FfiResult::OK
}

/// Slot 9: `iter_next_chunk`.
unsafe extern "C" fn host_iter_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
    if iter.is_null() || out_len.is_null() {
        return FfiBool::FALSE;
    }
    // SAFETY: `iter` points to a valid `IterState` for this query slot.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    // SAFETY: `state.world` is valid and its `chunks` slice is immutable during system execution.
    let world = unsafe { &*state.world };
    let limit = state
        .end_chunk_idx
        .map_or(world.chunks.len(), |end| end.min(world.chunks.len()));
    while state.next_chunk_idx < limit {
        let idx = state.next_chunk_idx;
        state.next_chunk_idx += 1;
        let chunk = &world.chunks[idx];
        if !chunk.entities.is_empty() && chunk_matches_terms(chunk, &state.terms) {
            state.active_chunk_idx = Some(idx);
            // SAFETY: Caller guarantees `out_len` is writable `usize` storage.
            unsafe {
                *out_len = chunk.entities.len();
            }
            return FfiBool::TRUE;
        }
    }
    state.active_chunk_idx = None;
    // SAFETY: Caller guarantees `out_len` is writable `usize` storage.
    unsafe {
        *out_len = 0;
    }
    FfiBool::FALSE
}

/// Slot 68: `iter_next_chunk_batch`.
unsafe extern "C" fn host_iter_next_chunk_batch(
    iter: *mut BamIter,
    out_len: *mut usize,
    out_entities: *mut *const FfiEntity,
    out_col_ptrs: *mut *mut u8,
    col_count: usize,
    mark_changed_mask: u32,
) -> FfiBool {
    if iter.is_null() || out_len.is_null() {
        return FfiBool::FALSE;
    }
    // SAFETY: `iter` points to a valid `IterState` for this query slot.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    // SAFETY: `state.world` is valid and its `chunks` slice is immutable during system execution.
    let world = unsafe { &*state.world };
    let limit = state
        .end_chunk_idx
        .map_or(world.chunks.len(), |end| end.min(world.chunks.len()));
    while state.next_chunk_idx < limit {
        let idx = state.next_chunk_idx;
        state.next_chunk_idx += 1;
        let chunk = &world.chunks[idx];
        if !chunk.entities.is_empty() && chunk_matches_terms(chunk, &state.terms) {
            state.active_chunk_idx = Some(idx);
            // SAFETY: Caller guarantees `out_len` is writable `usize` storage.
            unsafe {
                *out_len = chunk.entities.len();
            }
            if !out_entities.is_null() {
                // SAFETY: Caller guarantees `out_entities` is writable `*const FfiEntity` storage.
                unsafe {
                    *out_entities = chunk.entities.as_ptr();
                }
            }
            if col_count > 0 && !out_col_ptrs.is_null() {
                for term_index in 0..col_count {
                    let ptr = match state.terms.get(term_index) {
                        None => core::ptr::null_mut(),
                        Some(term) if term.mode.is_has() => {
                            let b = if chunk.has_component(term.component) {
                                core::ptr::from_ref(&FFI_BOOL_TRUE_BYTE)
                            } else {
                                core::ptr::from_ref(&FFI_BOOL_FALSE_BYTE)
                            };
                            b.cast_mut()
                        }
                        Some(term) if !term.mode.has_entity_column_data() => core::ptr::null_mut(),
                        Some(term) => {
                            let is_mut = term.mode.is_mutable();
                            let disallow_flags = if is_mut {
                                BAM_COMPONENT_FLAG_SPARSE_SET | BAM_COMPONENT_FLAG_IMMUTABLE
                            } else {
                                BAM_COMPONENT_FLAG_SPARSE_SET
                            };
                            match world.comp_meta(term.component) {
                                Some(meta) if (meta.flags & disallow_flags) == 0 => {
                                    match term.component.to_raw().checked_sub(1) {
                                        Some(comp_idx) => chunk
                                            .dense_columns
                                            .get(comp_idx as usize)
                                            .and_then(Option::as_ref)
                                            .map_or(core::ptr::null_mut(), |words| {
                                                if is_mut
                                                    && term_index < 32
                                                    && (mark_changed_mask & (1u32 << term_index))
                                                        != 0
                                                {
                                                    chunk.mark_changed(term.component);
                                                }
                                                UnsafeCell::raw_get(words.as_ptr()).cast::<u8>()
                                            }),
                                        None => core::ptr::null_mut(),
                                    }
                                }
                                _ => core::ptr::null_mut(),
                            }
                        }
                    };
                    // SAFETY: Caller guarantees `out_col_ptrs` has `col_count` writable slots.
                    unsafe {
                        *out_col_ptrs.add(term_index) = ptr;
                    }
                }
            }
            return FfiBool::TRUE;
        }
    }
    state.active_chunk_idx = None;
    // SAFETY: Caller guarantees `out_len` is writable `usize` storage.
    unsafe {
        *out_len = 0;
    }
    if !out_entities.is_null() {
        // SAFETY: Caller guarantees `out_entities` is writable `*const FfiEntity` storage.
        unsafe {
            *out_entities = core::ptr::null();
        }
    }
    if col_count > 0 && !out_col_ptrs.is_null() {
        for term_index in 0..col_count {
            // SAFETY: Caller guarantees `out_col_ptrs` has `col_count` writable slots.
            unsafe {
                *out_col_ptrs.add(term_index) = core::ptr::null_mut();
            }
        }
    }
    FfiBool::FALSE
}

/// Slot 10: `iter_reset`.
unsafe extern "C" fn host_iter_reset(iter: *mut BamIter) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    state.next_chunk_idx = state.start_chunk_idx;
    state.active_chunk_idx = None;
    FfiResult::OK
}

/// Slot 11: `iter_entities`.
unsafe extern "C" fn host_iter_entities(iter: *const BamIter) -> *const FfiEntity {
    if iter.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    // SAFETY: `state.world` outlives the system call and `c_idx` is valid.
    let world = unsafe { &*state.world };
    world.chunks[c_idx].entities.as_ptr()
}

/// Slot 12: `iter_column_readonly`.
unsafe extern "C" fn host_iter_column_readonly(
    iter: *const BamIter,
    term_index: usize,
) -> *const u8 {
    if iter.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null();
    };
    // SAFETY: `state.world` is valid and immutable structurally during system execution.
    let world = unsafe { &*state.world };
    let chunk = &world.chunks[c_idx];
    if term.mode.is_has() {
        return if chunk.has_component(term.component) {
            core::ptr::from_ref(&FFI_BOOL_TRUE_BYTE)
        } else {
            core::ptr::from_ref(&FFI_BOOL_FALSE_BYTE)
        };
    }
    if !term.mode.has_entity_column_data() {
        return core::ptr::null();
    }
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null();
    };
    if (meta.flags & BAM_COMPONENT_FLAG_SPARSE_SET) != 0 {
        return core::ptr::null();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    chunk
        .dense_columns
        .get(comp_idx as usize)
        .and_then(Option::as_ref)
        .map_or(core::ptr::null(), |words| {
            UnsafeCell::raw_get(words.as_ptr())
                .cast::<u8>()
                .cast_const()
        })
}

/// Slot 13: `iter_column_mutable`.
unsafe extern "C" fn host_iter_column_mutable(iter: *mut BamIter, term_index: usize) -> *mut u8 {
    // SAFETY: Forwarding `iter` and `term_index` to untracked column borrow.
    let ptr = unsafe { host_iter_column_mutable_untracked(iter, term_index) };
    if !ptr.is_null() {
        // SAFETY: `ptr` was non-null so `iter`, `active_chunk_idx`, and `term_index` are all valid.
        let state = unsafe { &*iter.cast::<IterState>() };
        if let Some(c_idx) = state.active_chunk_idx {
            // SAFETY: `state.world` is valid during system execution.
            let world = unsafe { &*state.world };
            world.chunks[c_idx].mark_changed(state.terms[term_index].component);
        }
    }
    ptr
}

/// Slot 14: `iter_column_mutable_untracked`.
unsafe extern "C" fn host_iter_column_mutable_untracked(
    iter: *mut BamIter,
    term_index: usize,
) -> *mut u8 {
    if iter.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return core::ptr::null_mut();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null_mut();
    };
    if !term.mode.is_mutable() || !term.mode.has_entity_column_data() {
        return core::ptr::null_mut();
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null_mut();
    };
    if (meta.flags & (BAM_COMPONENT_FLAG_SPARSE_SET | BAM_COMPONENT_FLAG_IMMUTABLE)) != 0 {
        return core::ptr::null_mut();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null_mut();
    };
    world.chunks[c_idx]
        .dense_columns
        .get(comp_idx as usize)
        .and_then(Option::as_ref)
        .map_or(core::ptr::null_mut(), |words| {
            UnsafeCell::raw_get(words.as_ptr()).cast::<u8>()
        })
}

/// Slot 15: `iter_mark_changed_range`.
unsafe extern "C" fn host_iter_mark_changed_range(
    iter: *mut BamIter,
    term_index: usize,
    start_row: usize,
    row_count: usize,
) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return FfiResult::INVALID_STATE;
    };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if !term.mode.is_mutable() {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let chunk = &world.chunks[c_idx];
    if start_row.saturating_add(row_count) > chunk.entities.len() {
        return FfiResult::OUT_OF_RANGE;
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if let Some(cell) = chunk.marked_rows.get(comp_idx as usize) {
        // SAFETY: `marked_rows[comp_idx]` is mutated solely inside this call on the active thread.
        let rows = unsafe { &mut *cell.get() };
        for r in start_row..(start_row + row_count) {
            rows.push(r);
        }
    }
    FfiResult::OK
}

/// Slot 16: `iter_column_rows_readonly`.
unsafe extern "C" fn host_iter_column_rows_readonly(
    iter: *mut BamIter,
    term_index: usize,
) -> *const *const u8 {
    if iter.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null();
    };
    if !term.mode.has_entity_column_data() {
        return core::ptr::null();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let chunk = &world.chunks[c_idx];
    let Some(Some(rows)) = chunk.sparse_columns.get(comp_idx as usize) else {
        return core::ptr::null();
    };
    state.ro_rows_scratch.clear();
    for row in rows {
        match row {
            Some(words) => state.ro_rows_scratch.push(
                UnsafeCell::raw_get(words.as_ptr())
                    .cast::<u8>()
                    .cast_const(),
            ),
            None => state.ro_rows_scratch.push(core::ptr::null()),
        }
    }
    state.ro_rows_scratch.as_ptr()
}

/// Slot 17: `iter_column_rows_mutable`.
unsafe extern "C" fn host_iter_column_rows_mutable(
    iter: *mut BamIter,
    term_index: usize,
) -> *const *mut u8 {
    if iter.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null();
    };
    if !term.mode.is_mutable() || !term.mode.has_entity_column_data() {
        return core::ptr::null();
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null();
    };
    if (meta.flags & BAM_COMPONENT_FLAG_IMMUTABLE) != 0 {
        return core::ptr::null();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    let chunk = &world.chunks[c_idx];
    let Some(Some(rows)) = chunk.sparse_columns.get(comp_idx as usize) else {
        return core::ptr::null();
    };
    state.mut_rows_scratch.clear();
    for row in rows {
        match row {
            Some(words) => state
                .mut_rows_scratch
                .push(UnsafeCell::raw_get(words.as_ptr()).cast::<u8>()),
            None => state.mut_rows_scratch.push(core::ptr::null_mut()),
        }
    }
    state.mut_rows_scratch.as_ptr()
}

/// Slot 18: `iter_column_info`.
unsafe extern "C" fn host_iter_column_info(
    iter: *const BamIter,
    term_index: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult {
    if iter.is_null() || out_info.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    // SAFETY: `out_info` is non-null writable `BamColumnInfo` storage.
    unsafe {
        *out_info = BamColumnInfo {
            component: term.component,
            mode: term.mode,
            uuid: meta.uuid,
            size: meta.size,
            align: meta.align,
            storage_flags: storage_flags_for_meta(meta, Some(term.mode)),
            reserved0: 0,
        };
    }
    FfiResult::OK
}

/// Slot 19: `res_ptr`.
unsafe extern "C" fn host_res_ptr(iter: *const BamIter, term_index: usize) -> *const u8 {
    if iter.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null();
    };
    if !term.mode.is_resource() {
        return core::ptr::null();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    world
        .resources
        .get(comp_idx as usize)
        .and_then(Option::as_ref)
        .map_or(core::ptr::null(), |slot| {
            UnsafeCell::raw_get(slot.words.as_ptr())
                .cast::<u8>()
                .cast_const()
        })
}

/// Slot 20: `res_mut_ptr`.
unsafe extern "C" fn host_res_mut_ptr(iter: *mut BamIter, term_index: usize) -> *mut u8 {
    if iter.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null_mut();
    };
    if term.mode != AccessMode::RESOURCE_WRITE {
        return core::ptr::null_mut();
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null_mut();
    };
    if (meta.flags & BAM_COMPONENT_FLAG_IMMUTABLE) != 0 {
        return core::ptr::null_mut();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null_mut();
    };
    let Some(Some(slot)) = world.resources.get(comp_idx as usize) else {
        return core::ptr::null_mut();
    };
    // SAFETY: Exclusive `RESOURCE_WRITE` term permits updating the resource's changed tick.
    unsafe {
        let ticks = &mut *slot.ticks.get();
        ticks.changed = ticks.this_run;
    }
    UnsafeCell::raw_get(slot.words.as_ptr()).cast::<u8>()
}

/// Slot 21: `cmd_spawn_empty`.
unsafe extern "C" fn host_cmd_spawn_empty(
    cmds: *mut BamCommands,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() || out_entity.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let entity = FfiEntity::from_bits(queue.next_entity_bits);
    queue.next_entity_bits += 1;
    queue.ops.push(RecordedCmd::SpawnEmpty { _entity: entity });
    // SAFETY: `out_entity` is writable `FfiEntity` storage.
    unsafe {
        *out_entity = entity;
    }
    FfiResult::OK
}

/// Slot 22 (and 38): `cmd_spawn` / `cmd_spawn_persistent`.
unsafe extern "C" fn host_cmd_spawn(
    cmds: *mut BamCommands,
    comps: *const BamComponent,
    data_ptrs: *const *const u8,
    count: usize,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() || out_entity.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if count > 0 && (comps.is_null() || data_ptrs.is_null()) {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    for i in 0..count {
        // SAFETY: `comps` and `data_ptrs` cover `count` readable entries when `count > 0`.
        let (comp, ptr) = unsafe { (*comps.add(i), *data_ptrs.add(i)) };
        let Some(meta) = queue.meta_for(comp) else {
            return FfiResult::INVALID_COMPONENT;
        };
        if meta.size > 0 && ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
    }
    let entity = FfiEntity::from_bits(queue.next_entity_bits);
    queue.next_entity_bits += 1;
    queue.ops.push(RecordedCmd::Spawn {
        _entity: entity,
        inline_inserts: count,
    });
    // SAFETY: `out_entity` is writable `FfiEntity` storage.
    unsafe {
        *out_entity = entity;
    }
    FfiResult::OK
}

/// Slot 23: `cmd_despawn`.
unsafe extern "C" fn host_cmd_despawn(cmds: *mut BamCommands, entity: FfiEntity) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    queue.ops.push(RecordedCmd::Despawn { _entity: entity });
    FfiResult::OK
}

/// Shared implementation for `cmd_insert` and `cmd_insert_if_new`.
unsafe fn cmd_insert_impl(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
    data_ptr: *const u8,
    data_len: usize,
    if_new: bool,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let Some(meta) = queue.meta_for(comp) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if meta.size != data_len {
        return FfiResult::TYPE_MISMATCH;
    }
    let mut inline_bytes = [0u8; 32];
    if data_len > 0 {
        if data_ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let copy_n = data_len.min(inline_bytes.len());
        // SAFETY: Caller guarantees `data_ptr` is readable for `data_len >= copy_n` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(data_ptr, inline_bytes.as_mut_ptr(), copy_n);
        }
    }
    let op = if if_new {
        RecordedCmd::InsertIfNew {
            _entity: entity,
            component: comp,
            byte_len: data_len,
            _inline_bytes: inline_bytes,
        }
    } else {
        RecordedCmd::Insert {
            _entity: entity,
            component: comp,
            byte_len: data_len,
            _inline_bytes: inline_bytes,
        }
    };
    queue.ops.push(op);
    FfiResult::OK
}

/// Slot 24: `cmd_insert`.
unsafe extern "C" fn host_cmd_insert(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    // SAFETY: Forwarding arguments to `cmd_insert_impl`.
    unsafe { cmd_insert_impl(cmds, entity, comp, data_ptr, data_len, false) }
}

/// Slot 25: `cmd_insert_default`.
unsafe extern "C" fn host_cmd_insert_default(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let Some(meta) = queue.meta_for(comp) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if !meta.has_default {
        return FfiResult::PERMISSION_DENIED;
    }
    queue.ops.push(RecordedCmd::InsertDefault {
        _entity: entity,
        _component: comp,
    });
    FfiResult::OK
}

/// Slot 26: `cmd_remove`.
unsafe extern "C" fn host_cmd_remove(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    if queue.meta_for(comp).is_none() {
        return FfiResult::INVALID_COMPONENT;
    }
    queue.ops.push(RecordedCmd::Remove {
        _entity: entity,
        _component: comp,
    });
    FfiResult::OK
}

/// Slot 27 (and 58): `par_for_each_chunk` / `par_for_each_chunk_readonly`.
unsafe fn par_for_each_chunk_impl(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    user_data: *mut c_void,
    readonly_only: bool,
) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(cb) = chunk_fn else {
        return FfiResult::NULL_POINTER;
    };
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &mut *iter.cast::<IterState>() };
    if state.in_par_iter {
        return FfiResult::ACCESS_CONFLICT;
    }
    if readonly_only && state.terms.iter().any(|t| t.mode.is_mutable()) {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world_len = unsafe { (*state.world).chunks.len() };
    let start = state.next_chunk_idx;
    state.next_chunk_idx = world_len;
    state.active_chunk_idx = None;
    for chunk_idx in start..world_len {
        let mut sub = IterState {
            world: state.world,
            sys_idx: state.sys_idx,
            query_idx: state.query_idx,
            terms: state.terms.clone(),
            start_chunk_idx: chunk_idx,
            next_chunk_idx: chunk_idx,
            end_chunk_idx: Some(chunk_idx + 1),
            active_chunk_idx: None,
            in_par_iter: true,
            ro_rows_scratch: Vec::new(),
            mut_rows_scratch: Vec::new(),
        };
        // SAFETY: `sub` is valid for the synchronous `cb` call and `cb`/`user_data` are `Sync`.
        let status = unsafe { cb((&raw mut sub).cast::<BamIter>(), user_data) };
        if !status.is_ok() {
            return status;
        }
    }
    FfiResult::OK
}

/// Slot 27: `par_for_each_chunk`.
unsafe extern "C" fn host_par_for_each_chunk(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    user_data: *mut c_void,
) -> FfiResult {
    // SAFETY: Forwarding arguments to `par_for_each_chunk_impl`.
    unsafe { par_for_each_chunk_impl(iter, chunk_fn, user_data, false) }
}

/// Slot 28: `sys_entity_get`.
unsafe extern "C" fn host_sys_entity_get(
    sys_ctx: *const BamSystemCtx,
    query_index: usize,
    entity: FfiEntity,
    term_index: usize,
) -> *const u8 {
    if sys_ctx.is_null() || entity.is_null() {
        return core::ptr::null();
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    if query_index >= frame.query_count {
        return core::ptr::null();
    }
    // SAFETY: `iter_ptrs[query_index]` points to a valid `IterState`.
    let iter_state = unsafe { &*frame.iter_ptrs[query_index] };
    let Some(term) = iter_state.terms.get(term_index) else {
        return core::ptr::null();
    };
    if !term.mode.has_entity_column_data() {
        return core::ptr::null();
    }
    // SAFETY: `frame.world` is valid during system execution.
    let world = unsafe { &*frame.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null();
    };
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    for chunk in &world.chunks {
        if let Some(row) = chunk.entities.iter().position(|&e| e == entity) {
            if !chunk_matches_terms(chunk, &iter_state.terms) {
                return core::ptr::null();
            }
            if let Some(Some(words)) = chunk.dense_columns.get(comp_idx as usize) {
                let base = UnsafeCell::raw_get(words.as_ptr())
                    .cast::<u8>()
                    .cast_const();
                // SAFETY: `row < chunk.entities.len()` and `words` contains `len * meta.size` bytes.
                return unsafe { base.add(row * meta.size) };
            }
            return core::ptr::null();
        }
    }
    core::ptr::null()
}

/// Slot 29: `sys_entity_get_mut`.
unsafe extern "C" fn host_sys_entity_get_mut(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    entity: FfiEntity,
    term_index: usize,
) -> *mut u8 {
    if sys_ctx.is_null() || entity.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    if query_index >= frame.query_count {
        return core::ptr::null_mut();
    }
    // SAFETY: `iter_ptrs[query_index]` points to a valid `IterState`.
    let iter_state = unsafe { &*frame.iter_ptrs[query_index] };
    let Some(term) = iter_state.terms.get(term_index) else {
        return core::ptr::null_mut();
    };
    if !term.mode.is_mutable() || !term.mode.has_entity_column_data() {
        return core::ptr::null_mut();
    }
    // SAFETY: `frame.world` is valid during system execution.
    let world = unsafe { &*frame.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null_mut();
    };
    if (meta.flags & BAM_COMPONENT_FLAG_IMMUTABLE) != 0 {
        return core::ptr::null_mut();
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return core::ptr::null_mut();
    };
    for chunk in &world.chunks {
        if let Some(row) = chunk.entities.iter().position(|&e| e == entity) {
            if !chunk_matches_terms(chunk, &iter_state.terms) {
                return core::ptr::null_mut();
            }
            chunk.mark_changed(term.component);
            if let Some(Some(words)) = chunk.dense_columns.get(comp_idx as usize) {
                let base = UnsafeCell::raw_get(words.as_ptr()).cast::<u8>();
                // SAFETY: `row < chunk.entities.len()` and `words` contains `len * meta.size` bytes.
                return unsafe { base.add(row * meta.size) };
            }
            return core::ptr::null_mut();
        }
    }
    core::ptr::null_mut()
}

/// Slot 30: `msg_read_batch`.
unsafe extern "C" fn host_msg_read_batch(
    iter: *mut BamIter,
    term_index: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    if iter.is_null() || out_read.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `out_read` is non-null writable `usize` storage.
    unsafe {
        *out_read = 0;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if !term.mode.is_message() {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    let elem_size = meta.size;
    if max_count > 0 && elem_size > 0 && out_buf.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `messages` and `msg_cursors` are accessed on the active system thread.
    let (messages, cursors) = unsafe { (&*world.messages.get(), &mut *world.msg_cursors.get()) };
    let cursor_key = (state.sys_idx, state.query_idx, term_index);
    let cursor = cursors.get(&cursor_key).copied().unwrap_or(0);
    let Some(stream) = messages.get(&term.component) else {
        return FfiResult::OK;
    };
    let avail = stream.len().saturating_sub(cursor);
    let to_copy = avail.min(max_count);
    for i in 0..to_copy {
        if elem_size > 0 {
            let bytes = &stream[cursor + i];
            // SAFETY: `out_buf` has `max_count * elem_size` writable bytes and `bytes` has `elem_size`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    out_buf.add(i * elem_size).cast::<MaybeUninit<u8>>(),
                    elem_size,
                );
            }
        }
    }
    cursors.insert(cursor_key, cursor + to_copy);
    // SAFETY: `out_read` is non-null writable `usize` storage.
    unsafe {
        *out_read = to_copy;
    }
    FfiResult::OK
}

/// Slot 31: `msg_send`.
unsafe extern "C" fn host_msg_send(
    iter: *mut BamIter,
    term_index: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if term.mode != AccessMode::MESSAGE_WRITE {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if meta.size != data_len {
        return FfiResult::TYPE_MISMATCH;
    }
    let mut entry = vec![MaybeUninit::uninit(); data_len];
    if data_len > 0 {
        if data_ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
        // SAFETY: Caller guarantees `data_ptr` is readable for `data_len` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data_ptr.cast::<MaybeUninit<u8>>(),
                entry.as_mut_ptr(),
                data_len,
            );
        }
    }
    // SAFETY: `messages` is accessed exclusively here on the active system thread.
    unsafe {
        (*world.messages.get())
            .entry(term.component)
            .or_default()
            .push(entry);
    }
    world.shared.msg_send_calls.fetch_add(1, Ordering::Relaxed);
    FfiResult::OK
}

/// Slot 32: `msg_write_batch`.
unsafe extern "C" fn host_msg_write_batch(
    iter: *mut BamIter,
    term_index: usize,
    in_buf: *const u8,
    count: usize,
) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if term.mode != AccessMode::MESSAGE_WRITE {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if count == 0 {
        return FfiResult::OK;
    }
    let elem_size = meta.size;
    if elem_size > 0 && in_buf.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `messages` is accessed exclusively here on the active system thread.
    let stream = unsafe { (*world.messages.get()).entry(term.component).or_default() };
    for i in 0..count {
        let mut entry = vec![MaybeUninit::uninit(); elem_size];
        if elem_size > 0 {
            // SAFETY: Caller guarantees `in_buf` covers `count * elem_size` readable bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    in_buf.add(i * elem_size).cast::<MaybeUninit<u8>>(),
                    entry.as_mut_ptr(),
                    elem_size,
                );
            }
        }
        stream.push(entry);
    }
    world
        .shared
        .msg_send_calls
        .fetch_add(count as u64, Ordering::Relaxed);
    FfiResult::OK
}

/// Slot 33: `res_ticks`.
unsafe extern "C" fn host_res_ticks(
    iter: *const BamIter,
    term_index: usize,
    out_ticks: *mut BamChangeTicks,
) -> FfiResult {
    if iter.is_null() || out_ticks.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if !term.mode.is_resource() {
        return FfiResult::PERMISSION_DENIED;
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return FfiResult::INVALID_COMPONENT;
    };
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(Some(slot)) = world.resources.get(comp_idx as usize) else {
        return FfiResult::NOT_FOUND;
    };
    // SAFETY: `slot.ticks` holds initialized `BamChangeTicks` and `out_ticks` is writable.
    unsafe {
        *out_ticks = *slot.ticks.get();
    }
    world.shared.res_ticks_calls.fetch_add(1, Ordering::Relaxed);
    FfiResult::OK
}

/// Slot 34: `sys_res_ptr`.
unsafe extern "C" fn host_sys_res_ptr(
    sys_ctx: *const BamSystemCtx,
    component: BamComponent,
    out_ticks: *mut BamChangeTicks,
) -> *const u8 {
    if sys_ctx.is_null() || !component.is_valid() {
        return core::ptr::null();
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    // SAFETY: `frame.world` is valid during system execution.
    let world = unsafe { &*frame.world };
    let declared = world.systems[frame.sys_idx]
        .queries
        .iter()
        .flatten()
        .any(|t| t.component == component && t.mode.is_resource());
    if !declared {
        return core::ptr::null();
    }
    let Some(comp_idx) = component.to_raw().checked_sub(1) else {
        return core::ptr::null();
    };
    let Some(Some(slot)) = world.resources.get(comp_idx as usize) else {
        return core::ptr::null();
    };
    if !out_ticks.is_null() {
        // SAFETY: `out_ticks` is non-null writable `BamChangeTicks` storage.
        unsafe {
            *out_ticks = *slot.ticks.get();
        }
    }
    UnsafeCell::raw_get(slot.words.as_ptr())
        .cast::<u8>()
        .cast_const()
}

/// Slot 35: `sys_res_mut_ptr`.
unsafe extern "C" fn host_sys_res_mut_ptr(
    sys_ctx: *mut BamSystemCtx,
    component: BamComponent,
    out_ticks: *mut BamChangeTicks,
) -> *mut u8 {
    if sys_ctx.is_null() || !component.is_valid() {
        return core::ptr::null_mut();
    }
    // SAFETY: `sys_ctx` points to the active `SystemFrame`.
    let frame = unsafe { &*sys_ctx.cast::<SystemFrame>() };
    // SAFETY: `frame.world` is valid during system execution.
    let world = unsafe { &*frame.world };
    let Some(meta) = world.comp_meta(component) else {
        return core::ptr::null_mut();
    };
    if (meta.flags & BAM_COMPONENT_FLAG_IMMUTABLE) != 0 {
        return core::ptr::null_mut();
    }
    let declared = world.systems[frame.sys_idx]
        .queries
        .iter()
        .flatten()
        .any(|t| t.component == component && t.mode == AccessMode::RESOURCE_WRITE);
    if !declared {
        return core::ptr::null_mut();
    }
    let Some(comp_idx) = component.to_raw().checked_sub(1) else {
        return core::ptr::null_mut();
    };
    let Some(Some(slot)) = world.resources.get(comp_idx as usize) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `slot.ticks` is exclusively accessed under `RESOURCE_WRITE`.
    unsafe {
        let ticks = &mut *slot.ticks.get();
        if !out_ticks.is_null() {
            *out_ticks = *ticks;
        }
        ticks.changed = ticks.this_run;
    }
    UnsafeCell::raw_get(slot.words.as_ptr()).cast::<u8>()
}

/// Slot 36: `register_observer`.
unsafe extern "C" fn host_register_observer(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamObserverDesc,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `BamObserverDesc` prefix.
    match unsafe { read_descriptor::<BamObserverDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d.validate(),
        Err(err) => err,
    }
}

/// Slot 37: `cmd_trigger`.
unsafe extern "C" fn host_cmd_trigger(
    cmds: *mut BamCommands,
    event: BamComponent,
    _target: FfiEntity,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let Some(meta) = queue.meta_for(event) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if meta.size != data_len {
        return FfiResult::TYPE_MISMATCH;
    }
    if data_len > 0 && data_ptr.is_null() {
        return FfiResult::NULL_POINTER;
    }
    queue.ops.push(RecordedCmd::Trigger);
    FfiResult::OK
}

/// Slot 39: `cmd_spawn_batch`.
unsafe extern "C" fn host_cmd_spawn_batch(
    cmds: *mut BamCommands,
    components: *const BamComponent,
    column_ptrs: *const *const u8,
    component_count: usize,
    entity_count: usize,
    out_entities: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if component_count > 0 && (components.is_null() || column_ptrs.is_null()) {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    for i in 0..component_count {
        // SAFETY: `components` and `column_ptrs` cover `component_count` readable entries.
        let (comp, ptr) = unsafe { (*components.add(i), *column_ptrs.add(i)) };
        let Some(meta) = queue.meta_for(comp) else {
            return FfiResult::INVALID_COMPONENT;
        };
        if entity_count > 0 && meta.size > 0 && ptr.is_null() {
            return FfiResult::NULL_POINTER;
        }
    }
    if !out_entities.is_null() {
        for i in 0..entity_count {
            let e = FfiEntity::from_bits(queue.next_entity_bits);
            queue.next_entity_bits += 1;
            // SAFETY: Caller guarantees `out_entities` is writable for `entity_count` entries.
            unsafe {
                *out_entities.add(i) = e;
            }
        }
    } else {
        queue.next_entity_bits += entity_count as u64;
    }
    queue.ops.push(RecordedCmd::SpawnBatch {
        entity_count,
        component_count,
    });
    FfiResult::OK
}

/// Slot 40: `cmd_despawn_all`.
unsafe extern "C" fn host_cmd_despawn_all(
    cmds: *mut BamCommands,
    terms: *const BamTermDesc,
    term_count: usize,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if term_count == 0 {
        return FfiResult::INVALID_ARGUMENT;
    }
    if terms.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: Caller guarantees `terms` covers `term_count` readable `BamTermDesc`s.
    let slice = unsafe { core::slice::from_raw_parts(terms, term_count) };
    for term in slice {
        let status = term.validate();
        if !status.is_ok() {
            return status;
        }
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    queue.ops.push(RecordedCmd::DespawnAll);
    FfiResult::OK
}

/// Slot 41: `cmd_insert_if_new`.
unsafe extern "C" fn host_cmd_insert_if_new(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    // SAFETY: Forwarding arguments to `cmd_insert_impl`.
    unsafe { cmd_insert_impl(cmds, entity, comp, data_ptr, data_len, true) }
}

/// Slot 42: `cmd_trigger_ex`.
unsafe extern "C" fn host_cmd_trigger_ex(
    cmds: *mut BamCommands,
    desc: *const BamTriggerDesc,
) -> FfiResult {
    if cmds.is_null() || desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `BamTriggerDesc` prefix.
    let d = match unsafe { read_descriptor::<BamTriggerDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d,
        Err(err) => return err,
    };
    let status = d.validate();
    if !status.is_ok() {
        return status;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    queue.ops.push(RecordedCmd::Trigger);
    FfiResult::OK
}

/// Slot 43: `cmd_observe`.
unsafe extern "C" fn host_cmd_observe(
    cmds: *mut BamCommands,
    desc: *const BamObserverDesc,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() || desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `BamObserverDesc` prefix.
    let d = match unsafe { read_descriptor::<BamObserverDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d,
        Err(err) => return err,
    };
    let status = d.validate();
    if !status.is_ok() {
        return status;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let e = FfiEntity::from_bits(queue.next_entity_bits);
    queue.next_entity_bits += 1;
    queue.ops.push(RecordedCmd::Observe);
    if !out_entity.is_null() {
        // SAFETY: Non-null `out_entity` is writable `FfiEntity` storage.
        unsafe {
            *out_entity = e;
        }
    }
    FfiResult::OK
}

/// Slot 44: `iter_read_removed`.
unsafe extern "C" fn host_iter_read_removed(
    iter: *mut BamIter,
    term_index: usize,
    _out_entities: *mut FfiEntity,
    _max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    if iter.is_null() || out_read.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if !term.mode.is_removed_read() {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `out_read` is non-null writable `usize` storage.
    unsafe {
        *out_read = 0;
    }
    FfiResult::OK
}

/// Slot 45: `sys_entity_children`.
unsafe extern "C" fn host_sys_entity_children(
    sys_ctx: *const BamSystemCtx,
    entity: FfiEntity,
    _relationship_target: BamComponent,
    out_entities: *mut *const FfiEntity,
    out_len: *mut usize,
) -> FfiResult {
    if sys_ctx.is_null() || out_entities.is_null() || out_len.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `out_entities` and `out_len` are writable storage.
    unsafe {
        *out_entities = core::ptr::null();
        *out_len = 0;
    }
    FfiResult::OK
}

/// Slot 46: `asset_load`.
unsafe extern "C" fn host_asset_load(
    ctx: *const BamHostCtx,
    _asset_type: BamComponent,
    _path: StrView,
    _out_handle: *mut FfiAssetHandle,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    FfiResult::UNSUPPORTED
}

/// Slot 47: `asset_get_load_state`.
unsafe extern "C" fn host_asset_get_load_state(
    ctx: *const BamHostCtx,
    _handle: *const FfiAssetHandle,
    _out_state: *mut FfiAssetLoadState,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    FfiResult::UNSUPPORTED
}

/// Slot 48: `asset_release`.
unsafe extern "C" fn host_asset_release(
    ctx: *const BamHostCtx,
    _handle: *const FfiAssetHandle,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    FfiResult::UNSUPPORTED
}

/// Slot 49: `cmd_spawn_scene`.
unsafe extern "C" fn host_cmd_spawn_scene(
    cmds: *mut BamCommands,
    _scene_handle: *const FfiAssetHandle,
    _parent: FfiEntity,
    _out_entity: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    FfiResult::UNSUPPORTED
}

/// Slot 50: `log_diagnostic`.
unsafe extern "C" fn host_log_diagnostic(
    ctx: *const BamHostCtx,
    _level: u32,
    message: StrView,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `message` is borrowed for this synchronous call per `BamLogDiagnosticFn` contract.
    if let Err(err) = unsafe { message.try_as_str() } {
        return err;
    }
    // SAFETY: `ctx` points to an active `HostSharedState`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    shared.diagnostic_logs.fetch_add(1, Ordering::Relaxed);
    FfiResult::OK
}

/// Slot 51: `report_guest_fault`.
unsafe extern "C" fn host_report_guest_fault(
    ctx: *const BamHostCtx,
    message: StrView,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `message` is borrowed for this synchronous call per `BamReportGuestFaultFn` contract.
    let msg = match unsafe { message.try_as_str() } {
        Ok(s) => s,
        Err(err) => return err,
    };
    // SAFETY: `ctx` points to an active `HostSharedState`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    {
        let mut faults = shared
            .faults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        faults.push(msg.to_owned());
    }
    {
        let mut last = shared
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        last.clear();
        last.push_str(msg);
    }
    FfiResult::OK
}

/// Slot 52: `resolve_component_by_uuid`.
unsafe extern "C" fn host_resolve_component_by_uuid(
    ctx: *const BamHostCtx,
    uuid: *const ComponentUuid,
    out_component: *mut BamComponent,
) -> FfiResult {
    if ctx.is_null() || uuid.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: Caller guarantees `uuid` points to a readable `ComponentUuid`.
    let key = unsafe { *uuid };
    if key.is_zero() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `ctx` points to `HostWorld::shared` of a live `HostWorld`.
    let world = unsafe { world_from_host_ctx(ctx) };
    let Some(&handle) = world.uuid_to_handle.get(&key) else {
        return FfiResult::NOT_FOUND;
    };
    // SAFETY: `out_component` is writable `BamComponent` storage.
    unsafe {
        *out_component = handle;
    }
    FfiResult::OK
}

/// Slot 53: `component_layout_info`.
unsafe extern "C" fn host_component_layout_info(
    ctx: *const BamHostCtx,
    component: BamComponent,
    out_info: *mut BamLayoutInfo,
) -> FfiResult {
    if ctx.is_null() || out_info.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `ctx` points to `HostWorld::shared` of a live `HostWorld`.
    let world = unsafe { world_from_host_ctx(ctx) };
    let Some(meta) = world.comp_meta(component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    // SAFETY: `out_info` is writable `BamLayoutInfo` storage.
    unsafe {
        *out_info = BamLayoutInfo {
            struct_size: core::mem::size_of::<BamLayoutInfo>() as u32,
            flags: storage_flags_for_meta(meta, None),
            uuid: meta.uuid,
            schema_fingerprint: meta.schema_fingerprint,
            size: meta.size,
            align: meta.align,
            fields: core::ptr::null(),
            field_count: 0,
        };
    }
    FfiResult::OK
}

/// Slot 54: `register_relation`.
unsafe extern "C" fn host_register_relation(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamRelationDesc,
    out_component: *mut BamComponent,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `BamRelationDesc` prefix.
    let d = match unsafe { read_descriptor::<BamRelationDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d,
        Err(err) => return err,
    };
    let status = d.validate();
    if !status.is_ok() {
        return status;
    }
    // SAFETY: `d.validate()` verified `d.name` is non-null.
    let Ok(name_str) = (unsafe { CStr::from_ptr(d.name) }).to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    if name_str.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `load_ctx` points to an exclusively held `HostWorld` during `bamabi_init`.
    let world = unsafe { &mut *load_ctx.cast::<HostWorld>() };
    if let Some(&existing) = world.name_to_handle.get(name_str) {
        // SAFETY: `out_component` is writable `BamComponent` storage.
        unsafe {
            *out_component = existing;
        }
        return FfiResult::OK;
    }
    let raw_id = (world.components.len() + 1) as u32;
    let handle = BamComponent::from_raw(raw_id);
    world.components.push(RegisteredComp {
        _handle: handle,
        _name: name_str.to_owned(),
        uuid: d.uuid,
        schema_fingerprint: 0,
        size: d.data_size,
        align: d.data_align,
        flags: 0,
        is_resource: false,
        _has_default: false,
        _drop_fn: None,
        _vtable_ptr: core::ptr::null(),
        _on_add: None,
        _on_remove: None,
        _hook_user_data: core::ptr::null_mut(),
    });
    world.name_to_handle.insert(name_str.to_owned(), handle);
    if !d.uuid.is_zero() {
        world.uuid_to_handle.insert(d.uuid, handle);
    }
    world.resources.push(None);
    world.commands.get_mut().comp_meta.push(CompCommandMeta {
        size: d.data_size,
        has_default: false,
    });
    // SAFETY: `out_component` is writable `BamComponent` storage.
    unsafe {
        *out_component = handle;
    }
    FfiResult::OK
}

/// Slot 55: `intern_symbol`.
unsafe extern "C" fn host_intern_symbol(
    ctx: *const BamHostCtx,
    symbol: StrView,
    out_symbol: *mut BamSymbol,
) -> FfiResult {
    if ctx.is_null() || out_symbol.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `symbol` points to valid bytes for the duration of this synchronous call.
    let sym_str = match unsafe { symbol.try_as_str() } {
        Ok(s) => s,
        Err(err) => return err,
    };
    if sym_str.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `ctx` points to an active `HostSharedState`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    let sym = shared.intern_symbol_str(sym_str);
    // SAFETY: `out_symbol` is writable `BamSymbol` storage.
    unsafe {
        *out_symbol = sym;
    }
    FfiResult::OK
}

/// Slot 56: `bind_named_object`.
unsafe extern "C" fn host_bind_named_object(
    ctx: *const BamHostCtx,
    name: StrView,
    entity: FfiEntity,
) -> FfiResult {
    if ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `name` points to valid bytes for the duration of this synchronous call.
    let name_str = match unsafe { name.try_as_str() } {
        Ok(s) => s,
        Err(err) => return err,
    };
    if name_str.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `ctx` points to an active `HostSharedState`.
    let shared = unsafe { &*ctx.cast::<HostSharedState>() };
    shared
        .named_objects
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(name_str.to_owned(), entity);
    FfiResult::OK
}

/// Slot 57: `iter_count`.
unsafe extern "C" fn host_iter_count(iter: *mut BamIter, out_count: *mut usize) -> FfiResult {
    if iter.is_null() || out_count.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let start = state.start_chunk_idx;
    let end = state
        .end_chunk_idx
        .map_or(world.chunks.len(), |e| e.min(world.chunks.len()));
    let mut total = 0usize;
    for chunk in &world.chunks[start..end] {
        if !chunk.entities.is_empty() && chunk_matches_terms(chunk, &state.terms) {
            total += chunk.entities.len();
        }
    }
    // SAFETY: `out_count` is writable `usize` storage.
    unsafe {
        *out_count = total;
    }
    FfiResult::OK
}

/// Slot 58: `iter_up_cascade`.
unsafe extern "C" fn host_iter_up_cascade(
    iter: *mut BamIter,
    _relation: BamComponent,
    entity: FfiEntity,
    term_index: usize,
    out_ptr: *mut *const c_void,
    out_owner: *mut FfiEntity,
) -> FfiResult {
    if iter.is_null() || out_ptr.is_null() || out_owner.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    if term_index >= state.terms.len() {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: `out_ptr` and `out_owner` are writable storage.
    unsafe {
        *out_ptr = core::ptr::null();
        *out_owner = FfiEntity::NULL;
    }
    FfiResult::NOT_FOUND
}

/// Slot 59: `par_for_each_chunk_readonly`.
unsafe extern "C" fn host_par_for_each_chunk_readonly(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    user_data: *mut c_void,
) -> FfiResult {
    // SAFETY: Forwarding arguments to `par_for_each_chunk_impl` with `readonly_only = true`.
    unsafe { par_for_each_chunk_impl(iter, chunk_fn, user_data, true) }
}

/// Slot 60: `register_system_handle`.
unsafe extern "C" fn host_register_system_handle(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
    out_system: *mut BamSystem,
) -> FfiResult {
    if out_system.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: Caller contract matches `register_system_impl`.
    match unsafe { register_system_impl(load_ctx, desc) } {
        Ok(sys) => {
            // SAFETY: `out_system` is non-null writable `BamSystem` storage.
            unsafe {
                *out_system = sys;
            }
            FfiResult::OK
        }
        Err(err) => err,
    }
}

/// Slot 61: `configure_schedule_set`.
unsafe extern "C" fn host_configure_schedule_set(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamScheduleSetDesc,
    out_set: *mut BamSystemSet,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_set.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `desc` points to a readable `BamScheduleSetDesc` prefix.
    let d = match unsafe { read_descriptor::<BamScheduleSetDesc>(desc.cast::<c_void>()) } {
        Ok(d) => d,
        Err(err) => return err,
    };
    let status = d.validate();
    if !status.is_ok() {
        return status;
    }
    // SAFETY: `d.validate()` verified `d.name` is non-null.
    let Ok(name_str) = (unsafe { CStr::from_ptr(d.name) }).to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    if name_str.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `load_ctx` points to an exclusively held `HostWorld` during `bamabi_init`.
    let world = unsafe { &mut *load_ctx.cast::<HostWorld>() };
    let handle = if let Some(idx) = world.schedule_sets.iter().position(|s| s == name_str) {
        BamSystemSet::from_raw(idx as u32)
    } else {
        let idx = world.schedule_sets.len();
        world.schedule_sets.push(name_str.to_owned());
        BamSystemSet::from_raw(idx as u32)
    };
    // SAFETY: `out_set` is non-null writable `BamSystemSet` storage.
    unsafe {
        *out_set = handle;
    }
    FfiResult::OK
}

/// Slot 62: `mount_system_in_schedule`.
unsafe extern "C" fn host_mount_system_in_schedule(
    load_ctx: *mut BamLoadCtx,
    system: BamSystem,
    schedule: ScheduleId,
    _set: BamSystemSet,
) -> FfiResult {
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if !system.is_valid() || !schedule.is_valid() {
        return FfiResult::INVALID_ARGUMENT;
    }
    // SAFETY: `load_ctx` points to an exclusively held `HostWorld` during `bamabi_init`.
    let world = unsafe { &mut *load_ctx.cast::<HostWorld>() };
    let Some(sys) = world.systems.get_mut(system.to_raw() as usize) else {
        return FfiResult::NOT_FOUND;
    };
    sys.schedule = schedule;
    FfiResult::OK
}

/// Slot 63: `cmd_checkpoint`.
unsafe extern "C" fn host_cmd_checkpoint(
    cmds: *mut BamCommands,
    out_checkpoint: *mut usize,
) -> FfiResult {
    if cmds.is_null() || out_checkpoint.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let token = queue.checkpoint();
    // SAFETY: `out_checkpoint` is non-null writable `usize` storage.
    unsafe {
        *out_checkpoint = token;
    }
    FfiResult::OK
}

/// Slot 64: `cmd_rollback`.
unsafe extern "C" fn host_cmd_rollback(cmds: *mut BamCommands, checkpoint: usize) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    queue.rollback(checkpoint)
}

/// Slot 65: `cmd_insert_relation`.
unsafe extern "C" fn host_cmd_insert_relation(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    relation: BamComponent,
    target: FfiEntity,
    data: *const c_void,
    data_size: usize,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() || target.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let Some(meta) = queue.meta_for(relation) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if meta.size != data_size {
        return FfiResult::TYPE_MISMATCH;
    }
    if data_size > 0 && data.is_null() {
        return FfiResult::NULL_POINTER;
    }
    queue.ops.push(RecordedCmd::InsertRelation);
    FfiResult::OK
}

/// Slot 66: `cmd_remove_relation`.
unsafe extern "C" fn host_cmd_remove_relation(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    relation: BamComponent,
    target: FfiEntity,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() || target.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    if queue.meta_for(relation).is_none() {
        return FfiResult::INVALID_COMPONENT;
    }
    queue.ops.push(RecordedCmd::RemoveRelation);
    FfiResult::OK
}

/// Slot 67: `migrate_component_column`.
unsafe extern "C" fn host_migrate_component_column(
    load_ctx: *mut BamLoadCtx,
    _component: BamComponent,
    _old_fingerprint: u64,
    _new_desc: *const ComponentDesc,
    _migrate_fn: Option<BamRowMigrateFn>,
    _user_data: *mut c_void,
) -> FfiResult {
    if load_ctx.is_null() {
        return FfiResult::NULL_POINTER;
    }
    FfiResult::UNSUPPORTED
}

/// Slot 69: `iter_column_ticks`.
unsafe extern "C" fn host_iter_column_ticks(
    iter: *const BamIter,
    term_index: usize,
    out_added: *mut *const u32,
    out_changed: *mut *const u32,
    out_last_run: *mut u32,
    out_this_run: *mut u32,
) -> FfiResult {
    if iter.is_null()
        || out_added.is_null()
        || out_changed.is_null()
        || out_last_run.is_null()
        || out_this_run.is_null()
    {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `iter` points to an active `IterState`.
    let state = unsafe { &*iter.cast::<IterState>() };
    let Some(c_idx) = state.active_chunk_idx else {
        return FfiResult::INVALID_STATE;
    };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if !term.mode.has_entity_column_data() {
        return FfiResult::PERMISSION_DENIED;
    }
    // SAFETY: `state.world` is valid during system execution.
    let world = unsafe { &*state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if (meta.flags & BAM_COMPONENT_FLAG_SPARSE_SET) != 0 {
        return FfiResult::UNSUPPORTED;
    }
    let Some(comp_idx) = term.component.to_raw().checked_sub(1) else {
        return FfiResult::INVALID_COMPONENT;
    };
    let chunk = &world.chunks[c_idx];
    if chunk
        .dense_columns
        .get(comp_idx as usize)
        .and_then(Option::as_ref)
        .is_none()
    {
        return FfiResult::NOT_FOUND;
    }
    // SAFETY: All out-pointers were verified non-null and point to valid writable storage.
    // `chunk.added_ticks` and `chunk.changed_ticks` each contain `chunk.entities.len()` `u32`
    // entries and remain valid for the duration of system execution.
    unsafe {
        *out_added = chunk.added_ticks.as_ptr();
        *out_changed = chunk.changed_ticks.as_ptr();
        *out_last_run = 0;
        *out_this_run = 1;
    }
    FfiResult::OK
}

/// Slot 70: `cmd_clone_entity`.
unsafe extern "C" fn host_cmd_clone_entity(
    cmds: *mut BamCommands,
    src_entity: FfiEntity,
    dst_entity: FfiEntity,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if src_entity.is_null() || dst_entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    queue.ops.push(RecordedCmd::CloneEntity {
        _src: src_entity,
        _dst: dst_entity,
    });
    FfiResult::OK
}

/// Slot 71: `cmd_insert_resource`.
unsafe extern "C" fn host_cmd_insert_resource(
    cmds: *mut BamCommands,
    resource: BamComponent,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    let Some(meta) = queue.meta_for(resource) else {
        return FfiResult::INVALID_COMPONENT;
    };
    if data_len > 0 && data_ptr.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if meta.size != data_len {
        return FfiResult::TYPE_MISMATCH;
    }
    queue.ops.push(RecordedCmd::InsertResource {
        _resource: resource,
        _byte_len: data_len,
    });
    FfiResult::OK
}

/// Slot 72: `cmd_remove_resource`.
unsafe extern "C" fn host_cmd_remove_resource(
    cmds: *mut BamCommands,
    resource: BamComponent,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `cmds` points to the system's active `CommandQueue`.
    let queue = unsafe { &mut *cmds.cast::<CommandQueue>() };
    if queue.meta_for(resource).is_none() {
        return FfiResult::INVALID_COMPONENT;
    }
    queue.ops.push(RecordedCmd::RemoveResource {
        _resource: resource,
    });
    FfiResult::OK
}

/// Constructs a complete 73-slot ABI `0.3.0` [`BamHostApi`] table wired to [`HostWorld`].
pub(crate) fn make_host_api() -> BamHostApi {
    let api = BamHostApi {
        struct_size: core::mem::size_of::<BamHostApi>() as u32,
        abi_version: ABI_VERSION,
        get_proc_address: Some(host_get_proc_address),
        last_error: Some(host_last_error),
        load_host_ctx: Some(host_load_host_ctx),
        register_component: Some(host_register_component),
        resolve_component: Some(host_resolve_component),
        register_system: Some(host_register_system),
        sys_host_ctx: Some(host_sys_host_ctx),
        sys_query: Some(host_sys_query),
        sys_commands: Some(host_sys_commands),
        iter_next_chunk: Some(host_iter_next_chunk),
        iter_reset: Some(host_iter_reset),
        iter_entities: Some(host_iter_entities),
        iter_column_readonly: Some(host_iter_column_readonly),
        iter_column_mutable: Some(host_iter_column_mutable),
        iter_column_mutable_untracked: Some(host_iter_column_mutable_untracked),
        iter_mark_changed_range: Some(host_iter_mark_changed_range),
        iter_column_rows_readonly: Some(host_iter_column_rows_readonly),
        iter_column_rows_mutable: Some(host_iter_column_rows_mutable),
        iter_column_info: Some(host_iter_column_info),
        res_ptr: Some(host_res_ptr),
        res_mut_ptr: Some(host_res_mut_ptr),
        cmd_spawn_empty: Some(host_cmd_spawn_empty),
        cmd_spawn: Some(host_cmd_spawn),
        cmd_despawn: Some(host_cmd_despawn),
        cmd_insert: Some(host_cmd_insert),
        cmd_insert_default: Some(host_cmd_insert_default),
        cmd_remove: Some(host_cmd_remove),
        par_for_each_chunk: Some(host_par_for_each_chunk),
        sys_entity_get: Some(host_sys_entity_get),
        sys_entity_get_mut: Some(host_sys_entity_get_mut),
        msg_read_batch: Some(host_msg_read_batch),
        msg_send: Some(host_msg_send),
        msg_write_batch: Some(host_msg_write_batch),
        res_ticks: Some(host_res_ticks),
        sys_res_ptr: Some(host_sys_res_ptr),
        sys_res_mut_ptr: Some(host_sys_res_mut_ptr),
        register_observer: Some(host_register_observer),
        cmd_trigger: Some(host_cmd_trigger),
        cmd_spawn_persistent: Some(host_cmd_spawn),
        cmd_spawn_batch: Some(host_cmd_spawn_batch),
        cmd_despawn_all: Some(host_cmd_despawn_all),
        cmd_insert_if_new: Some(host_cmd_insert_if_new),
        cmd_trigger_ex: Some(host_cmd_trigger_ex),
        cmd_observe: Some(host_cmd_observe),
        iter_read_removed: Some(host_iter_read_removed),
        sys_entity_children: Some(host_sys_entity_children),
        asset_load: Some(host_asset_load),
        asset_get_load_state: Some(host_asset_get_load_state),
        asset_release: Some(host_asset_release),
        cmd_spawn_scene: Some(host_cmd_spawn_scene),
        log_diagnostic: Some(host_log_diagnostic),
        report_guest_fault: Some(host_report_guest_fault),
        resolve_component_by_uuid: Some(host_resolve_component_by_uuid),
        component_layout_info: Some(host_component_layout_info),
        register_relation: Some(host_register_relation),
        intern_symbol: Some(host_intern_symbol),
        bind_named_object: Some(host_bind_named_object),
        iter_count: Some(host_iter_count),
        iter_up_cascade: Some(host_iter_up_cascade),
        par_for_each_chunk_readonly: Some(host_par_for_each_chunk_readonly),
        register_system_handle: Some(host_register_system_handle),
        configure_schedule_set: Some(host_configure_schedule_set),
        mount_system_in_schedule: Some(host_mount_system_in_schedule),
        cmd_checkpoint: Some(host_cmd_checkpoint),
        cmd_rollback: Some(host_cmd_rollback),
        cmd_insert_relation: Some(host_cmd_insert_relation),
        cmd_remove_relation: Some(host_cmd_remove_relation),
        migrate_component_column: Some(host_migrate_component_column),
        iter_next_chunk_batch: Some(host_iter_next_chunk_batch),
        iter_column_ticks: Some(host_iter_column_ticks),
        cmd_clone_entity: Some(host_cmd_clone_entity),
        cmd_insert_resource: Some(host_cmd_insert_resource),
        cmd_remove_resource: Some(host_cmd_remove_resource),
    };
    debug_assert!(api.has_all_slots());
    api
}
