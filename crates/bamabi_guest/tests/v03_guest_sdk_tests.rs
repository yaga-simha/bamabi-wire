//! Comprehensive ABI `0.3.0` mock-host unit and integration tests for `bamabi_guest` and
//! `bamabi_guest_sys` (also executed under Miri Tree Borrows).

#![allow(
    dead_code,
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    clippy::vec_box
)]

use core::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};
use std::collections::{HashMap, HashSet};

use bamabi_guest::{
    build_guest_fn,
    prelude::*,
    raw::{
        BamChunkTaskFn, BamCommands, BamHostApi, BamHostCtx, BamInstance, BamIter, BamLoadCtx,
        BamRunIfFn, BamSystemCtx, BamSystemDesc, BamSystemFn, BamTermDesc, ComponentDesc,
        SymbolEntry, init_guest_instance, shutdown_guest_instance,
    },
};

// ============================================================================
// Mock ABI 0.3.0 Host World
// ============================================================================

/// Registered component metadata tracked by [`MockV03World`] (including ABI 0.3.0 schema fingerprint).
#[derive(Clone)]
struct RegisteredComp {
    handle: BamComponent,
    name: String,
    uuid: ComponentUuid,
    schema_fingerprint: u64,
    size: usize,
    align: usize,
    flags: u32,
    fields: Vec<BamFieldDesc>,
}

/// Registered ABI 0.3.0 typed relationship metadata stored in [`MockV03World`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredRelation {
    handle: BamComponent,
    name: String,
    uuid: ComponentUuid,
    flags: u32,
    data_size: usize,
    data_align: usize,
    max_degree: u32,
}

/// Registered system descriptor, callback, and mount metadata in [`MockV03World`].
#[derive(Clone)]
struct RegisteredSystem {
    handle: BamSystem,
    name: String,
    queries: Vec<Vec<BamTermDesc>>,
    schedule: ScheduleId,
    callback: BamSystemFn,
    user_data: *mut c_void,
    run_if: Option<BamRunIfFn>,
    run_if_user_data: *mut c_void,
    run_if_ctx: Option<BamSystemFn>,
    run_if_query: Vec<BamTermDesc>,
    run_if_ctx_user_data: *mut c_void,
}

/// Registered ABI 0.3.0 schedule system-set configuration stored in [`MockV03World`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredScheduleSet {
    handle: BamSystemSet,
    name: String,
    schedule: ScheduleId,
    flags: u32,
    before_sets: Vec<BamSystemSet>,
    after_sets: Vec<BamSystemSet>,
}

/// Recorded deferred command entry supporting transaction checkpoints and rollbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecordedCmd {
    SpawnEmpty(FfiEntity),
    Despawn(FfiEntity),
    Insert {
        entity: FfiEntity,
        component: BamComponent,
        bytes: Vec<u8>,
    },
    Remove {
        entity: FfiEntity,
        component: BamComponent,
    },
    InsertRelation {
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
        payload: Vec<u8>,
    },
    RemoveRelation {
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    },
    CloneEntity {
        src: FfiEntity,
        dst: FfiEntity,
    },
    InsertResource {
        resource: BamComponent,
        bytes: Vec<u8>,
    },
    RemoveResource {
        resource: BamComponent,
    },
    SpawnBatch {
        components: Vec<BamComponent>,
        entity_count: usize,
    },
}

/// Simulated dense archetype chunk inside [`MockV03World`].
struct MockChunk {
    entities: Vec<FfiEntity>,
    columns: HashMap<BamComponent, Vec<u64>>,
    column_added_ticks: HashMap<BamComponent, Vec<u32>>,
    column_changed_ticks: HashMap<BamComponent, Vec<u32>>,
}

impl MockChunk {
    /// Creates a mock chunk for the given `entities`.
    fn new(entities: Vec<FfiEntity>) -> Self {
        Self {
            entities,
            columns: HashMap::new(),
            column_added_ticks: HashMap::new(),
            column_changed_ticks: HashMap::new(),
        }
    }

    /// Sets per-row `(added, changed)` change-detection ticks for `comp` in this chunk.
    fn insert_column_ticks(&mut self, comp: BamComponent, added: &[u32], changed: &[u32]) {
        assert_eq!(added.len(), self.entities.len());
        assert_eq!(changed.len(), self.entities.len());
        self.column_added_ticks.insert(comp, added.to_vec());
        self.column_changed_ticks.insert(comp, changed.to_vec());
    }

    /// Populates a dense column buffer for `comp` from `values`.
    fn insert_dense<T: Copy>(&mut self, comp: BamComponent, values: &[T]) {
        assert_eq!(values.len(), self.entities.len());
        let byte_len = core::mem::size_of_val(values);
        let word_len = byte_len.div_ceil(8).max(1);
        let mut words = vec![0u64; word_len];
        if byte_len > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    values.as_ptr().cast::<u8>(),
                    words.as_mut_ptr().cast::<u8>(),
                    byte_len,
                );
            }
        }
        self.columns.insert(comp, words);
    }

    /// Reads back the dense column elements for `comp` as `Vec<T>`.
    fn read_dense<T: Copy>(&self, comp: BamComponent) -> Vec<T> {
        let count = self.entities.len();
        if core::mem::size_of::<T>() == 0 {
            return (0..count).map(|_| unsafe { core::mem::zeroed() }).collect();
        }
        let words = self.columns.get(&comp).expect("missing dense column");
        let mut out = Vec::<T>::with_capacity(count);
        unsafe {
            core::ptr::copy_nonoverlapping(words.as_ptr().cast::<T>(), out.as_mut_ptr(), count);
            out.set_len(count);
        }
        out
    }
}

/// Active iterator cursor and query term list backing `*mut BamIter` in [`MockV03World`].
struct MockIterState {
    world: *mut MockV03World,
    terms: Vec<BamTermDesc>,
    next_chunk_idx: usize,
    end_chunk_idx: Option<usize>,
    active_chunk_idx: Option<usize>,
}

/// Per-invocation frame backing `*mut BamSystemCtx` during [`V03TestHost::run_system`].
struct MockSystemFrame {
    world: *mut MockV03World,
    iters: Vec<Box<MockIterState>>,
}

/// Full ABI `0.3.0` (73-slot) mock host world exercising all v0.3 guest SDK APIs.
struct MockV03World {
    api: BamHostApi,
    components: Vec<RegisteredComp>,
    name_to_comp: HashMap<String, BamComponent>,
    uuid_to_comp: HashMap<ComponentUuid, BamComponent>,
    relations: HashMap<BamComponent, RegisteredRelation>,
    /// Active relation edges: `(relation, source) -> Vec<target>` (used for cycle detection & `up_cascade`).
    relation_edges: HashMap<(BamComponent, FfiEntity), Vec<FfiEntity>>,
    /// Per-entity component storage for `up_cascade` ancestor lookup.
    entity_components: HashMap<(FfiEntity, BamComponent), Vec<u64>>,
    /// When `true`, missing `up_cascade` returns `OK` with null `out_owner`/`out_ptr` instead of `NOT_FOUND`.
    up_cascade_missing_as_ok_null: bool,
    chunks: Vec<MockChunk>,
    systems: Vec<RegisteredSystem>,
    schedule_sets: Vec<RegisteredScheduleSet>,
    mounted_systems: Vec<(BamSystem, ScheduleId, BamSystemSet)>,
    commands: Vec<RecordedCmd>,
    rollback_calls: usize,
    sys_host_ctx_calls: AtomicUsize,
    sys_commands_calls: AtomicUsize,
    layout_info_calls: AtomicUsize,
    iter_column_info_calls: AtomicUsize,
    res_ticks_calls: AtomicUsize,
    iter_column_ro_calls: AtomicUsize,
    iter_column_mut_calls: AtomicUsize,
    next_entity_bits: u64,
    logs: Vec<(u32, String)>,
    faults: Vec<String>,
    last_error: String,
    symbols: Vec<String>,
    symbol_to_id: HashMap<String, BamSymbol>,
    named_objects: HashMap<String, FfiEntity>,
    tick_last_run: u32,
    tick_this_run: u32,
    resources: HashMap<BamComponent, (Vec<u64>, BamChangeTicks)>,
    messages: HashMap<BamComponent, Vec<Vec<u8>>>,
}

/// Owning pointer wrapper around heap-allocated [`MockV03World`] with stable provenance.
struct V03TestHost {
    raw: *mut MockV03World,
}

impl Drop for V03TestHost {
    /// Reclaims the heap-allocated [`MockV03World`].
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { drop(Box::from_raw(self.raw)) };
        }
    }
}

impl core::ops::Deref for V03TestHost {
    type Target = MockV03World;
    /// Borrows the underlying [`MockV03World`] immutably.
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.raw }
    }
}

impl core::ops::DerefMut for V03TestHost {
    /// Borrows the underlying [`MockV03World`] mutably.
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.raw }
    }
}

impl V03TestHost {
    /// Creates a [`V03TestHost`] wired with a complete 73-slot ABI `0.3.0` [`BamHostApi`].
    fn new() -> Self {
        Self::with_api(make_v03_host_api())
    }

    /// Creates a [`V03TestHost`] using a custom `api` table (e.g., truncated v0.2 host).
    fn with_api(api: BamHostApi) -> Self {
        Self {
            raw: Box::into_raw(Box::new(MockV03World {
                api,
                components: Vec::new(),
                name_to_comp: HashMap::new(),
                uuid_to_comp: HashMap::new(),
                relations: HashMap::new(),
                relation_edges: HashMap::new(),
                entity_components: HashMap::new(),
                up_cascade_missing_as_ok_null: false,
                chunks: Vec::new(),
                systems: Vec::new(),
                schedule_sets: Vec::new(),
                mounted_systems: Vec::new(),
                commands: Vec::new(),
                rollback_calls: 0,
                sys_host_ctx_calls: AtomicUsize::new(0),
                sys_commands_calls: AtomicUsize::new(0),
                layout_info_calls: AtomicUsize::new(0),
                iter_column_info_calls: AtomicUsize::new(0),
                res_ticks_calls: AtomicUsize::new(0),
                iter_column_ro_calls: AtomicUsize::new(0),
                iter_column_mut_calls: AtomicUsize::new(0),
                next_entity_bits: 1000,
                logs: Vec::new(),
                faults: Vec::new(),
                last_error: "initial_host_error".to_owned(),
                symbols: Vec::new(),
                symbol_to_id: HashMap::new(),
                named_objects: HashMap::new(),
                tick_last_run: 5,
                tick_this_run: 10,
                resources: HashMap::new(),
                messages: HashMap::new(),
            })),
        }
    }

    /// Returns a pointer to the mock world's [`BamHostApi`] table.
    const fn api_ptr(&self) -> *const BamHostApi {
        unsafe { &raw const (*self.raw).api }
    }

    /// Initializes an inline closure mod named `mod_name` against this ABI `0.3.0` mock host.
    fn load_fn<F>(&self, mod_name: &CStr, build_fn: F) -> Result<*mut BamInstance, FfiResult>
    where
        F: FnOnce(&mut GuestApp<'_>) -> FfiResult,
    {
        let manifest = BamManifest::new(mod_name.as_ptr(), c"0.3.0".as_ptr());
        let mut instance_out: *mut BamInstance = core::ptr::null_mut();
        let status = unsafe {
            init_guest_instance(
                self.api_ptr(),
                self.raw.cast::<BamLoadCtx>(),
                &mut instance_out,
                &manifest,
                |inst, load| build_guest_fn(inst, load, build_fn),
            )
        };
        if status.is_ok() {
            Ok(instance_out)
        } else {
            Err(status)
        }
    }

    /// Shuts down and frees a guest instance loaded via [`Self::load_fn`].
    fn unload(&self, instance: *mut BamInstance) {
        unsafe { shutdown_guest_instance(instance) };
    }

    /// Executes registered system `sys_idx` against this mock world.
    fn run_system(&self, sys_idx: usize) -> FfiResult {
        let (
            run_if,
            run_if_data,
            run_if_ctx,
            run_if_query,
            run_if_ctx_data,
            callback,
            user_data,
            queries,
        ) = unsafe {
            let world = &*self.raw;
            let sys = &world.systems[sys_idx];
            (
                sys.run_if,
                sys.run_if_user_data,
                sys.run_if_ctx,
                sys.run_if_query.clone(),
                sys.run_if_ctx_user_data,
                sys.callback,
                sys.user_data,
                sys.queries.clone(),
            )
        };
        if let Some(pred) = run_if {
            if !unsafe { pred(run_if_data) }.get() {
                return FfiResult::OK;
            }
        }
        let world_ptr = self.raw;
        if let Some(pred_ctx) = run_if_ctx {
            let cond_iters = if run_if_query.is_empty() {
                Vec::new()
            } else {
                vec![Box::new(MockIterState {
                    world: world_ptr,
                    terms: run_if_query,
                    next_chunk_idx: 0,
                    end_chunk_idx: None,
                    active_chunk_idx: None,
                })]
            };
            let mut cond_frame = MockSystemFrame {
                world: world_ptr,
                iters: cond_iters,
            };
            let cond_res = unsafe {
                pred_ctx(
                    (&raw mut cond_frame).cast::<BamSystemCtx>(),
                    run_if_ctx_data,
                )
            };
            if !cond_res.is_ok() {
                return FfiResult::OK;
            }
        }
        let iters = queries
            .into_iter()
            .map(|terms| {
                Box::new(MockIterState {
                    world: world_ptr,
                    terms,
                    next_chunk_idx: 0,
                    end_chunk_idx: None,
                    active_chunk_idx: None,
                })
            })
            .collect();
        let mut frame = MockSystemFrame {
            world: world_ptr,
            iters,
        };
        unsafe { callback((&raw mut frame).cast::<BamSystemCtx>(), user_data) }
    }
}

impl MockV03World {
    /// Sets a mock singleton resource value and change ticks for `comp`.
    fn set_resource<R: Copy>(&mut self, comp: BamComponent, value: R, ticks: BamChangeTicks) {
        let byte_len = core::mem::size_of::<R>();
        let word_len = byte_len.div_ceil(8).max(1);
        let mut words = vec![0u64; word_len];
        if byte_len > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&raw const value).cast::<u8>(),
                    words.as_mut_ptr().cast::<u8>(),
                    byte_len,
                );
            }
        }
        self.resources.insert(comp, (words, ticks));
    }

    /// Removes a mock singleton resource for `comp`.
    fn remove_mock_resource(&mut self, comp: BamComponent) {
        self.resources.remove(&comp);
    }

    /// Enqueues a mock buffered message payload for `comp`.
    fn push_message<M: Copy>(&mut self, comp: BamComponent, msg: M) {
        let byte_len = core::mem::size_of::<M>();
        let mut bytes = vec![0u8; byte_len];
        if byte_len > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&raw const msg).cast::<u8>(),
                    bytes.as_mut_ptr(),
                    byte_len,
                );
            }
        }
        self.messages.entry(comp).or_default().push(bytes);
    }

    /// Pre-registers a host component type `T` with explicit UUID and schema fingerprint.
    fn expose_component<T: 'static>(
        &mut self,
        name: &str,
        uuid: ComponentUuid,
        schema_fingerprint: u64,
        fields: &[BamFieldDesc],
    ) -> BamComponent {
        let handle = BamComponent::from_raw((self.components.len() + 1) as u32);
        self.components.push(RegisteredComp {
            handle,
            name: name.to_owned(),
            uuid,
            schema_fingerprint,
            size: core::mem::size_of::<T>(),
            align: core::mem::align_of::<T>(),
            flags: 0,
            fields: fields.to_vec(),
        });
        self.name_to_comp.insert(name.to_owned(), handle);
        if !uuid.is_zero() {
            self.uuid_to_comp.insert(uuid, handle);
        }
        handle
    }

    /// Stores per-entity component bytes used by `host_iter_up_cascade` ancestor lookups.
    fn set_entity_component<T: Copy>(&mut self, entity: FfiEntity, comp: BamComponent, value: T) {
        let byte_len = core::mem::size_of::<T>();
        let word_len = byte_len.div_ceil(8).max(1);
        let mut words = vec![0u64; word_len];
        if byte_len > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&raw const value).cast::<u8>(),
                    words.as_mut_ptr().cast::<u8>(),
                    byte_len,
                );
            }
        }
        self.entity_components.insert((entity, comp), words);
    }

    /// Returns immutable metadata for 1-based component `handle`.
    fn comp_meta(&self, handle: BamComponent) -> Option<&RegisteredComp> {
        let idx = (handle.to_raw() as usize).checked_sub(1)?;
        self.components.get(idx)
    }

    /// Returns mutable metadata for 1-based component `handle`.
    fn comp_meta_mut(&mut self, handle: BamComponent) -> Option<&mut RegisteredComp> {
        let idx = (handle.to_raw() as usize).checked_sub(1)?;
        self.components.get_mut(idx)
    }

    /// Performs a DFS reachability check to detect whether adding `(source -> target)` creates a cycle.
    fn would_create_cycle(
        &self,
        relation: BamComponent,
        source: FfiEntity,
        target: FfiEntity,
    ) -> bool {
        if source == target {
            return true;
        }
        let mut stack = vec![target];
        let mut visited = HashSet::new();
        while let Some(cur) = stack.pop() {
            if cur == source {
                return true;
            }
            if !visited.insert(cur) {
                continue;
            }
            if let Some(nexts) = self.relation_edges.get(&(relation, cur)) {
                stack.extend_from_slice(nexts);
            }
        }
        false
    }
}

// ============================================================================
// Mock ABI 0.2 + 0.3 Host FFI Implementations
// ============================================================================

/// Mock `BamHostApi::get_proc_address` (returns null in v0.3 harness).
unsafe extern "C" fn host_get_proc_address(
    _ctx: *const BamHostCtx,
    _name: *const c_char,
) -> *const c_void {
    core::ptr::null()
}

/// Mock `BamHostApi::last_error` copying `MockV03World::last_error` into caller's buffer.
unsafe extern "C" fn host_last_error(
    ctx: *const BamHostCtx,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let world = unsafe { &*ctx.cast::<MockV03World>() };
    let bytes = world.last_error.as_bytes();
    if !buf.is_null() && cap > 0 {
        let n = bytes.len().min(cap - 1);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), n);
            *buf.add(n) = 0;
        }
    }
    bytes.len()
}

/// Mock `BamHostApi::load_host_ctx` returning the underlying `*const BamHostCtx`.
unsafe extern "C" fn host_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
    load_ctx.cast::<BamHostCtx>()
}

/// Mock `BamHostApi::register_component` storing component metadata and ABI 0.3.0 fingerprint.
unsafe extern "C" fn host_register_component(
    load_ctx: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out_component: *mut BamComponent,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    let desc_ref = unsafe { &*desc };
    let st = desc_ref.validate();
    if !st.is_ok() {
        return st;
    }
    let Ok(name) = unsafe { CStr::from_ptr(desc_ref.name) }.to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    if let Some(&existing) = world.name_to_comp.get(name) {
        unsafe { *out_component = existing };
        return FfiResult::OK;
    }
    let fields = if !desc_ref.fields.is_null() && desc_ref.field_count > 0 {
        unsafe { core::slice::from_raw_parts(desc_ref.fields, desc_ref.field_count) }.to_vec()
    } else {
        Vec::new()
    };
    let handle = BamComponent::from_raw((world.components.len() + 1) as u32);
    world.components.push(RegisteredComp {
        handle,
        name: name.to_owned(),
        uuid: desc_ref.uuid,
        schema_fingerprint: desc_ref.schema_fingerprint,
        size: desc_ref.size,
        align: desc_ref.align,
        flags: desc_ref.flags,
        fields,
    });
    world.name_to_comp.insert(name.to_owned(), handle);
    if !desc_ref.uuid.is_zero() {
        world.uuid_to_comp.insert(desc_ref.uuid, handle);
    }
    unsafe { *out_component = handle };
    FfiResult::OK
}

/// Mock `BamHostApi::resolve_component` resolving a component handle by name or UUID.
unsafe extern "C" fn host_resolve_component(
    ctx: *const BamHostCtx,
    name: *const c_char,
    uuid: *const ComponentUuid,
    out_component: *mut BamComponent,
) -> FfiResult {
    if ctx.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &*ctx.cast::<MockV03World>() };
    let mut found = None;
    if !name.is_null() {
        let Ok(name_str) = unsafe { CStr::from_ptr(name) }.to_str() else {
            return FfiResult::INVALID_UTF8;
        };
        if let Some(&h) = world.name_to_comp.get(name_str) {
            found = Some(h);
        }
    } else if !uuid.is_null() {
        let u = unsafe { *uuid };
        if let Some(&h) = world.uuid_to_comp.get(&u) {
            found = Some(h);
        }
    } else {
        return FfiResult::INVALID_ARGUMENT;
    }
    let Some(handle) = found else {
        return FfiResult::NOT_FOUND;
    };
    if !uuid.is_null() {
        let u = unsafe { *uuid };
        if !u.is_zero() && world.comp_meta(handle).is_some_and(|c| c.uuid != u) {
            return FfiResult::TYPE_MISMATCH;
        }
    }
    unsafe { *out_component = handle };
    FfiResult::OK
}

/// Shared helper registering a `BamSystemDesc` into `world` and returning its 1-based [`BamSystem`].
fn push_system(world: &mut MockV03World, desc: &BamSystemDesc) -> Result<BamSystem, FfiResult> {
    let st = desc.validate();
    if !st.is_ok() {
        return Err(st);
    }
    let Ok(name) = unsafe { CStr::from_ptr(desc.name) }.to_str() else {
        return Err(FfiResult::INVALID_UTF8);
    };
    let mut queries = Vec::new();
    if desc.query_count > 0 && !desc.queries.is_null() {
        let q_slice = unsafe { core::slice::from_raw_parts(desc.queries, desc.query_count) };
        for q in q_slice {
            let q_st = q.validate();
            if !q_st.is_ok() {
                return Err(q_st);
            }
            let terms = if q.term_count > 0 && !q.terms.is_null() {
                unsafe { core::slice::from_raw_parts(q.terms, q.term_count) }.to_vec()
            } else {
                Vec::new()
            };
            queries.push(terms);
        }
    }
    let run_if_query = if !desc.run_if_query.is_null() {
        let q = unsafe { &*desc.run_if_query };
        let q_st = q.validate();
        if !q_st.is_ok() {
            return Err(q_st);
        }
        if q.term_count > 0 && !q.terms.is_null() {
            unsafe { core::slice::from_raw_parts(q.terms, q.term_count) }.to_vec()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let handle = BamSystem::from_raw(world.systems.len() as u32);
    world.systems.push(RegisteredSystem {
        handle,
        name: name.to_owned(),
        queries,
        schedule: desc.schedule,
        callback: desc.callback.ok_or(FfiResult::NULL_POINTER)?,
        user_data: desc.user_data,
        run_if: desc.run_if,
        run_if_user_data: desc.run_if_user_data,
        run_if_ctx: desc.run_if_ctx,
        run_if_query,
        run_if_ctx_user_data: desc.run_if_ctx_user_data,
    });
    Ok(handle)
}

/// Mock `BamHostApi::register_system` delegating to [`push_system`].
unsafe extern "C" fn host_register_system(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    let desc_ref = unsafe { &*desc };
    match push_system(world, desc_ref) {
        Ok(_) => FfiResult::OK,
        Err(err) => err,
    }
}

/// Mock `BamHostApi::sys_host_ctx` returning the world pointer from `MockSystemFrame`.
unsafe extern "C" fn host_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
    let frame = unsafe { &*sys_ctx.cast::<MockSystemFrame>() };
    let world = unsafe { &*frame.world };
    world.sys_host_ctx_calls.fetch_add(1, Ordering::Relaxed);
    frame.world.cast::<BamHostCtx>()
}

/// Mock `BamHostApi::sys_query` returning the `MockIterState` pointer for `query_index`.
unsafe extern "C" fn host_sys_query(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    out_iter: *mut *mut BamIter,
) -> FfiResult {
    let frame = unsafe { &mut *sys_ctx.cast::<MockSystemFrame>() };
    let Some(iter_box) = frame.iters.get_mut(query_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    unsafe { *out_iter = (&raw mut **iter_box).cast::<BamIter>() };
    FfiResult::OK
}

/// Mock `BamHostApi::sys_commands` returning the world pointer as `*mut BamCommands`.
unsafe extern "C" fn host_sys_commands(
    sys_ctx: *mut BamSystemCtx,
    out_commands: *mut *mut BamCommands,
) -> FfiResult {
    let frame = unsafe { &*sys_ctx.cast::<MockSystemFrame>() };
    let world = unsafe { &*frame.world };
    world.sys_commands_calls.fetch_add(1, Ordering::Relaxed);
    unsafe { *out_commands = frame.world.cast::<BamCommands>() };
    FfiResult::OK
}

/// Mock `BamHostApi::iter_next_chunk` advancing to the next chunk in [`MockV03World`].
unsafe extern "C" fn host_iter_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
    let state = unsafe { &mut *iter.cast::<MockIterState>() };
    let world = unsafe { &*state.world };
    let limit = state
        .end_chunk_idx
        .unwrap_or(world.chunks.len())
        .min(world.chunks.len());
    while state.next_chunk_idx < limit {
        let idx = state.next_chunk_idx;
        state.next_chunk_idx += 1;
        let len = world.chunks[idx].entities.len();
        if len > 0 {
            state.active_chunk_idx = Some(idx);
            unsafe { *out_len = len };
            return FfiBool::TRUE;
        }
    }
    state.active_chunk_idx = None;
    unsafe { *out_len = 0 };
    FfiBool::FALSE
}

/// Mock `BamHostApi::iter_reset` rewinding the chunk cursor.
unsafe extern "C" fn host_iter_reset(iter: *mut BamIter) -> FfiResult {
    let state = unsafe { &mut *iter.cast::<MockIterState>() };
    state.next_chunk_idx = 0;
    state.active_chunk_idx = None;
    FfiResult::OK
}

/// Mock `BamHostApi::iter_entities` returning the active chunk's entity slice pointer.
unsafe extern "C" fn host_iter_entities(iter: *const BamIter) -> *const FfiEntity {
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    let world = unsafe { &*state.world };
    world.chunks[idx].entities.as_ptr()
}

/// Mock `BamHostApi::iter_column_readonly` returning a read-only dense column pointer.
unsafe extern "C" fn host_iter_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(idx) = state.active_chunk_idx else {
        return core::ptr::null();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null();
    };
    let world = unsafe { &*state.world };
    world.iter_column_ro_calls.fetch_add(1, Ordering::Relaxed);
    world.chunks[idx]
        .columns
        .get(&term.component)
        .map_or(core::ptr::null(), |w| w.as_ptr().cast::<u8>())
}

/// Mock `BamHostApi::iter_column_mutable` returning a writable dense column pointer.
unsafe extern "C" fn host_iter_col_mut(iter: *mut BamIter, term_index: usize) -> *mut u8 {
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(idx) = state.active_chunk_idx else {
        return core::ptr::null_mut();
    };
    let Some(term) = state.terms.get(term_index) else {
        return core::ptr::null_mut();
    };
    let world = unsafe { &mut *state.world };
    world.iter_column_mut_calls.fetch_add(1, Ordering::Relaxed);
    world.chunks[idx]
        .columns
        .get_mut(&term.component)
        .map_or(core::ptr::null_mut(), |w| w.as_mut_ptr().cast::<u8>())
}

/// Mock `BamHostApi::iter_column_mutable_untracked` delegating to [`host_iter_col_mut`].
unsafe extern "C" fn host_iter_col_mut_untracked(iter: *mut BamIter, term_index: usize) -> *mut u8 {
    unsafe { host_iter_col_mut(iter, term_index) }
}

/// Mock `BamHostApi::iter_mark_changed_range` accepting valid change-mark calls.
unsafe extern "C" fn host_iter_mark_changed(
    _iter: *mut BamIter,
    _term_index: usize,
    _start: usize,
    _count: usize,
) -> FfiResult {
    FfiResult::OK
}

/// Stub `BamHostApi::iter_column_rows_readonly` (unused in v0.3 dense tests).
unsafe extern "C" fn host_iter_rows_ro(_iter: *mut BamIter, _idx: usize) -> *const *const u8 {
    core::ptr::null()
}

/// Stub `BamHostApi::iter_column_rows_mutable` (unused in v0.3 dense tests).
unsafe extern "C" fn host_iter_rows_mut(_iter: *mut BamIter, _idx: usize) -> *const *mut u8 {
    core::ptr::null()
}

/// Mock `BamHostApi::iter_column_info` populating `BamColumnInfo` for `term_index`.
unsafe extern "C" fn host_iter_col_info(
    iter: *const BamIter,
    term_index: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult {
    if iter.is_null() || out_info.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let world = unsafe { &*state.world };
    world.iter_column_info_calls.fetch_add(1, Ordering::Relaxed);
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::NOT_FOUND;
    };
    let storage_flags = if term.mode.is_resource() {
        bamabi_sys::BAM_STORAGE_FLAG_RESOURCE
    } else if term.mode.is_message() {
        bamabi_sys::BAM_STORAGE_FLAG_MESSAGE
    } else {
        0
    };
    let info = BamColumnInfo {
        component: term.component,
        mode: term.mode,
        uuid: meta.uuid,
        size: meta.size,
        align: meta.align,
        storage_flags,
        reserved0: 0,
    };
    unsafe { *out_info = info };
    FfiResult::OK
}

/// Mock `BamHostApi::res_ptr` returning the mock singleton resource buffer for `term_index`.
unsafe extern "C" fn host_res_ptr(iter: *const BamIter, idx: usize) -> *const u8 {
    if iter.is_null() {
        return core::ptr::null();
    }
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(term) = state.terms.get(idx) else {
        return core::ptr::null();
    };
    let world = unsafe { &*state.world };
    world
        .resources
        .get(&term.component)
        .map_or(core::ptr::null(), |(words, _)| words.as_ptr().cast::<u8>())
}

/// Stub `BamHostApi::res_mut_ptr` (unused in v0.3 harness).
unsafe extern "C" fn host_res_mut_ptr(_iter: *mut BamIter, _idx: usize) -> *mut u8 {
    core::ptr::null_mut()
}

/// Mock `BamHostApi::res_ticks` returning change ticks for the singleton resource at `idx`.
unsafe extern "C" fn host_res_ticks(
    iter: *const BamIter,
    idx: usize,
    out_ticks: *mut BamChangeTicks,
) -> FfiResult {
    if iter.is_null() || out_ticks.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(term) = state.terms.get(idx) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let world = unsafe { &*state.world };
    world.res_ticks_calls.fetch_add(1, Ordering::Relaxed);
    let Some((_, ticks)) = world.resources.get(&term.component) else {
        return FfiResult::NOT_FOUND;
    };
    unsafe { *out_ticks = *ticks };
    FfiResult::OK
}

/// Stateful cursor iterator advancing `cursor` before yielding and draining consumed items on drop.
struct StatefulCursorIter<'a, T: Default> {
    items: &'a mut Vec<T>,
    cursor: usize,
}

impl<T: Default> Iterator for StatefulCursorIter<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let slot = self.items.get_mut(self.cursor)?;
        self.cursor += 1;
        Some(core::mem::take(slot))
    }
}

impl<T: Default> Drop for StatefulCursorIter<'_, T> {
    fn drop(&mut self) {
        if self.cursor > 0 {
            self.items.drain(..self.cursor);
        }
    }
}

/// Mock `BamHostApi::msg_read_batch` draining queued mock messages for `term_index`.
unsafe extern "C" fn host_msg_read_batch(
    iter: *mut BamIter,
    idx: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_count: *mut usize,
) -> FfiResult {
    if iter.is_null() || out_count.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let state = unsafe { &mut *iter.cast::<MockIterState>() };
    let Some(term) = state.terms.get(idx) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let world = unsafe { &mut *state.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    let elem_size = meta.size;
    let queue = world.messages.entry(term.component).or_default();
    let avail = queue.len().min(max_count);
    if avail > 0 && elem_size > 0 && out_buf.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let cursor_iter = StatefulCursorIter {
        items: queue,
        cursor: 0,
    };
    let mut take = 0usize;
    for (i, payload) in cursor_iter.take(max_count).enumerate() {
        if elem_size > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    payload.as_ptr(),
                    out_buf.add(i * elem_size),
                    elem_size,
                );
            }
        }
        take = i + 1;
    }
    unsafe { *out_count = take };
    FfiResult::OK
}

/// Mock `BamHostApi::sys_entity_get_mut` locating `(entity, comp)` in mock chunks.
unsafe extern "C" fn host_sys_entity_get_mut(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    entity: FfiEntity,
    term_index: usize,
) -> *mut u8 {
    if sys_ctx.is_null() {
        return core::ptr::null_mut();
    }
    let frame = unsafe { &mut *sys_ctx.cast::<MockSystemFrame>() };
    let Some(iter_box) = frame.iters.get(query_index) else {
        return core::ptr::null_mut();
    };
    let Some(term) = iter_box.terms.get(term_index) else {
        return core::ptr::null_mut();
    };
    let world = unsafe { &mut *frame.world };
    let Some(meta) = world.comp_meta(term.component) else {
        return core::ptr::null_mut();
    };
    let elem_size = meta.size;
    for chunk in &mut world.chunks {
        if let Some(row) = chunk.entities.iter().position(|&e| e == entity) {
            if let Some(words) = chunk.columns.get_mut(&term.component) {
                return unsafe { words.as_mut_ptr().cast::<u8>().add(row * elem_size) };
            }
        }
    }
    core::ptr::null_mut()
}

/// Mock `BamHostApi::iter_column_ticks` (slot 69) returning per-row tick slices for the active chunk.
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
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(chunk_idx) = state.active_chunk_idx else {
        return FfiResult::OUT_OF_RANGE;
    };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let world = unsafe { &*state.world };
    let chunk = &world.chunks[chunk_idx];
    let (Some(added), Some(changed)) = (
        chunk.column_added_ticks.get(&term.component),
        chunk.column_changed_ticks.get(&term.component),
    ) else {
        return FfiResult::NOT_FOUND;
    };
    unsafe {
        *out_added = added.as_ptr();
        *out_changed = changed.as_ptr();
        *out_last_run = world.tick_last_run;
        *out_this_run = world.tick_this_run;
    }
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_clone_entity` (slot 70).
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
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    world.commands.push(RecordedCmd::CloneEntity {
        src: src_entity,
        dst: dst_entity,
    });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_insert_resource` (slot 71).
unsafe extern "C" fn host_cmd_insert_resource(
    cmds: *mut BamCommands,
    resource: BamComponent,
    data: *const u8,
    size: usize,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    let bytes = if size == 0 {
        Vec::new()
    } else if data.is_null() {
        return FfiResult::NULL_POINTER;
    } else {
        unsafe { core::slice::from_raw_parts(data, size) }.to_vec()
    };
    world
        .commands
        .push(RecordedCmd::InsertResource { resource, bytes });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_remove_resource` (slot 72).
unsafe extern "C" fn host_cmd_remove_resource(
    cmds: *mut BamCommands,
    resource: BamComponent,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    world
        .commands
        .push(RecordedCmd::RemoveResource { resource });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_spawn_empty` allocating an entity ID and logging `RecordedCmd::SpawnEmpty`.
unsafe extern "C" fn host_cmd_spawn_empty(
    cmds: *mut BamCommands,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    world.next_entity_bits += 1;
    let entity = FfiEntity::from_bits(world.next_entity_bits);
    world.commands.push(RecordedCmd::SpawnEmpty(entity));
    unsafe { *out_entity = entity };
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_spawn` allocating an entity ID and logging `RecordedCmd::SpawnEmpty`.
unsafe extern "C" fn host_cmd_spawn(
    cmds: *mut BamCommands,
    _comps: *const BamComponent,
    _ptrs: *const *const u8,
    _count: usize,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    unsafe { host_cmd_spawn_empty(cmds, out_entity) }
}

/// Mock `BamHostApi::cmd_despawn` logging `RecordedCmd::Despawn`.
unsafe extern "C" fn host_cmd_despawn(cmds: *mut BamCommands, entity: FfiEntity) -> FfiResult {
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    world.commands.push(RecordedCmd::Despawn(entity));
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_insert` logging `RecordedCmd::Insert`.
unsafe extern "C" fn host_cmd_insert(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
    data: *const u8,
    len: usize,
) -> FfiResult {
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    let bytes = if len > 0 && !data.is_null() {
        unsafe { core::slice::from_raw_parts(data, len) }.to_vec()
    } else {
        Vec::new()
    };
    world.commands.push(RecordedCmd::Insert {
        entity,
        component: comp,
        bytes,
    });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_insert_default` logging `RecordedCmd::Insert` with zero length.
unsafe extern "C" fn host_cmd_insert_default(
    _cmds: *mut BamCommands,
    _entity: FfiEntity,
    _comp: BamComponent,
) -> FfiResult {
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_remove` logging `RecordedCmd::Remove`.
unsafe extern "C" fn host_cmd_remove(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    comp: BamComponent,
) -> FfiResult {
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    world.commands.push(RecordedCmd::Remove {
        entity,
        component: comp,
    });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_spawn_batch` allocating `entity_count` entities and recording `RecordedCmd::SpawnBatch`.
unsafe extern "C" fn host_cmd_spawn_batch(
    cmds: *mut BamCommands,
    comps: *const BamComponent,
    col_ptrs: *const *const u8,
    comp_count: usize,
    entity_count: usize,
    out_entities: *mut FfiEntity,
) -> FfiResult {
    if cmds.is_null() || (comp_count > 0 && (comps.is_null() || col_ptrs.is_null())) {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: Non-null `cmds` points to `MockV03World` in this test harness.
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    let components = if comp_count == 0 {
        Vec::new()
    } else {
        // SAFETY: `comps` is non-null and valid for `comp_count` `BamComponent` elements.
        unsafe { core::slice::from_raw_parts(comps, comp_count) }.to_vec()
    };
    for i in 0..entity_count {
        world.next_entity_bits += 1;
        let ent = FfiEntity::from_bits(world.next_entity_bits);
        if !out_entities.is_null() {
            // SAFETY: Caller verified `out_entities` has at least `entity_count` writable slots.
            unsafe { *out_entities.add(i) = ent };
        }
    }
    world.commands.push(RecordedCmd::SpawnBatch {
        components,
        entity_count,
    });
    FfiResult::OK
}

// --- Extended & ABI 0.3.0 Host Slots ---

/// Mock `BamHostApi::log_diagnostic` (slot 50) recording `(level, message)` diagnostics.
unsafe extern "C" fn host_log_diagnostic(
    ctx: *const BamHostCtx,
    level: u32,
    message: *const StrView,
) -> FfiResult {
    if ctx.is_null() || message.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if level > BAM_LOG_ERROR {
        return FfiResult::INVALID_ARGUMENT;
    }
    let Ok(text) = (unsafe { (*message).as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    // NOTE: Must NOT overwrite `world.last_error`.
    let world = unsafe { &mut *ctx.cast::<MockV03World>().cast_mut() };
    world.logs.push((level, text.to_owned()));
    FfiResult::OK
}

/// Mock `BamHostApi::report_guest_fault` (slot 51) recording fault messages and updating `last_error`.
unsafe extern "C" fn host_report_guest_fault(
    ctx: *const BamHostCtx,
    message: *const StrView,
) -> FfiResult {
    if ctx.is_null() || message.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Ok(text) = (unsafe { (*message).as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    if text.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &mut *ctx.cast::<MockV03World>().cast_mut() };
    world.faults.push(text.to_owned());
    world.last_error = text.to_owned();
    FfiResult::OK
}

/// Mock `BamHostApi::resolve_component_by_uuid` (slot 52) looking up handles strictly by UUID.
unsafe extern "C" fn host_resolve_component_by_uuid(
    ctx: *const BamHostCtx,
    uuid: *const ComponentUuid,
    out_component: *mut BamComponent,
) -> FfiResult {
    if ctx.is_null() || uuid.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let u = unsafe { *uuid };
    if u.is_zero() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &*ctx.cast::<MockV03World>() };
    let Some(&handle) = world.uuid_to_comp.get(&u) else {
        return FfiResult::NOT_FOUND;
    };
    unsafe { *out_component = handle };
    FfiResult::OK
}

/// Mock `BamHostApi::component_layout_info` (slot 53) populating `BamLayoutInfo`.
unsafe extern "C" fn host_component_layout_info(
    ctx: *const BamHostCtx,
    component: BamComponent,
    out_info: *mut BamLayoutInfo,
) -> FfiResult {
    if ctx.is_null() || out_info.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if !component.is_valid() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &*ctx.cast::<MockV03World>() };
    world.layout_info_calls.fetch_add(1, Ordering::Relaxed);
    let Some(meta) = world.comp_meta(component) else {
        return FfiResult::NOT_FOUND;
    };
    let info = BamLayoutInfo {
        struct_size: core::mem::size_of::<BamLayoutInfo>() as u32,
        flags: meta.flags,
        uuid: meta.uuid,
        schema_fingerprint: meta.schema_fingerprint,
        size: meta.size,
        align: meta.align,
        fields: if meta.fields.is_empty() {
            core::ptr::null()
        } else {
            meta.fields.as_ptr()
        },
        field_count: meta.fields.len(),
    };
    unsafe { *out_info = info };
    FfiResult::OK
}

/// Mock `BamHostApi::register_relation` (slot 54) validating and storing relationship descriptors.
unsafe extern "C" fn host_register_relation(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamRelationDesc,
    out_component: *mut BamComponent,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_component.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let desc_ref = unsafe { &*desc };
    let st = desc_ref.validate();
    if !st.is_ok() {
        return st;
    }
    let Ok(name) = unsafe { CStr::from_ptr(desc_ref.name) }.to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    if let Some(&existing) = world.name_to_comp.get(name) {
        unsafe { *out_component = existing };
        return FfiResult::OK;
    }
    let handle = BamComponent::from_raw((world.components.len() + 1) as u32);
    world.components.push(RegisteredComp {
        handle,
        name: name.to_owned(),
        uuid: desc_ref.uuid,
        schema_fingerprint: 0,
        size: desc_ref.data_size,
        align: desc_ref.data_align,
        flags: 0,
        fields: Vec::new(),
    });
    world.name_to_comp.insert(name.to_owned(), handle);
    if !desc_ref.uuid.is_zero() {
        world.uuid_to_comp.insert(desc_ref.uuid, handle);
    }
    world.relations.insert(
        handle,
        RegisteredRelation {
            handle,
            name: name.to_owned(),
            uuid: desc_ref.uuid,
            flags: desc_ref.flags,
            data_size: desc_ref.data_size,
            data_align: desc_ref.data_align,
            max_degree: desc_ref.max_degree,
        },
    );
    unsafe { *out_component = handle };
    FfiResult::OK
}

/// Mock `BamHostApi::intern_symbol` (slot 55) deduplicating UTF-8 strings into `BamSymbol` IDs.
unsafe extern "C" fn host_intern_symbol(
    ctx: *const BamHostCtx,
    symbol: *const StrView,
    out_symbol: *mut BamSymbol,
) -> FfiResult {
    if ctx.is_null() || symbol.is_null() || out_symbol.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Ok(text) = (unsafe { (*symbol).as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    if text.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &mut *ctx.cast::<MockV03World>().cast_mut() };
    if let Some(&sym) = world.symbol_to_id.get(text) {
        unsafe { *out_symbol = sym };
        return FfiResult::OK;
    }
    let sym = BamSymbol::from_raw(world.symbols.len() as u32);
    world.symbols.push(text.to_owned());
    world.symbol_to_id.insert(text.to_owned(), sym);
    unsafe { *out_symbol = sym };
    FfiResult::OK
}

/// Mock `BamHostApi::bind_named_object` (slot 56) binding a canonical name to an `FfiEntity`.
unsafe extern "C" fn host_bind_named_object(
    ctx: *const BamHostCtx,
    name: *const StrView,
    entity: FfiEntity,
) -> FfiResult {
    if ctx.is_null() || name.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let Ok(text) = (unsafe { (*name).as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    if text.is_empty() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &mut *ctx.cast::<MockV03World>().cast_mut() };
    world.named_objects.insert(text.to_owned(), entity);
    FfiResult::OK
}

/// Mock `BamHostApi::iter_count` (slot 57) non-destructively counting matching entities.
unsafe extern "C" fn host_iter_count(iter: *mut BamIter, out_count: *mut usize) -> FfiResult {
    if iter.is_null() || out_count.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let world = unsafe { &*state.world };
    let total: usize = world.chunks.iter().map(|c| c.entities.len()).sum();
    unsafe { *out_count = total };
    FfiResult::OK
}

/// Mock `BamHostApi::iter_up_cascade` (slot 58) walking relationship ancestors with cycle detection.
unsafe extern "C" fn host_iter_up_cascade(
    iter: *mut BamIter,
    relation: BamComponent,
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
    let state = unsafe { &*iter.cast::<MockIterState>() };
    let Some(term) = state.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    if matches!(term.mode, AccessMode::WRITE | AccessMode::OPTIONAL_WRITE) {
        return FfiResult::PERMISSION_DENIED;
    }
    let world = unsafe { &*state.world };
    let mut visited = HashSet::new();
    visited.insert(entity);
    let mut cur = entity;
    while let Some(targets) = world.relation_edges.get(&(relation, cur)) {
        let Some(&parent) = targets.first() else {
            break;
        };
        if !visited.insert(parent) {
            return FfiResult::CYCLE_DETECTED;
        }
        if let Some(words) = world.entity_components.get(&(parent, term.component)) {
            unsafe {
                *out_ptr = words.as_ptr().cast::<c_void>();
                *out_owner = parent;
            }
            return FfiResult::OK;
        }
        cur = parent;
    }
    unsafe {
        *out_ptr = core::ptr::null();
        *out_owner = FfiEntity::NULL;
    }
    if world.up_cascade_missing_as_ok_null {
        FfiResult::OK
    } else {
        FfiResult::NOT_FOUND
    }
}

/// Mock `BamHostApi::par_for_each_chunk_readonly` (slot 59) rejecting writable queries and dispatching read-only chunks.
unsafe extern "C" fn host_par_for_each_chunk_readonly(
    iter: *mut BamIter,
    task_fn: Option<BamChunkTaskFn>,
    user_data: *mut c_void,
) -> FfiResult {
    if iter.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(cb) = task_fn else {
        return FfiResult::NULL_POINTER;
    };
    let state = unsafe { &*iter.cast::<MockIterState>() };
    for term in &state.terms {
        if matches!(term.mode, AccessMode::WRITE | AccessMode::OPTIONAL_WRITE) {
            return FfiResult::PERMISSION_DENIED;
        }
    }
    let world_ptr = state.world;
    let chunk_count = unsafe { &*world_ptr }.chunks.len();
    for chunk_idx in 0..chunk_count {
        let chunk_len = unsafe { &*world_ptr }.chunks[chunk_idx].entities.len();
        if chunk_len == 0 {
            continue;
        }
        let mut task_state = MockIterState {
            world: world_ptr,
            terms: state.terms.clone(),
            next_chunk_idx: chunk_idx,
            end_chunk_idx: Some(chunk_idx + 1),
            active_chunk_idx: None,
        };
        let status = unsafe { cb((&raw mut task_state).cast::<BamIter>(), user_data) };
        if !status.is_ok() {
            return status;
        }
    }
    FfiResult::OK
}

/// Mock `BamHostApi::register_system_handle` (slot 60) returning a decoupled `BamSystem` handle.
unsafe extern "C" fn host_register_system_handle(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
    out_system: *mut BamSystem,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_system.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    let desc_ref = unsafe { &*desc };
    match push_system(world, desc_ref) {
        Ok(handle) => {
            unsafe { *out_system = handle };
            FfiResult::OK
        }
        Err(err) => err,
    }
}

/// Mock `BamHostApi::configure_schedule_set` (slot 61) registering a schedule system set.
unsafe extern "C" fn host_configure_schedule_set(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamScheduleSetDesc,
    out_set: *mut BamSystemSet,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() || out_set.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let desc_ref = unsafe { &*desc };
    let st = desc_ref.validate();
    if !st.is_ok() {
        return st;
    }
    let Ok(name) = unsafe { CStr::from_ptr(desc_ref.name) }.to_str() else {
        return FfiResult::INVALID_UTF8;
    };
    let before_sets = if desc_ref.before_count > 0 && !desc_ref.before_sets.is_null() {
        unsafe { core::slice::from_raw_parts(desc_ref.before_sets, desc_ref.before_count as usize) }
            .to_vec()
    } else {
        Vec::new()
    };
    let after_sets = if desc_ref.after_count > 0 && !desc_ref.after_sets.is_null() {
        unsafe { core::slice::from_raw_parts(desc_ref.after_sets, desc_ref.after_count as usize) }
            .to_vec()
    } else {
        Vec::new()
    };
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    let handle = BamSystemSet::from_raw(world.schedule_sets.len() as u32);
    world.schedule_sets.push(RegisteredScheduleSet {
        handle,
        name: name.to_owned(),
        schedule: desc_ref.schedule,
        flags: desc_ref.flags,
        before_sets,
        after_sets,
    });
    unsafe { *out_set = handle };
    FfiResult::OK
}

/// Mock `BamHostApi::mount_system_in_schedule` (slot 62) mounting a system into a schedule/set.
unsafe extern "C" fn host_mount_system_in_schedule(
    load_ctx: *mut BamLoadCtx,
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
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    if (system.to_raw() as usize) >= world.systems.len() {
        return FfiResult::NOT_FOUND;
    }
    if set.is_valid() && (set.to_raw() as usize) >= world.schedule_sets.len() {
        return FfiResult::NOT_FOUND;
    }
    world.mounted_systems.push((system, schedule, set));
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_checkpoint` (slot 63) returning the current command-log watermark.
unsafe extern "C" fn host_cmd_checkpoint(
    cmds: *mut BamCommands,
    out_checkpoint: *mut usize,
) -> FfiResult {
    if cmds.is_null() || out_checkpoint.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &*cmds.cast::<MockV03World>() };
    unsafe { *out_checkpoint = world.commands.len() };
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_rollback` (slot 64) truncating the command log back to `checkpoint`.
unsafe extern "C" fn host_cmd_rollback(cmds: *mut BamCommands, checkpoint: usize) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    if checkpoint > world.commands.len() {
        return FfiResult::OUT_OF_RANGE;
    }
    world.commands.truncate(checkpoint);
    world.rollback_calls += 1;
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_insert_relation` (slot 65) enforcing exclusivity/acyclic/degree rules.
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
    if entity.is_null() || target.is_null() || !relation.is_valid() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    let Some(rel) = world.relations.get(&relation).cloned() else {
        return FfiResult::NOT_FOUND;
    };
    if data_size != rel.data_size {
        return FfiResult::TYPE_MISMATCH;
    }
    if data_size > 0 && data.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if (rel.flags & BAM_RELATION_FLAG_ACYCLIC) != 0
        && world.would_create_cycle(relation, entity, target)
    {
        return FfiResult::CYCLE_DETECTED;
    }
    let payload = if data_size > 0 {
        unsafe { core::slice::from_raw_parts(data.cast::<u8>(), data_size) }.to_vec()
    } else {
        Vec::new()
    };
    world
        .relation_edges
        .entry((relation, entity))
        .or_default()
        .push(target);
    world.commands.push(RecordedCmd::InsertRelation {
        entity,
        relation,
        target,
        payload,
    });
    FfiResult::OK
}

/// Mock `BamHostApi::cmd_remove_relation` (slot 66) removing a relation edge between entities.
unsafe extern "C" fn host_cmd_remove_relation(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    relation: BamComponent,
    target: FfiEntity,
) -> FfiResult {
    if cmds.is_null() {
        return FfiResult::NULL_POINTER;
    }
    if entity.is_null() || target.is_null() || !relation.is_valid() {
        return FfiResult::INVALID_ARGUMENT;
    }
    let world = unsafe { &mut *cmds.cast::<MockV03World>() };
    if !world.relations.contains_key(&relation) {
        return FfiResult::NOT_FOUND;
    }
    if let Some(edges) = world.relation_edges.get_mut(&(relation, entity)) {
        edges.retain(|&t| t != target);
    }
    world.commands.push(RecordedCmd::RemoveRelation {
        entity,
        relation,
        target,
    });
    FfiResult::OK
}

/// Mock `BamHostApi::migrate_component_column` (slot 67) invoking the guest migration callback per row.
unsafe extern "C" fn host_migrate_component_column(
    load_ctx: *mut BamLoadCtx,
    component: BamComponent,
    old_fingerprint: u64,
    new_desc: *const ComponentDesc,
    migrate_fn: Option<BamRowMigrateFn>,
    user_data: *mut c_void,
) -> FfiResult {
    if load_ctx.is_null() || new_desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let Some(cb) = migrate_fn else {
        return FfiResult::NULL_POINTER;
    };
    let desc_ref = unsafe { &*new_desc };
    let st = desc_ref.validate();
    if !st.is_ok() {
        return st;
    }
    let world = unsafe { &mut *load_ctx.cast::<MockV03World>() };
    let Some(old_meta) = world.comp_meta(component).cloned() else {
        return FfiResult::NOT_FOUND;
    };
    if old_fingerprint != 0
        && old_meta.schema_fingerprint != 0
        && old_meta.schema_fingerprint != old_fingerprint
    {
        return FfiResult::TYPE_MISMATCH;
    }

    let mut migrated_chunks: Vec<(usize, Vec<u64>)> = Vec::new();
    for (chunk_idx, chunk) in world.chunks.iter().enumerate() {
        let Some(old_words) = chunk.columns.get(&component) else {
            continue;
        };
        let row_count = chunk.entities.len();
        let new_byte_len = row_count * desc_ref.size;
        let new_word_len = new_byte_len.div_ceil(8).max(1);
        let mut new_words = vec![0u64; new_word_len];
        let old_base = old_words.as_ptr().cast::<u8>();
        let new_base = new_words.as_mut_ptr().cast::<u8>();
        for row in 0..row_count {
            let old_row_ptr = if old_meta.size == 0 {
                core::ptr::NonNull::<u8>::dangling()
                    .as_ptr()
                    .cast::<c_void>()
            } else {
                unsafe { old_base.add(row * old_meta.size).cast::<c_void>() }
            };
            let new_row_ptr = if desc_ref.size == 0 {
                core::ptr::NonNull::<u8>::dangling()
                    .as_ptr()
                    .cast::<c_void>()
            } else {
                unsafe { new_base.add(row * desc_ref.size).cast::<c_void>() }
            };
            let row_status = unsafe { cb(old_row_ptr, new_row_ptr, user_data) };
            if !row_status.is_ok() {
                return row_status;
            }
        }
        migrated_chunks.push((chunk_idx, new_words));
    }

    for (chunk_idx, new_words) in migrated_chunks {
        world.chunks[chunk_idx].columns.insert(component, new_words);
    }
    let new_fields = if !desc_ref.fields.is_null() && desc_ref.field_count > 0 {
        unsafe { core::slice::from_raw_parts(desc_ref.fields, desc_ref.field_count) }.to_vec()
    } else {
        Vec::new()
    };
    if let Some(meta) = world.comp_meta_mut(component) {
        meta.uuid = desc_ref.uuid;
        meta.schema_fingerprint = desc_ref.schema_fingerprint;
        meta.size = desc_ref.size;
        meta.align = desc_ref.align;
        meta.flags = desc_ref.flags;
        meta.fields = new_fields;
    }
    if !desc_ref.uuid.is_zero() {
        world.uuid_to_comp.insert(desc_ref.uuid, component);
    }
    FfiResult::OK
}

/// Constructs a v0.2-only `BamHostApi` (with ABI 0.3.0 slots null and optionally truncated `struct_size`).
fn make_v02_only_host_api(truncated_struct_size: bool) -> BamHostApi {
    BamHostApi {
        struct_size: if truncated_struct_size {
            BamHostApi::MIN_V0_2_SIZE as u32
        } else {
            core::mem::size_of::<BamHostApi>() as u32
        },
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
        iter_column_readonly: Some(host_iter_col_ro),
        iter_column_mutable: Some(host_iter_col_mut),
        iter_column_mutable_untracked: Some(host_iter_col_mut_untracked),
        iter_mark_changed_range: Some(host_iter_mark_changed),
        iter_column_rows_readonly: Some(host_iter_rows_ro),
        iter_column_rows_mutable: Some(host_iter_rows_mut),
        iter_column_info: Some(host_iter_col_info),
        res_ptr: Some(host_res_ptr),
        res_mut_ptr: Some(host_res_mut_ptr),
        cmd_spawn_empty: Some(host_cmd_spawn_empty),
        cmd_spawn: Some(host_cmd_spawn),
        cmd_despawn: Some(host_cmd_despawn),
        cmd_insert: Some(host_cmd_insert),
        cmd_insert_default: Some(host_cmd_insert_default),
        cmd_remove: Some(host_cmd_remove),
        par_for_each_chunk: None,
        sys_entity_get: None,
        sys_entity_get_mut: None,
        msg_read_batch: None,
        msg_send: None,
        msg_write_batch: None,
        res_ticks: None,
        sys_res_ptr: None,
        sys_res_mut_ptr: None,
        register_observer: None,
        cmd_trigger: None,
        cmd_spawn_persistent: None,
        cmd_spawn_batch: None,
        cmd_despawn_all: None,
        cmd_insert_if_new: None,
        cmd_trigger_ex: None,
        cmd_observe: None,
        iter_read_removed: None,
        sys_entity_children: None,
        asset_load: None,
        asset_get_load_state: None,
        asset_release: None,
        cmd_spawn_scene: None,
        log_diagnostic: None,
        report_guest_fault: None,
        resolve_component_by_uuid: None,
        component_layout_info: None,
        register_relation: None,
        intern_symbol: None,
        bind_named_object: None,
        iter_count: None,
        iter_up_cascade: None,
        par_for_each_chunk_readonly: None,
        register_system_handle: None,
        configure_schedule_set: None,
        mount_system_in_schedule: None,
        cmd_checkpoint: None,
        cmd_rollback: None,
        cmd_insert_relation: None,
        cmd_remove_relation: None,
        migrate_component_column: None,
        iter_next_chunk_batch: None,
        iter_column_ticks: None,
        cmd_clone_entity: None,
        cmd_insert_resource: None,
        cmd_remove_resource: None,
        register_resource: None,
        register_message: None,
        resolve_named_object: None,
        resolve_symbol_str: None,
        iter_column_rows_mutable_untracked: None,
        iter_mark_changed_ranges: None,
        sys_current_ticks: None,
        sys_time_info: None,
        par_for_each_chunk_ex: None,
        sys_relation_targets: None,
        sys_relation_sources: None,
        sys_relation_edge_data: None,
        cmd_despawn_batch: None,
        cmd_insert_batch: None,
        cmd_remove_batch: None,
        msg_read_slice: None,
        cmd_spawn_batch_ex: None,
    }
}

/// Mock `BamHostApi::iter_next_chunk_batch` (slot 68) advancing chunks and filling column pointers.
unsafe extern "C" fn host_iter_next_chunk_batch(
    iter: *mut BamIter,
    out_len: *mut usize,
    out_entities: *mut *const FfiEntity,
    out_col_ptrs: *mut *mut u8,
    col_count: usize,
    _mark_changed_mask: u32,
) -> FfiBool {
    let state = unsafe { &mut *iter.cast::<MockIterState>() };
    let world = unsafe { &mut *state.world };
    let limit = state
        .end_chunk_idx
        .unwrap_or(world.chunks.len())
        .min(world.chunks.len());
    while state.next_chunk_idx < limit {
        let idx = state.next_chunk_idx;
        state.next_chunk_idx += 1;
        let len = world.chunks[idx].entities.len();
        if len > 0 {
            state.active_chunk_idx = Some(idx);
            unsafe { *out_len = len };
            if !out_entities.is_null() {
                unsafe { *out_entities = world.chunks[idx].entities.as_ptr() };
            }
            for term_idx in 0..col_count {
                let ptr = if let Some(term) = state.terms.get(term_idx) {
                    world.chunks[idx]
                        .columns
                        .get_mut(&term.component)
                        .map_or(core::ptr::null_mut(), |w| w.as_mut_ptr().cast::<u8>())
                } else {
                    core::ptr::null_mut()
                };
                unsafe { *out_col_ptrs.add(term_idx) = ptr };
            }
            return FfiBool::TRUE;
        }
    }
    state.active_chunk_idx = None;
    unsafe { *out_len = 0 };
    FfiBool::FALSE
}

/// Constructs a complete ABI `0.3.0` `BamHostApi` wired with slot 68 (`iter_next_chunk_batch`).
fn make_v03_host_api_with_batch() -> BamHostApi {
    let mut api = make_v03_host_api();
    api.iter_next_chunk_batch = Some(host_iter_next_chunk_batch);
    api
}

/// Constructs an ABI `0.3.0` (73-slot) `BamHostApi` wired to `host_*` callbacks.
fn make_v03_host_api() -> BamHostApi {
    let mut api = make_v02_only_host_api(false);
    api.cmd_spawn_batch = Some(host_cmd_spawn_batch);
    api.log_diagnostic = Some(host_log_diagnostic);
    api.report_guest_fault = Some(host_report_guest_fault);
    api.resolve_component_by_uuid = Some(host_resolve_component_by_uuid);
    api.component_layout_info = Some(host_component_layout_info);
    api.register_relation = Some(host_register_relation);
    api.intern_symbol = Some(host_intern_symbol);
    api.bind_named_object = Some(host_bind_named_object);
    api.iter_count = Some(host_iter_count);
    api.iter_up_cascade = Some(host_iter_up_cascade);
    api.par_for_each_chunk_readonly = Some(host_par_for_each_chunk_readonly);
    api.register_system_handle = Some(host_register_system_handle);
    api.configure_schedule_set = Some(host_configure_schedule_set);
    api.mount_system_in_schedule = Some(host_mount_system_in_schedule);
    api.cmd_checkpoint = Some(host_cmd_checkpoint);
    api.cmd_rollback = Some(host_cmd_rollback);
    api.cmd_insert_relation = Some(host_cmd_insert_relation);
    api.cmd_remove_relation = Some(host_cmd_remove_relation);
    api.migrate_component_column = Some(host_migrate_component_column);
    api.res_ticks = Some(host_res_ticks);
    api.msg_read_batch = Some(host_msg_read_batch);
    api.sys_entity_get_mut = Some(host_sys_entity_get_mut);
    api.iter_column_ticks = Some(host_iter_column_ticks);
    api.cmd_clone_entity = Some(host_cmd_clone_entity);
    api.cmd_insert_resource = Some(host_cmd_insert_resource);
    api.cmd_remove_resource = Some(host_cmd_remove_resource);
    api
}

// ============================================================================
// Test Component Types (works with both default features and `--all-features`)
// ============================================================================

/// Helper macro defining a `#[repr(C)]` `AbiPod` + `GuestComponent` test fixture.
macro_rules! define_pod_component {
    ($vis:vis struct $name:ident { $($field:ident : $ty:ty),* $(,)? } => $cname:literal) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        $vis struct $name {
            $(pub $field: $ty,)*
        }
        unsafe impl RawInsertable for $name {}
        unsafe impl AbiPod for $name {}
        impl GuestComponent for $name {
            const NAME: &'static CStr = $cname;
        }
    };
}

define_pod_component!(struct Faction { id: u32 } => c"v03::Faction");
define_pod_component!(struct Counter { value: u32 } => c"v03::Counter");
define_pod_component!(struct EdgeWeight { weight: f32 } => c"v03::EdgeWeight");
define_pod_component!(struct ParentRel {} => c"v03::ParentRel");

/// Helper macro implementing `ModComponent` with explicit schema fingerprint and field descriptors.
macro_rules! impl_manual_mod_component {
    ($ty:ty, $name:literal, $cstr:literal, $fp:expr, $fields:expr) => {
        impl ModComponent for $ty {
            const CANONICAL_NAME: &'static str = $name;
            const COMPONENT_CSTR: &'static CStr = $cstr;
            const UUID: ComponentUuid = ComponentUuid::from_parts(
                $name,
                core::mem::size_of::<$ty>(),
                core::mem::align_of::<$ty>(),
                $fp,
            );
            const IS_ABI_POD: bool = true;
            const IS_RAW_INSERTABLE: bool = true;
            const SCHEMA_FINGERPRINT: u64 = $fp;
            const BAM_FIELDS: &'static [BamFieldDesc] = $fields;
            type VTable = VTableHeader;
            /// Returns a static unit VTable placeholder.
            fn vtable() -> &'static Self::VTable {
                static VT: VTableHeader = VTableHeader::from_raw_parts(
                    core::mem::size_of::<VTableHeader>() as u32,
                    <$ty as ModComponent>::UUID,
                    core::mem::size_of::<$ty>(),
                    core::mem::align_of::<$ty>(),
                );
                &VT
            }
            /// Returns an empty symbol table for manual test fixtures.
            fn symbols() -> &'static [SymbolEntry] {
                &[]
            }
        }
    };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Legacy schema v1 stats fixture used as the migration source in column-migration tests.
struct OldStats {
    hp: u32,
}
unsafe impl RawInsertable for OldStats {}
unsafe impl AbiPod for OldStats {}
impl_manual_mod_component!(
    OldStats,
    "v03::Stats",
    c"v03::Stats",
    0x1111_2222_3333_4444,
    &[BamFieldDesc::new(
        BamFieldKind::U32,
        StrView::from_str("hp"),
        StrView::from_str("u32"),
        0,
        4,
        4,
    )]
);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Upgraded schema v2 stats fixture used as the migration target in column-migration tests.
struct NewStats {
    hp: f64,
    armor: u32,
    pad: u32,
}
unsafe impl RawInsertable for NewStats {}
unsafe impl AbiPod for NewStats {}
impl_manual_mod_component!(
    NewStats,
    "v03::Stats",
    c"v03::Stats",
    0x5555_6666_7777_8888,
    &[
        BamFieldDesc::new(
            BamFieldKind::F64,
            StrView::from_str("hp"),
            StrView::from_str("f64"),
            0,
            8,
            8,
        ),
        BamFieldDesc::new(
            BamFieldKind::U32,
            StrView::from_str("armor"),
            StrView::from_str("u32"),
            8,
            4,
            4,
        ),
    ]
);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Fixture with matching size/align but a deliberately wrong `SCHEMA_FINGERPRINT`.
struct BadFingerprintStats {
    hp: u32,
}
unsafe impl RawInsertable for BadFingerprintStats {}
unsafe impl AbiPod for BadFingerprintStats {}
impl_manual_mod_component!(
    BadFingerprintStats,
    "v03::Stats",
    c"v03::Stats",
    0xDEAD_BEEF_CAFE_BABE,
    &[]
);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Fixture with a deliberately mismatched byte size for `verify_component_layout` rejection.
struct BadSizeStats {
    hp: u64,
}
unsafe impl RawInsertable for BadSizeStats {}
unsafe impl AbiPod for BadSizeStats {}
impl_manual_mod_component!(
    BadSizeStats,
    "v03::Stats",
    c"v03::Stats",
    0x1111_2222_3333_4444,
    &[]
);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Fixture with a deliberately mismatched alignment for `verify_component_layout` rejection.
struct BadAlignStats {
    bytes: [u8; 4],
}
unsafe impl RawInsertable for BadAlignStats {}
unsafe impl AbiPod for BadAlignStats {}
impl_manual_mod_component!(
    BadAlignStats,
    "v03::Stats",
    c"v03::Stats",
    0x1111_2222_3333_4444,
    &[]
);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Zero-sized source marker fixture for ZST-to-ZST column migration testing.
struct OldTag;
unsafe impl RawInsertable for OldTag {}
unsafe impl AbiPod for OldTag {}
impl_manual_mod_component!(OldTag, "v03::Tag", c"v03::Tag", 0x0000_0000_0000_0001, &[]);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
/// Zero-sized target marker fixture for ZST-to-ZST column migration testing.
struct NewTag;
unsafe impl RawInsertable for NewTag {}
unsafe impl AbiPod for NewTag {}
impl_manual_mod_component!(NewTag, "v03::Tag", c"v03::Tag", 0x0000_0000_0000_0002, &[]);

define_pod_component!(struct C0 { v: u32 } => c"q16::C0");
define_pod_component!(struct C1 { v: u32 } => c"q16::C1");
define_pod_component!(struct C2 { v: u32 } => c"q16::C2");
define_pod_component!(struct C3 { v: u32 } => c"q16::C3");
define_pod_component!(struct C4 { v: u32 } => c"q16::C4");
define_pod_component!(struct C5 { v: u32 } => c"q16::C5");
define_pod_component!(struct C6 { v: u32 } => c"q16::C6");
define_pod_component!(struct C7 { v: u32 } => c"q16::C7");
define_pod_component!(struct C8 { v: u32 } => c"q16::C8");
define_pod_component!(struct C9 { v: u32 } => c"q16::C9");
define_pod_component!(struct C10 { v: u32 } => c"q16::C10");
define_pod_component!(struct C11 { v: u32 } => c"q16::C11");
define_pod_component!(struct C12 { v: u32 } => c"q16::C12");
define_pod_component!(struct C13 { v: u32 } => c"q16::C13");
define_pod_component!(struct C14 { v: u32 } => c"q16::C14");
define_pod_component!(struct C15 { v: u32 } => c"q16::C15");

// ============================================================================
// 1. ABI 0.2 Truncated Table & Missing Slot `UNSUPPORTED` Fallbacks
// ============================================================================

/// Scenario: Loads a guest against both a truncated-`struct_size` v0.2 host and a full-size host
/// with null ABI 0.3.0 slots (50..67), asserting every v0.3 wrapper cleanly returns `UNSUPPORTED`.
#[test]
fn abi_v02_truncated_and_missing_slots_return_unsupported() {
    for truncated in [true, false] {
        let mut host = V03TestHost::with_api(make_v02_only_host_api(truncated));
        let comp_h = host.expose_component::<Faction>("v03::Faction", Faction::UUID, 0, &[]);
        let rel_h = host.expose_component::<ParentRel>("v03::ParentRel", ParentRel::UUID, 0, &[]);
        let mut chunk = MockChunk::new(vec![FfiEntity::from_bits(1)]);
        chunk.insert_dense(comp_h, &[Faction { id: 1 }]);
        host.chunks.push(chunk);

        let inst = host
            .load_fn(c"fallback_mod", |app| {
                // Slots 50..=56 & 60..=62 & 67 during init
                assert_eq!(
                    app.log_diagnostic(BAM_LOG_INFO, "hello"),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(guest_log!(app, info, "macro {}", 1), FfiResult::UNSUPPORTED);
                assert_eq!(app.report_fault("fault"), FfiResult::UNSUPPORTED);
                assert_eq!(
                    app.resolve_component_by_uuid(Faction::UUID),
                    Err(FfiResult::UNSUPPORTED)
                );
                assert_eq!(
                    app.component_layout_info(comp_h).unwrap_err(),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(
                    app.verify_component_layout_handle::<OldStats>(comp_h)
                        .unwrap_err(),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(
                    app.register_relation(RelationDescBuilder::new("likes")),
                    Err(FfiResult::UNSUPPORTED)
                );
                assert_eq!(app.intern_symbol("sym"), Err(FfiResult::UNSUPPORTED));
                assert_eq!(
                    app.bind_named_object("hero", FfiEntity::from_bits(1)),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(
                    app.system(c"decoupled")
                        .query_typed::<&Faction>()
                        .register_handle(|_| FfiResult::OK),
                    Err(FfiResult::UNSUPPORTED)
                );
                assert_eq!(
                    app.configure_schedule_set(ScheduleSetBuilder::new("set_a")),
                    Err(FfiResult::UNSUPPORTED)
                );
                assert_eq!(
                    app.mount_system(
                        BamSystem::from_raw(0),
                        ScheduleId::UPDATE,
                        BamSystemSet::INVALID
                    ),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(
                    app.migrate_component_column::<OldStats, NewStats, _>(comp_h, |old| {
                        Ok(NewStats {
                            hp: old.hp as f64,
                            armor: 0,
                            pad: 0,
                        })
                    }),
                    FfiResult::UNSUPPORTED
                );
                assert_eq!(
                    app.migrate_pod_component_column::<Faction, Counter, _>(comp_h, 0, 0, |f| Ok(
                        Counter { value: f.id }
                    )),
                    FfiResult::UNSUPPORTED
                );

                // Register an ABI 0.2 system to verify runtime slots 57..=59 & 63..=66
                app.system(c"check_runtime_fallbacks")
                    .query_typed::<&Faction>()
                    .build(move |ctx| {
                        assert_eq!(
                            ctx.log_diagnostic(BAM_LOG_WARN, "w"),
                            FfiResult::UNSUPPORTED
                        );
                        assert_eq!(ctx.report_fault("rf"), FfiResult::UNSUPPORTED);
                        assert_eq!(ctx.intern_symbol("x"), Err(FfiResult::UNSUPPORTED));
                        assert_eq!(
                            ctx.bind_named_object("x", FfiEntity::from_bits(1)),
                            FfiResult::UNSUPPORTED
                        );
                        assert_eq!(
                            ctx.resolve_component_by_uuid(Faction::UUID),
                            Err(FfiResult::UNSUPPORTED)
                        );
                        assert_eq!(
                            ctx.component_layout_info(comp_h).unwrap_err(),
                            FfiResult::UNSUPPORTED
                        );

                        assert_eq!(ctx.iter().count(), Err(FfiResult::UNSUPPORTED));
                        assert_eq!(
                            ctx.single_query::<&Faction>().count(),
                            Err(FfiResult::UNSUPPORTED)
                        );
                        assert_eq!(
                            ctx.iter()
                                .up_cascade::<Faction>(rel_h, FfiEntity::from_bits(1), 0),
                            Err(FfiResult::UNSUPPORTED)
                        );
                        assert_eq!(
                            ctx.single_query::<&Faction>()
                                .up_cascade::<ParentRel, Faction>(FfiEntity::from_bits(1), 0),
                            Err(FfiResult::UNSUPPORTED)
                        );
                        assert_eq!(ctx.par_chunks_readonly(|_| {}), FfiResult::UNSUPPORTED);
                        assert_eq!(
                            ctx.single_query::<&Faction>()
                                .par_for_each_chunk_readonly(|_| FfiResult::OK),
                            FfiResult::UNSUPPORTED
                        );

                        let cmds = ctx.commands();
                        assert_eq!(cmds.checkpoint(), Err(FfiResult::UNSUPPORTED));
                        assert_eq!(cmds.rollback(0), FfiResult::UNSUPPORTED);
                        assert_eq!(cmds.transaction(|_| Ok(())), Err(FfiResult::UNSUPPORTED));
                        assert_eq!(
                            cmds.insert_relation(
                                FfiEntity::from_bits(1),
                                rel_h,
                                FfiEntity::from_bits(2)
                            ),
                            FfiResult::UNSUPPORTED
                        );
                        assert_eq!(
                            cmds.insert_relation_with_payload(
                                FfiEntity::from_bits(1),
                                rel_h,
                                FfiEntity::from_bits(2),
                                &EdgeWeight { weight: 1.0 }
                            ),
                            FfiResult::UNSUPPORTED
                        );
                        assert_eq!(
                            cmds.remove_relation(
                                FfiEntity::from_bits(1),
                                rel_h,
                                FfiEntity::from_bits(2)
                            ),
                            FfiResult::UNSUPPORTED
                        );
                        FfiResult::OK
                    })
            })
            .expect("init must succeed even when v0.3 slots are unsupported");

        assert_eq!(host.run_system(0), FfiResult::OK);
        host.unload(inst);
    }
}

// ============================================================================
// 2. Diagnostics (`log_diagnostic`, `guest_log!`) & `report_fault`
// ============================================================================

/// Scenario: Exercises `log_diagnostic` (slot 50) and `report_fault` (slot 51) across `GuestApp`,
/// `SystemContext`, and `RunConditionContext`, verifying recorded levels, messages, and `last_error`.
#[test]
fn diagnostics_guest_log_and_report_fault() {
    let host = V03TestHost::new();
    let inst = host
        .load_fn(c"diag_mod", |app| {
            assert_eq!(guest_log!(app, trace, "boot trace"), FfiResult::OK);
            assert_eq!(guest_log!(app, debug, "boot debug {}", 7), FfiResult::OK);
            assert_eq!(
                app.log_diagnostic(BAM_LOG_INFO, "direct info"),
                FfiResult::OK
            );
            assert_eq!(
                app.log_diagnostic(99, "bad level"),
                FfiResult::INVALID_ARGUMENT
            );
            // `log_diagnostic` must NOT clobber `last_error`.
            assert_eq!(app.host().last_error(), "initial_host_error");

            app.system(c"diag_sys")
                .run_if(|rctx| {
                    assert_eq!(guest_log!(rctx, trace, "run_if check"), FfiResult::OK);
                    true
                })
                .build(|ctx| {
                    let count = 42u32;
                    assert_eq!(guest_log!(ctx, info, "static info"), FfiResult::OK);
                    assert_eq!(guest_log!(ctx, warn, "count={count}"), FfiResult::OK);
                    assert_eq!(guest_log!(ctx, error, "err: {}", "disk"), FfiResult::OK);
                    assert_eq!(
                        guest_log!(ctx, BAM_LOG_DEBUG, "explicit level"),
                        FfiResult::OK
                    );
                    assert_eq!(ctx.host().last_error(), "initial_host_error");

                    assert_eq!(ctx.report_fault(""), FfiResult::INVALID_ARGUMENT);
                    assert_eq!(ctx.report_fault("tick fault recorded"), FfiResult::OK);
                    assert_eq!(ctx.host().last_error(), "tick fault recorded");
                    FfiResult::OK
                })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(
        host.logs,
        vec![
            (BAM_LOG_TRACE, "boot trace".to_owned()),
            (BAM_LOG_DEBUG, "boot debug 7".to_owned()),
            (BAM_LOG_INFO, "direct info".to_owned()),
            (BAM_LOG_TRACE, "run_if check".to_owned()),
            (BAM_LOG_INFO, "static info".to_owned()),
            (BAM_LOG_WARN, "count=42".to_owned()),
            (BAM_LOG_ERROR, "err: disk".to_owned()),
            (BAM_LOG_DEBUG, "explicit level".to_owned()),
        ]
    );
    assert_eq!(host.faults, vec!["tick fault recorded".to_owned()]);
    host.unload(inst);
}

// ============================================================================
// 3. UUID Resolution, `component_layout_info`, & `verify_component_layout`
// ============================================================================

/// Scenario: Exercises `resolve_component_by_uuid` (slot 52), `component_layout_info` (slot 53),
/// and `verify_component_layout`, confirming matching schemas pass while mismatched fingerprint,
/// size, or alignment fail with `TYPE_MISMATCH`.
#[test]
fn resolve_by_uuid_layout_info_and_verify_layout() {
    let mut host = V03TestHost::new();
    let stats_h = host.expose_component::<OldStats>(
        OldStats::CANONICAL_NAME,
        OldStats::UUID,
        OldStats::SCHEMA_FINGERPRINT,
        OldStats::BAM_FIELDS,
    );

    let inst = host
        .load_fn(c"layout_mod", |app| {
            assert_eq!(app.resolve_component_by_uuid(OldStats::UUID), Ok(stats_h));
            assert_eq!(
                app.resolve_component_by_uuid(ComponentUuid::ZERO),
                Err(FfiResult::INVALID_ARGUMENT)
            );
            assert_eq!(
                app.resolve_component_by_uuid(ComponentUuid::from_u128(0x1234)),
                Err(FfiResult::NOT_FOUND)
            );

            let info = app.component_layout_info(stats_h).unwrap();
            assert_eq!(info.uuid, OldStats::UUID);
            assert_eq!(info.schema_fingerprint, OldStats::SCHEMA_FINGERPRINT);
            assert_eq!(info.size, core::mem::size_of::<OldStats>());
            assert_eq!(info.align, core::mem::align_of::<OldStats>());
            assert_eq!(info.field_count, 1);

            // Matching layout + schema fingerprint passes.
            let verified = app.verify_component_layout::<OldStats>().unwrap();
            assert_eq!(verified.schema_fingerprint, OldStats::SCHEMA_FINGERPRINT);

            // Mismatched fingerprint, size, or align all return TYPE_MISMATCH.
            assert_eq!(
                app.verify_component_layout_handle::<BadFingerprintStats>(stats_h)
                    .unwrap_err(),
                FfiResult::TYPE_MISMATCH
            );
            assert_eq!(
                app.verify_component_layout_handle::<BadSizeStats>(stats_h)
                    .unwrap_err(),
                FfiResult::TYPE_MISMATCH
            );
            assert_eq!(
                app.verify_component_layout_handle::<BadAlignStats>(stats_h)
                    .unwrap_err(),
                FfiResult::TYPE_MISMATCH
            );

            app.system(c"check_layout_in_sys").build(move |ctx| {
                assert_eq!(ctx.resolve_component_by_uuid(OldStats::UUID), Ok(stats_h));
                assert!(ctx.verify_component_layout::<OldStats>().is_ok());
                assert_eq!(
                    ctx.verify_component_layout_handle::<BadFingerprintStats>(stats_h)
                        .unwrap_err(),
                    FfiResult::TYPE_MISMATCH
                );
                FfiResult::OK
            })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    host.unload(inst);
}

/// Scenario: Verifies that `#[derive(ModComponent)]` computes a non-zero `SCHEMA_FINGERPRINT` and
/// accurate `BAM_FIELDS` descriptors that pass host `verify_component_layout` checks.
#[cfg(feature = "derive")]
#[test]
fn derived_mod_component_schema_fingerprint_and_fields_verified() {
    /// Derived `ModComponent` fixture verifying generated `SCHEMA_FINGERPRINT` and `BAM_FIELDS`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, ModComponent)]
    #[mod_component(name = "v03::DerivedHero")]
    struct DerivedHero {
        hp: f32,
        mana: f32,
        level: u32,
    }

    let host = V03TestHost::new();
    let inst = host
        .load_fn(c"derive_v03_mod", |app| {
            let handle = app.register_dynamic_component::<DerivedHero>().unwrap();
            let info = app.verify_component_layout::<DerivedHero>().unwrap();
            assert_eq!(info.size, core::mem::size_of::<DerivedHero>());
            assert_eq!(info.align, core::mem::align_of::<DerivedHero>());
            assert_eq!(info.schema_fingerprint, DerivedHero::SCHEMA_FINGERPRINT);
            assert_ne!(info.schema_fingerprint, 0);
            assert_eq!(info.field_count, 3);
            assert_eq!(app.resolve_component_by_uuid(DerivedHero::UUID), Ok(handle));
            FfiResult::OK
        })
        .unwrap();
    host.unload(inst);
}

// ============================================================================
// 4. Typed Relationships (`RelationDescBuilder`, `insert_relation`, Cycles)
// ============================================================================

/// Scenario: Exercises `RelationDescBuilder` + `register_relation` (slot 54), `insert_relation`
/// (slot 65), and `remove_relation` (slot 66), verifying exclusivity, out-degree limits, payload
/// edges, and `CYCLE_DETECTED` rejection on acyclic graphs.
#[test]
fn relations_builder_commands_and_cycle_detection() {
    let host = V03TestHost::new();
    let mut likes_handle = BamComponent::INVALID;
    let mut weighted_handle = BamComponent::INVALID;
    let mut parent_handle = BamComponent::INVALID;

    let inst = host
        .load_fn(c"rel_mod", |app| {
            let likes_desc = RelationDescBuilder::new("likes")
                .exclusive()
                .acyclic()
                .cascade_despawn()
                .max_degree(2);
            assert_eq!(
                likes_desc.raw_flags(),
                BAM_RELATION_FLAG_EXCLUSIVE
                    | BAM_RELATION_FLAG_ACYCLIC
                    | BAM_RELATION_FLAG_CASCADE_DESPAWN
            );
            assert_eq!(likes_desc.configured_max_degree(), 2);
            likes_handle = app.register_relation(likes_desc).unwrap();

            let weighted_desc = RelationDescBuilder::new("v03::WeightedLink")
                .symmetric()
                .cleanup_remove()
                .cascade_delete()
                .with_payload::<EdgeWeight>();
            assert_eq!(
                weighted_desc.data_size(),
                core::mem::size_of::<EdgeWeight>()
            );
            assert_eq!(
                weighted_desc.data_align(),
                core::mem::align_of::<EdgeWeight>()
            );
            weighted_handle = app.register_relation(weighted_desc).unwrap();

            parent_handle = app
                .register_relation(RelationDescBuilder::for_component::<ParentRel>().acyclic())
                .unwrap();

            // Empty relation name rejected
            assert_eq!(
                app.register_relation(RelationDescBuilder::new("")),
                Err(FfiResult::INVALID_ARGUMENT)
            );

            let likes = likes_handle;
            let weighted = weighted_handle;
            app.system(c"rel_ops").build(move |ctx| {
                let cmds = ctx.commands();
                let a = FfiEntity::from_bits(10);
                let b = FfiEntity::from_bits(20);
                let c = FfiEntity::from_bits(30);

                // Marker relation chain A -> B -> C succeeds
                assert_eq!(cmds.insert_relation(a, likes, b), FfiResult::OK);
                assert_eq!(cmds.insert_relation(b, likes, c), FfiResult::OK);
                // Cycle C -> A rejected with CYCLE_DETECTED
                assert_eq!(cmds.insert_relation(c, likes, a), FfiResult::CYCLE_DETECTED);
                // Self-loop rejected with CYCLE_DETECTED
                assert_eq!(cmds.insert_relation(a, likes, a), FfiResult::CYCLE_DETECTED);

                // Payload relation
                assert_eq!(
                    cmds.insert_relation_with_payload(a, weighted, b, &EdgeWeight { weight: 7.5 }),
                    FfiResult::OK
                );
                // Wrong payload size rejected
                assert_eq!(
                    cmds.insert_relation_with_payload(a, likes, c, &EdgeWeight { weight: 1.0 }),
                    FfiResult::TYPE_MISMATCH
                );
                // Null entity rejected before FFI
                assert_eq!(
                    cmds.insert_relation(FfiEntity::NULL, likes, b),
                    FfiResult::INVALID_ENTITY
                );
                assert_eq!(cmds.remove_relation(b, likes, c), FfiResult::OK);
                FfiResult::OK
            })
        })
        .unwrap();

    // Verify auto-qualification of "likes" -> "rel_mod::likes"
    let registered_likes = host.relations.get(&likes_handle).unwrap();
    assert_eq!(registered_likes.name, "rel_mod::likes");
    assert_eq!(registered_likes.max_degree, 2);
    assert_eq!(
        registered_likes.flags,
        BAM_RELATION_FLAG_EXCLUSIVE | BAM_RELATION_FLAG_ACYCLIC | BAM_RELATION_FLAG_CASCADE_DESPAWN
    );
    let registered_weighted = host.relations.get(&weighted_handle).unwrap();
    assert_eq!(registered_weighted.name, "v03::WeightedLink");
    assert_eq!(registered_weighted.data_size, 4);
    assert_eq!(
        host.relations.get(&parent_handle).unwrap().name,
        "v03::ParentRel"
    );

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.commands.len(), 4);
    assert_eq!(
        host.commands[2],
        RecordedCmd::InsertRelation {
            entity: FfiEntity::from_bits(10),
            relation: weighted_handle,
            target: FfiEntity::from_bits(20),
            payload: 7.5f32.to_ne_bytes().to_vec(),
        }
    );
    assert_eq!(
        host.commands[3],
        RecordedCmd::RemoveRelation {
            entity: FfiEntity::from_bits(20),
            relation: likes_handle,
            target: FfiEntity::from_bits(30),
        }
    );
    host.unload(inst);
}

// ============================================================================
// 5. Symbol Interning & Named Object Binding
// ============================================================================

/// Scenario: Exercises `intern_symbol` (slot 55) deduplication and `bind_named_object` (slot 56)
/// across `GuestApp` and `SystemContext`, including invalid-argument rejections.
#[test]
fn symbol_interning_and_named_object_binding() {
    let host = V03TestHost::new();
    let e_hero = FfiEntity::from_bits(501);
    let e_boss = FfiEntity::from_bits(502);

    let inst = host
        .load_fn(c"sym_mod", |app| {
            let hero_a = app.intern_symbol("hero").unwrap();
            let villain = app.intern_symbol("villain").unwrap();
            let hero_b = app.intern_symbol("hero").unwrap();
            assert_eq!(hero_a, hero_b);
            assert_ne!(hero_a, villain);
            assert!(hero_a.is_valid());
            assert_eq!(app.intern_symbol(""), Err(FfiResult::INVALID_ARGUMENT));

            assert_eq!(app.bind_named_object("player_one", e_hero), FfiResult::OK);
            assert_eq!(
                app.bind_named_object("", e_hero),
                FfiResult::INVALID_ARGUMENT
            );
            assert_eq!(
                app.bind_named_object("null_obj", FfiEntity::NULL),
                FfiResult::INVALID_ENTITY
            );

            app.system(c"sym_sys").build(move |ctx| {
                assert_eq!(ctx.intern_symbol("hero"), Ok(hero_a));
                assert_eq!(ctx.bind_named_object("final_boss", e_boss), FfiResult::OK);
                FfiResult::OK
            })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.named_objects.get("player_one"), Some(&e_hero));
    assert_eq!(host.named_objects.get("final_boss"), Some(&e_boss));
    host.unload(inst);
}

// ============================================================================
// 6. `iter_count`, `iter_up_cascade`, & `par_for_each_chunk_readonly`
// ============================================================================

/// Scenario: Exercises `iter_count` (slot 57) non-destructive counting, `up_cascade` (slot 58)
/// ancestor lookup with cycle detection, and `par_chunks_readonly` (slot 59) confirming read-only
/// queries succeed while writable queries are rejected with `PERMISSION_DENIED`.
#[test]
fn iter_count_up_cascade_and_par_readonly() {
    let mut host = V03TestHost::new();
    let faction_h = host.expose_component::<Faction>("v03::Faction", Faction::UUID, 0, &[]);
    let counter_h = host.expose_component::<Counter>("v03::Counter", Counter::UUID, 0, &[]);
    let rel_h = host.expose_component::<ParentRel>("v03::ParentRel", ParentRel::UUID, 0, &[]);

    let root = FfiEntity::from_bits(1);
    let mid = FfiEntity::from_bits(2);
    let leaf = FfiEntity::from_bits(3);
    let orphan = FfiEntity::from_bits(4);
    let cyc_a = FfiEntity::from_bits(5);
    let cyc_b = FfiEntity::from_bits(6);

    host.relation_edges.insert((rel_h, leaf), vec![mid]);
    host.relation_edges.insert((rel_h, mid), vec![root]);
    // Default `ChildOf` cascade (`BamComponent::INVALID`) also walks `leaf -> mid -> root`
    host.relation_edges
        .insert((BamComponent::INVALID, leaf), vec![mid]);
    host.relation_edges
        .insert((BamComponent::INVALID, mid), vec![root]);
    host.relation_edges.insert((rel_h, cyc_a), vec![cyc_b]);
    host.relation_edges.insert((rel_h, cyc_b), vec![cyc_a]);
    host.set_entity_component(root, faction_h, Faction { id: 77 });

    let mut c1 = MockChunk::new(vec![root, mid]);
    c1.insert_dense(faction_h, &[Faction { id: 77 }, Faction { id: 0 }]);
    c1.insert_dense(counter_h, &[Counter { value: 10 }, Counter { value: 20 }]);
    let mut c2 = MockChunk::new(vec![leaf, orphan, cyc_a]);
    c2.insert_dense(
        faction_h,
        &[Faction { id: 0 }, Faction { id: 0 }, Faction { id: 0 }],
    );
    c2.insert_dense(
        counter_h,
        &[
            Counter { value: 30 },
            Counter { value: 40 },
            Counter { value: 50 },
        ],
    );
    host.chunks.push(c1);
    host.chunks.push(c2);

    let par_sum_raw = std::sync::Arc::new(AtomicU32::new(0));
    let par_sum_typed = std::sync::Arc::new(AtomicU32::new(0));
    let sum_raw_clone = std::sync::Arc::clone(&par_sum_raw);
    let sum_typed_clone = std::sync::Arc::clone(&par_sum_typed);

    let inst = host
        .load_fn(c"iter_v03_mod", |app| {
            // System 0: read-only query testing count, up_cascade, par_for_each_chunk_readonly
            let s0 = app
                .system(c"ro_query")
                .query_typed::<(&Faction, &Counter)>()
                .build(move |ctx| {
                    // Non-destructive count before, during, and after chunk traversal
                    assert_eq!(ctx.iter().count(), Ok(5));
                    {
                        let mut typed = ctx.single_query::<(&Faction, &Counter)>();
                        assert_eq!(typed.count(), Ok(5));
                        let (f0, c0) = typed.next_chunk().unwrap();
                        assert_eq!(f0.len(), 2);
                        assert_eq!(c0[1].value, 20);
                        // Cursor unaffected by count()!
                        assert_eq!(typed.count(), Ok(5));
                        let (f1, c1) = typed.next_chunk().unwrap();
                        assert_eq!(f1.len(), 3);
                        assert_eq!(c1[2].value, 50);
                        assert!(typed.next_chunk().is_none());
                        assert_eq!(typed.count(), Ok(5));

                        // Typed up_cascade via Rel type & pre-resolved handle
                        let hit = typed
                            .up_cascade::<ParentRel, Faction>(leaf, 0)
                            .unwrap()
                            .unwrap();
                        assert_eq!(hit.0, root);
                        assert_eq!(hit.1.id, 77);

                        let hit2 = typed
                            .up_cascade_handle::<Faction>(rel_h, leaf, 0)
                            .unwrap()
                            .unwrap();
                        assert_eq!(hit2.0, root);
                        assert_eq!(hit2.1.id, 77);

                        // Missing ancestor returns Ok(None)
                        assert_eq!(typed.up_cascade::<ParentRel, Faction>(orphan, 0), Ok(None));
                        // Ancestor cycle returns CYCLE_DETECTED
                        assert_eq!(
                            typed.up_cascade::<ParentRel, Faction>(cyc_a, 0),
                            Err(FfiResult::CYCLE_DETECTED)
                        );
                        // Wrong type at term 0 (asking for Counter instead of Faction) -> TYPE_MISMATCH
                        assert_eq!(
                            typed.up_cascade::<ParentRel, Counter>(leaf, 0),
                            Err(FfiResult::TYPE_MISMATCH)
                        );
                    }

                    // Untyped ChunkIter::up_cascade_pod and default-ChildOf (`BamComponent::INVALID`)
                    let pod_hit = ctx
                        .iter()
                        .up_cascade_pod::<Faction>(rel_h, leaf, 0)
                        .unwrap()
                        .unwrap();
                    assert_eq!(pod_hit.0, root);
                    assert_eq!(pod_hit.1.id, 77);

                    let default_child_of_hit = ctx
                        .iter()
                        .up_cascade::<Faction>(BamComponent::INVALID, leaf, 0)
                        .unwrap()
                        .unwrap();
                    assert_eq!(default_child_of_hit.0, root);
                    assert_eq!(default_child_of_hit.1.id, 77);

                    // SystemContext::par_chunks_readonly (&Chunk cannot borrow mutably)
                    let s_raw = &sum_raw_clone;
                    assert_eq!(
                        ctx.par_chunks_readonly(|chunk| {
                            let counters = chunk.component::<Counter>(1).unwrap();
                            for c in counters {
                                s_raw.fetch_add(c.value, Ordering::Relaxed);
                            }
                        }),
                        FfiResult::OK
                    );

                    // Fallible par_try_chunks_readonly propagates error
                    assert_eq!(
                        ctx.par_try_chunks_readonly(|_| FfiResult::OUT_OF_RANGE),
                        FfiResult::OUT_OF_RANGE
                    );

                    // TypedChunkIter::par_for_each_chunk_readonly
                    let s_typed = &sum_typed_clone;
                    assert_eq!(
                        ctx.single_query::<(&Faction, &Counter)>()
                            .par_for_each_chunk_readonly(|(_factions, counters)| {
                                for c in counters {
                                    s_typed.fetch_add(c.value, Ordering::Relaxed);
                                }
                            }),
                        FfiResult::OK
                    );
                    FfiResult::OK
                });
            assert_eq!(s0, FfiResult::OK);

            // System 1: panic inside par_for_each_chunk_readonly caught as PANIC_CAUGHT
            let s1 = app
                .system(c"ro_par_panic")
                .query_typed::<&Counter>()
                .build(|ctx| {
                    let res = ctx.par_chunks_readonly(|_chunk| {
                        panic!("simulated par readonly panic");
                    });
                    assert_eq!(res, FfiResult::PANIC_CAUGHT);
                    FfiResult::OK
                });
            assert_eq!(s1, FfiResult::OK);

            // System 2: writable query rejected by both TypedChunkIter and host
            app.system(c"rw_rejected")
                .query_typed::<(&mut Counter, &Faction)>()
                .build(move |ctx| {
                    // TypedChunkIter rejects `!Q::IS_READ_ONLY` before FFI
                    assert_eq!(
                        ctx.single_query::<(&mut Counter, &Faction)>()
                            .par_for_each_chunk_readonly(|_| FfiResult::OK),
                        FfiResult::PERMISSION_DENIED
                    );
                    // Host also rejects writable query via ChunkIter::par_for_each_chunk_readonly
                    assert_eq!(
                        ctx.par_chunks_readonly(|_| {}),
                        FfiResult::PERMISSION_DENIED
                    );
                    // Host rejects up_cascade targeting writable term 0
                    assert_eq!(
                        ctx.iter().up_cascade::<Counter>(rel_h, leaf, 0),
                        Err(FfiResult::PERMISSION_DENIED)
                    );
                    FfiResult::OK
                })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(par_sum_raw.load(Ordering::Relaxed), 150);
    assert_eq!(par_sum_typed.load(Ordering::Relaxed), 150);

    // Also test host convention returning OK + null owner/ptr for missing up_cascade
    host.up_cascade_missing_as_ok_null = true;
    assert_eq!(host.run_system(0), FfiResult::OK);

    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.run_system(2), FfiResult::OK);
    host.unload(inst);
}

// ============================================================================
// 7. Decoupled Scheduling (`register_handle`, `ScheduleSetBuilder`, `mount_system`)
// ============================================================================

/// Scenario: Exercises `register_handle` (slot 60), `ScheduleSetBuilder` + `configure_schedule_set`
/// (slot 61), and `mount_system` (slot 62), verifying execution-mode flags and before/after ordering.
#[test]
fn decoupled_scheduling_and_schedule_sets() {
    let mut host = V03TestHost::new();
    let _c = host.expose_component::<Counter>("v03::Counter", Counter::UUID, 0, &[]);
    let ran_count = std::sync::Arc::new(AtomicUsize::new(0));
    let ran_a = std::sync::Arc::clone(&ran_count);
    let ran_b = std::sync::Arc::clone(&ran_count);

    let inst = host
        .load_fn(c"sched_mod", |app| {
            let sys_a = app
                .system(c"physics_step")
                .query_typed::<&Counter>()
                .register_handle(move |_ctx| {
                    ran_a.fetch_add(1, Ordering::Relaxed);
                    FfiResult::OK
                })
                .unwrap();
            let sys_b = app
                .system(c"render_sync")
                .register_handle(move |_ctx| {
                    ran_b.fetch_add(10, Ordering::Relaxed);
                    FfiResult::OK
                })
                .unwrap();
            assert_eq!(sys_a, BamSystem::from_raw(0));
            assert_eq!(sys_b, BamSystem::from_raw(1));

            let set_pre = app
                .configure_schedule_set(
                    ScheduleSetBuilder::new("pre_physics")
                        .schedule(ScheduleId::FIXED_UPDATE)
                        .parallel(),
                )
                .unwrap();
            let set_post = app
                .configure_schedule_set(
                    ScheduleSetBuilder::new("post_physics")
                        .schedule(ScheduleId::FIXED_UPDATE)
                        .chain_weak(),
                )
                .unwrap();

            // Sequence overwrites parallel cleanly; weak/strong before & after flags preserved
            let set_main_builder = ScheduleSetBuilder::new("physics_core")
                .schedule(ScheduleId::FIXED_UPDATE)
                .parallel()
                .sequence()
                .after(set_pre)
                .after_weak(set_pre)
                .before(set_post)
                .before_weak(set_post);
            assert_eq!(
                set_main_builder.raw_flags(),
                BAM_SCHEDULE_SET_FLAG_SEQUENCE
                    | BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK
                    | BAM_SCHEDULE_SET_FLAG_AFTER_WEAK
            );
            let set_main = app.configure_schedule_set(set_main_builder).unwrap();

            // Mutually exclusive raw exec-mode flags rejected by `BamScheduleSetDesc::validate()`
            assert_eq!(
                app.configure_schedule_set(
                    ScheduleSetBuilder::new("invalid_exec")
                        .flags(BAM_SCHEDULE_SET_FLAG_SEQUENCE | BAM_SCHEDULE_SET_FLAG_PARALLEL)
                ),
                Err(FfiResult::INVALID_ARGUMENT)
            );
            assert_eq!(
                app.configure_schedule_set(ScheduleSetBuilder::new("")),
                Err(FfiResult::INVALID_ARGUMENT)
            );

            // Mount into set and ungrouped (`BamSystemSet::INVALID`)
            assert_eq!(
                app.mount_system(sys_a, ScheduleId::FIXED_UPDATE, set_main),
                FfiResult::OK
            );
            assert_eq!(
                app.mount_system(sys_b, ScheduleId::UPDATE, BamSystemSet::INVALID),
                FfiResult::OK
            );
            assert_eq!(
                app.mount_system(
                    BamSystem::INVALID,
                    ScheduleId::UPDATE,
                    BamSystemSet::INVALID
                ),
                FfiResult::INVALID_ARGUMENT
            );
            FfiResult::OK
        })
        .unwrap();

    assert_eq!(host.schedule_sets.len(), 3);
    assert_eq!(host.schedule_sets[0].flags, BAM_SCHEDULE_SET_FLAG_PARALLEL);
    assert_eq!(
        host.schedule_sets[1].flags,
        BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK
    );
    assert_eq!(
        host.schedule_sets[2].before_sets,
        vec![BamSystemSet::from_raw(1), BamSystemSet::from_raw(1)]
    );
    assert_eq!(
        host.schedule_sets[2].after_sets,
        vec![BamSystemSet::from_raw(0), BamSystemSet::from_raw(0)]
    );
    assert_eq!(
        host.mounted_systems,
        vec![
            (
                BamSystem::from_raw(0),
                ScheduleId::FIXED_UPDATE,
                BamSystemSet::from_raw(2)
            ),
            (
                BamSystem::from_raw(1),
                ScheduleId::UPDATE,
                BamSystemSet::INVALID
            ),
        ]
    );

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(ran_count.load(Ordering::Relaxed), 11);
    host.unload(inst);
}

// ============================================================================
// 8. Command Transactions (`GuestTransaction<'a>` & `GuestCommands::transaction`)
// ============================================================================

/// Scenario: Exercises `GuestCommands::transaction` (`cmd_checkpoint` slot 63 / `cmd_rollback`
/// slot 64) across explicit commit, explicit rollback, RAII drop auto-rollback, and nested
/// transactions.
#[test]
fn command_transactions_commit_rollback_drop_and_nested() {
    let host = V03TestHost::new();
    let mut rel_h = BamComponent::INVALID;

    let inst = host
        .load_fn(c"tx_mod", |app| {
            rel_h = app
                .register_relation(RelationDescBuilder::new("tx_edge").acyclic())
                .unwrap();
            let rel = rel_h;

            // System 0: commit, rollback-on-Err, rollback-on-Drop, explicit rollback, nested tx
            let s0 = app.system(c"tx_lifecycle").build(move |ctx| {
                let cmds = ctx.commands();
                // Pre-transaction command (index 0)
                let base = cmds.spawn_empty().unwrap();

                // 1. Committed transaction via closure
                let committed = cmds
                    .transaction(|tx| -> Result<FfiEntity, FfiResult> {
                        let e = tx.spawn_empty()?;
                        let st = tx.insert_relation(base, rel, e);
                        if !st.is_ok() {
                            return Err(st);
                        }
                        Ok(e)
                    })
                    .unwrap();

                // 2. Rolled-back transaction via Err(...)
                let err_res: Result<(), FfiResult> = cmds.transaction(|tx| {
                    let temp = tx.spawn_empty()?;
                    let _ = tx.despawn(temp);
                    Err(FfiResult::CYCLE_DETECTED)
                });
                assert_eq!(err_res, Err(FfiResult::CYCLE_DETECTED));

                // 3. Uncommitted Drop rolls back automatically
                {
                    let mut tx = cmds.begin_transaction().unwrap();
                    let _ = tx.spawn_empty().unwrap();
                    let _ = tx.remove_relation(base, rel, committed);
                    // dropped without commit()
                }

                // 4. Explicit rollback does NOT double-rollback on drop
                {
                    let mut tx = cmds.begin_transaction().unwrap();
                    let _ = tx.spawn_empty().unwrap();
                    assert_eq!(tx.rollback(), FfiResult::OK);
                }

                // 5. Explicit commit keeps commands and does NOT roll back on drop
                {
                    let mut tx = cmds.begin_transaction().unwrap();
                    let _ = tx.despawn(committed);
                    tx.commit();
                }

                // 6. Nested transactions: outer commits while inner rolls back
                cmds.transaction(|outer| -> Result<(), FfiResult> {
                    let outer_e = outer.spawn_empty()?;
                    let inner_res: Result<(), FfiResult> = outer.transaction(|inner| {
                        let _ = inner.spawn_empty()?;
                        Err(FfiResult::PERMISSION_DENIED)
                    });
                    assert_eq!(inner_res, Err(FfiResult::PERMISSION_DENIED));
                    let _ = outer.despawn(outer_e);
                    Ok(())
                })
                .unwrap();

                FfiResult::OK
            });
            assert_eq!(s0, FfiResult::OK);

            // System 1: panic inside transaction rolls back on unwind before PANIC_CAUGHT
            app.system(c"tx_panic_rollback").build(|ctx| {
                let cmds = ctx.commands();
                let _ = cmds.transaction(|tx| -> Result<(), FfiResult> {
                    let _ = tx.spawn_empty()?;
                    let _ = tx.spawn_empty()?;
                    panic!("abort inside transaction");
                });
                FfiResult::OK
            })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    // Expected surviving commands:
    // 0: base SpawnEmpty
    // 1: committed SpawnEmpty
    // 2: InsertRelation(base -> committed)
    // 3: Despawn(committed)
    // 4: outer_e SpawnEmpty
    // 5: Despawn(outer_e)
    assert_eq!(host.commands.len(), 6);
    assert!(matches!(host.commands[0], RecordedCmd::SpawnEmpty(_)));
    assert!(matches!(host.commands[1], RecordedCmd::SpawnEmpty(_)));
    assert!(matches!(
        host.commands[2],
        RecordedCmd::InsertRelation { .. }
    ));
    assert!(matches!(host.commands[3], RecordedCmd::Despawn(_)));
    assert!(matches!(host.commands[4], RecordedCmd::SpawnEmpty(_)));
    assert!(matches!(host.commands[5], RecordedCmd::Despawn(_)));
    // Exactly 4 rollbacks: Err tx, dropped tx, explicit rollback (no double rollback), nested inner tx
    assert_eq!(host.rollback_calls, 4);

    // Run panicking transaction: rollback runs during guest unwind, caught as PANIC_CAUGHT!
    assert_eq!(host.run_system(1), FfiResult::PANIC_CAUGHT);
    assert_eq!(host.commands.len(), 6);
    assert_eq!(host.rollback_calls, 5);
    host.unload(inst);
}

// ============================================================================
// 9. Live Component Column Migration (`migrate_component_column`)
// ============================================================================

/// Scenario: Exercises `migrate_component_column` (slot 67) across successful `OldStats -> NewStats`
/// migration, callback error abort, panic containment (`PANIC_CAUGHT`), and ZST `OldTag -> NewTag`
/// migration.
#[test]
fn component_column_migration_success_error_panic_and_zst() {
    let mut host = V03TestHost::new();
    let stats_h = host.expose_component::<OldStats>(
        OldStats::CANONICAL_NAME,
        OldStats::UUID,
        OldStats::SCHEMA_FINGERPRINT,
        OldStats::BAM_FIELDS,
    );
    let tag_h = host.expose_component::<OldTag>(
        OldTag::CANONICAL_NAME,
        OldTag::UUID,
        OldTag::SCHEMA_FINGERPRINT,
        &[],
    );
    let mut chunk = MockChunk::new(vec![
        FfiEntity::from_bits(1),
        FfiEntity::from_bits(2),
        FfiEntity::from_bits(3),
    ]);
    chunk.insert_dense(
        stats_h,
        &[
            OldStats { hp: 10 },
            OldStats { hp: 25 },
            OldStats { hp: 40 },
        ],
    );
    chunk.insert_dense(tag_h, &[OldTag, OldTag, OldTag]);
    host.chunks.push(chunk);

    let inst = host
        .load_fn(c"migrate_mod", |app| {
            // 1. Mismatched old fingerprint rejected before mutating rows
            assert_eq!(
                app.migrate_component_column::<BadFingerprintStats, NewStats, _>(stats_h, |o| {
                    Ok(NewStats {
                        hp: o.hp as f64,
                        armor: 0,
                        pad: 0,
                    })
                }),
                FfiResult::TYPE_MISMATCH
            );

            // 2. Closure returning Err aborts migration without mutating rows
            assert_eq!(
                app.migrate_component_column::<OldStats, NewStats, _>(stats_h, |o| {
                    if o.hp == 25 {
                        return Err(FfiResult::INVALID_ARGUMENT);
                    }
                    Ok(NewStats {
                        hp: o.hp as f64,
                        armor: 1,
                        pad: 0,
                    })
                }),
                FfiResult::INVALID_ARGUMENT
            );

            // 3. Panic inside migration closure is caught cleanly as PANIC_CAUGHT
            assert_eq!(
                app.migrate_component_column::<OldStats, NewStats, _>(stats_h, |o| {
                    if o.hp == 25 {
                        panic!("row migration panic");
                    }
                    Ok(NewStats {
                        hp: o.hp as f64,
                        armor: 1,
                        pad: 0,
                    })
                }),
                FfiResult::PANIC_CAUGHT
            );

            // 4. Successful migration OldStats (4B, align 4) -> NewStats (16B, align 8)
            assert_eq!(
                app.migrate_component_column::<OldStats, NewStats, _>(stats_h, |old| {
                    Ok(NewStats {
                        hp: (old.hp as f64) * 2.5,
                        armor: 50,
                        pad: 0,
                    })
                }),
                FfiResult::OK
            );

            // Verify updated host layout & schema fingerprint
            let new_info = app
                .verify_component_layout_handle::<NewStats>(stats_h)
                .unwrap();
            assert_eq!(new_info.schema_fingerprint, NewStats::SCHEMA_FINGERPRINT);
            assert_eq!(new_info.size, core::mem::size_of::<NewStats>());
            assert_eq!(new_info.field_count, 2);

            // 5. ZST -> ZST migration under Miri
            assert_eq!(
                app.migrate_component_column::<OldTag, NewTag, _>(tag_h, |_| Ok(NewTag)),
                FfiResult::OK
            );
            // 6. ZST -> non-ZST POD migration via `migrate_pod_component_column`
            assert_eq!(
                app.migrate_pod_component_column::<NewTag, Counter, _>(
                    tag_h,
                    NewTag::SCHEMA_FINGERPRINT,
                    0x9999,
                    |_| Ok(Counter { value: 123 })
                ),
                FfiResult::OK
            );
            FfiResult::OK
        })
        .unwrap();

    let migrated_stats = host.chunks[0].read_dense::<NewStats>(stats_h);
    assert_eq!(
        migrated_stats,
        vec![
            NewStats {
                hp: 25.0,
                armor: 50,
                pad: 0
            },
            NewStats {
                hp: 62.5,
                armor: 50,
                pad: 0
            },
            NewStats {
                hp: 100.0,
                armor: 50,
                pad: 0
            },
        ]
    );
    let migrated_counters = host.chunks[0].read_dense::<Counter>(tag_h);
    assert_eq!(
        migrated_counters,
        vec![
            Counter { value: 123 },
            Counter { value: 123 },
            Counter { value: 123 },
        ]
    );
    host.unload(inst);
}

// ============================================================================
// 10. 16-Term `QueryTuple` & 2..=8 Branch `Or<(T0..T7)>` + `Or2`/`Or3`
// ============================================================================

/// Scenario: Verifies compile-time and descriptor-generation support for maximum-arity 16-term
/// `QueryTuple`s and 8-branch `Or<(...)>` disjunctive filter groups.
#[test]
fn sixteen_term_query_tuple_and_eight_branch_or() {
    /// Compile-time helper asserting that `Q` satisfies [`QueryTuple`].
    fn assert_tuple<Q: QueryTuple>() {}

    // Compile-time coverage for every QueryTuple arity 1..=16 and every Or arity 2..=8
    assert_tuple::<(&C0,)>();
    assert_tuple::<(&C0, &C1)>();
    assert_tuple::<(&C0, &C1, &C2)>();
    assert_tuple::<(&C0, &C1, &C2, &C3)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6, &C7)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6, &C7, &C8)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6, &C7, &C8, &C9)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6, &C7, &C8, &C9, &C10)>();
    assert_tuple::<(&C0, &C1, &C2, &C3, &C4, &C5, &C6, &C7, &C8, &C9, &C10, &C11)>();
    assert_tuple::<(
        &C0,
        &C1,
        &C2,
        &C3,
        &C4,
        &C5,
        &C6,
        &C7,
        &C8,
        &C9,
        &C10,
        &C11,
        &C12,
    )>();
    assert_tuple::<(
        &C0,
        &C1,
        &C2,
        &C3,
        &C4,
        &C5,
        &C6,
        &C7,
        &C8,
        &C9,
        &C10,
        &C11,
        &C12,
        &C13,
    )>();
    assert_tuple::<(
        &C0,
        &C1,
        &C2,
        &C3,
        &C4,
        &C5,
        &C6,
        &C7,
        &C8,
        &C9,
        &C10,
        &C11,
        &C12,
        &C13,
        &C14,
    )>();
    assert_tuple::<(
        &C0,
        &mut C1,
        &C2,
        &C3,
        &C4,
        &C5,
        &C6,
        &C7,
        &C8,
        &C9,
        &C10,
        &C11,
        &C12,
        &C13,
        &C14,
        &C15,
    )>();

    assert_tuple::<Or2<With<C0>, Added<C1>>>();
    assert_tuple::<Or3<With<C0>, Without<C1>, Changed<C2>>>();
    assert_tuple::<Or<(With<C0>, Without<C1>)>>();
    assert_tuple::<Or<(With<C0>, Without<C1>, Changed<C2>)>>();
    assert_tuple::<Or<(With<C0>, Without<C1>, Changed<C2>, Added<C3>)>>();
    assert_tuple::<Or<(With<C0>, Without<C1>, Changed<C2>, Added<C3>, With<C4>)>>();
    assert_tuple::<
        Or<(
            With<C0>,
            Without<C1>,
            Changed<C2>,
            Added<C3>,
            With<C4>,
            Without<C5>,
        )>,
    >();
    assert_tuple::<
        Or<(
            With<C0>,
            Without<C1>,
            Changed<C2>,
            Added<C3>,
            With<C4>,
            Without<C5>,
            Changed<C6>,
        )>,
    >();
    assert_tuple::<
        Or<(
            With<C0>,
            Without<C1>,
            Changed<C2>,
            Added<C3>,
            With<C4>,
            Without<C5>,
            Changed<C6>,
            Added<C7>,
        )>,
    >();

    // Live runtime execution of a 16-term QueryTuple + 8-branch Or + 2nd Or group
    let mut host = V03TestHost::new();
    let handles = [
        host.expose_component::<C0>("q16::C0", C0::UUID, 0, &[]),
        host.expose_component::<C1>("q16::C1", C1::UUID, 0, &[]),
        host.expose_component::<C2>("q16::C2", C2::UUID, 0, &[]),
        host.expose_component::<C3>("q16::C3", C3::UUID, 0, &[]),
        host.expose_component::<C4>("q16::C4", C4::UUID, 0, &[]),
        host.expose_component::<C5>("q16::C5", C5::UUID, 0, &[]),
        host.expose_component::<C6>("q16::C6", C6::UUID, 0, &[]),
        host.expose_component::<C7>("q16::C7", C7::UUID, 0, &[]),
        host.expose_component::<C8>("q16::C8", C8::UUID, 0, &[]),
        host.expose_component::<C9>("q16::C9", C9::UUID, 0, &[]),
        host.expose_component::<C10>("q16::C10", C10::UUID, 0, &[]),
        host.expose_component::<C11>("q16::C11", C11::UUID, 0, &[]),
        host.expose_component::<C12>("q16::C12", C12::UUID, 0, &[]),
        host.expose_component::<C13>("q16::C13", C13::UUID, 0, &[]),
        host.expose_component::<C14>("q16::C14", C14::UUID, 0, &[]),
        host.expose_component::<C15>("q16::C15", C15::UUID, 0, &[]),
    ];

    let mut chunk = MockChunk::new(vec![FfiEntity::from_bits(1), FfiEntity::from_bits(2)]);
    chunk.insert_dense(handles[0], &[C0 { v: 1 }, C0 { v: 10 }]);
    chunk.insert_dense(handles[1], &[C1 { v: 0 }, C1 { v: 0 }]);
    chunk.insert_dense(handles[2], &[C2 { v: 2 }, C2 { v: 20 }]);
    chunk.insert_dense(handles[3], &[C3 { v: 3 }, C3 { v: 30 }]);
    chunk.insert_dense(handles[4], &[C4 { v: 4 }, C4 { v: 40 }]);
    chunk.insert_dense(handles[5], &[C5 { v: 5 }, C5 { v: 50 }]);
    chunk.insert_dense(handles[6], &[C6 { v: 6 }, C6 { v: 60 }]);
    chunk.insert_dense(handles[7], &[C7 { v: 7 }, C7 { v: 70 }]);
    chunk.insert_dense(handles[8], &[C8 { v: 8 }, C8 { v: 80 }]);
    chunk.insert_dense(handles[9], &[C9 { v: 9 }, C9 { v: 90 }]);
    chunk.insert_dense(handles[10], &[C10 { v: 10 }, C10 { v: 100 }]);
    chunk.insert_dense(handles[11], &[C11 { v: 11 }, C11 { v: 110 }]);
    chunk.insert_dense(handles[12], &[C12 { v: 12 }, C12 { v: 120 }]);
    chunk.insert_dense(handles[13], &[C13 { v: 13 }, C13 { v: 130 }]);
    chunk.insert_dense(handles[14], &[C14 { v: 14 }, C14 { v: 140 }]);
    chunk.insert_dense(handles[15], &[C15 { v: 15 }, C15 { v: 150 }]);
    host.chunks.push(chunk);

    type Q16<'q> = (
        &'q C0,
        &'q mut C1,
        &'q C2,
        &'q C3,
        &'q C4,
        &'q C5,
        &'q C6,
        &'q C7,
        &'q C8,
        &'q C9,
        &'q C10,
        &'q C11,
        &'q C12,
        &'q C13,
        &'q C14,
        &'q C15,
    );
    type Or8AndOr2<'q> = (
        &'q C0,
        Or<(
            With<C1>,
            Without<C2>,
            Changed<C3>,
            Added<C4>,
            With<C5>,
            Without<C6>,
            Changed<C7>,
            Added<C8>,
        )>,
        Or2<With<C9>, Added<C10>>,
        Or3<With<C11>, Without<C12>, Changed<C13>>,
    );

    let inst = host
        .load_fn(c"q16_mod", |app| {
            let s0 = app
                .system(c"sum_16_terms")
                .query_typed::<Q16<'_>>()
                .build(|ctx| {
                    let mut typed = ctx.single_query::<Q16<'_>>();
                    typed.for_each(
                        |(c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15)| {
                            for i in 0..c0.len() {
                                c1[i].v = c0[i].v
                                    + c2[i].v
                                    + c3[i].v
                                    + c4[i].v
                                    + c5[i].v
                                    + c6[i].v
                                    + c7[i].v
                                    + c8[i].v
                                    + c9[i].v
                                    + c10[i].v
                                    + c11[i].v
                                    + c12[i].v
                                    + c13[i].v
                                    + c14[i].v
                                    + c15[i].v;
                            }
                        },
                    )
                });
            assert_eq!(s0, FfiResult::OK);

            app.system(c"or_groups_8_2_3")
                .query_typed::<Or8AndOr2<'_>>()
                .build(|ctx| {
                    let mut typed = ctx.single_query::<Or8AndOr2<'_>>();
                    let mut seen = 0usize;
                    let status = typed.for_each(|(c0, (), (), ())| {
                        seen += c0.len();
                    });
                    assert_eq!(status, FfiResult::OK);
                    assert_eq!(seen, 2);
                    FfiResult::OK
                })
        })
        .unwrap();

    // Verify registered descriptor counts and distinct `or_group` IDs (0, 1*8, 2*2, 3*3)
    assert_eq!(host.systems[0].queries[0].len(), 16);
    let or_terms = &host.systems[1].queries[0];
    assert_eq!(or_terms.len(), 1 + 8 + 2 + 3);
    assert_eq!(or_terms[0].or_group, 0);
    for t in &or_terms[1..9] {
        assert_eq!(t.or_group, 1);
    }
    for t in &or_terms[9..11] {
        assert_eq!(t.or_group, 2);
    }
    for t in &or_terms[11..14] {
        assert_eq!(t.or_group, 3);
    }

    assert_eq!(host.run_system(0), FfiResult::OK);
    // Row 0: 1 + 2 + 3 + ... + 15 = 120; Row 1: 10 + 20 + ... + 150 = 1200
    assert_eq!(
        host.chunks[0].read_dense::<C1>(handles[1]),
        vec![C1 { v: 120 }, C1 { v: 1200 }]
    );
    assert_eq!(host.run_system(1), FfiResult::OK);
    host.unload(inst);
}

/// Scenario: Verifies that `SystemContext` avoids `sys_host_ctx` and `sys_commands` FFI calls on
/// `noop` and query-only systems (including `queries_3_mut`), while lazily calling each at most
/// once per tick when `ctx.commands()` or `ctx.intern_symbol()` / `ctx.host()` are invoked.
#[test]
fn system_context_defers_sys_host_ctx_and_sys_commands_on_noop_and_queries() {
    define_pod_component!(struct CompA { v: u32 } => c"lazy_test::A");
    define_pod_component!(struct CompB { v: u32 } => c"lazy_test::B");
    define_pod_component!(struct CompC { v: u32 } => c"lazy_test::C");

    let mut host = V03TestHost::new();
    let ha = host.expose_component::<CompA>("lazy_test::A", CompA::UUID, 0, &[]);
    let hb = host.expose_component::<CompB>("lazy_test::B", CompB::UUID, 0, &[]);
    let hc = host.expose_component::<CompC>("lazy_test::C", CompC::UUID, 0, &[]);
    let mut chunk = MockChunk::new(vec![FfiEntity::from_bits(1)]);
    chunk.insert_dense(ha, &[CompA { v: 1 }]);
    chunk.insert_dense(hb, &[CompB { v: 2 }]);
    chunk.insert_dense(hc, &[CompC { v: 3 }]);
    host.chunks.push(chunk);

    let inst = host
        .load_fn(c"lazy_sdk_mod", |app| {
            // System 0: pure noop.
            let rc = app.system(c"noop_sys").build(|_ctx| FfiResult::OK);
            assert_eq!(rc, FfiResult::OK);

            // System 1: 3-query system calling `queries_3_mut` (must NOT trigger `sys_commands` or `sys_host_ctx`).
            let rc = app
                .system(c"three_query_sys")
                .query(|q| q.read::<CompA>())
                .query(|q| q.read::<CompB>())
                .query(|q| q.read::<CompC>())
                .build(|ctx| {
                    let (q0, q1, q2) = ctx.queries_3_mut(0, 1, 2).expect("3 queries");
                    assert!(q0.next_chunk().is_some());
                    assert!(q1.next_chunk().is_some());
                    assert!(q2.next_chunk().is_some());
                    FfiResult::OK
                });
            assert_eq!(rc, FfiResult::OK);

            // System 2: invokes `commands()` twice and `intern_symbol()` twice (1 FFI call each).
            app.system(c"cmds_and_host_sys").build(|ctx| {
                let _ = ctx.commands().spawn_empty().unwrap();
                let _ = ctx.commands().spawn_empty().unwrap();
                let s1 = ctx.intern_symbol("lazy.sym").unwrap();
                let s2 = ctx.intern_symbol("lazy.sym").unwrap();
                assert_eq!(s1, s2);
                FfiResult::OK
            })
        })
        .unwrap();

    // System 0 (noop): 0 sys_host_ctx, 0 sys_commands.
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.sys_host_ctx_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.sys_commands_calls.load(Ordering::Relaxed), 0);

    // System 1 (queries_3_mut): still 0 sys_host_ctx, 0 sys_commands.
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.sys_host_ctx_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.sys_commands_calls.load(Ordering::Relaxed), 0);

    // System 2 (commands + intern_symbol twice each): exactly 1 sys_host_ctx, 1 sys_commands.
    assert_eq!(host.run_system(2), FfiResult::OK);
    assert_eq!(host.sys_host_ctx_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.sys_commands_calls.load(Ordering::Relaxed), 1);

    host.unload(inst);
}

// ============================================================================
// Wave 3 Parity Tests: P1.4, P1.5, P1.7, P2.8
// ============================================================================

/// Scenario (P1.4): Verifies `ChangedMut<T>`, `AddedMut<T>`, `WithChanged<T>`, `WithAdded<T>`
/// emit the exact `AccessMode` + `BAM_TERM_FLAG_CHANGED`/`BAM_TERM_FLAG_ADDED` descriptors,
/// verifies `Ref<T>`/`RefSlice<T>` and `Mut<T>`/`MutSlice<T>` per-row tick inspection via slot 69
/// (`iter_column_ticks`) alongside `Chunk::column_ticks`/`ChunkIter::column_ticks`, and verifies
/// graceful fallback when slot 69 is absent on older hosts.
#[test]
fn changed_mut_added_mut_with_changed_with_added_and_ref_mut_slices() {
    define_pod_component!(struct TickPos { x: f32, y: f32 } => c"wave3::TickPos");
    define_pod_component!(struct TickTag { v: u32 } => c"wave3::TickTag");

    let mut host = V03TestHost::new();
    let h_pos = host.expose_component::<TickPos>("wave3::TickPos", TickPos::UUID, 0, &[]);
    let h_tag = host.expose_component::<TickTag>("wave3::TickTag", TickTag::UUID, 0, &[]);

    let mut chunk = MockChunk::new(vec![
        FfiEntity::from_bits(1),
        FfiEntity::from_bits(2),
        FfiEntity::from_bits(3),
    ]);
    chunk.insert_dense(
        h_pos,
        &[
            TickPos { x: 1.0, y: 0.0 },
            TickPos { x: 2.0, y: 0.0 },
            TickPos { x: 3.0, y: 0.0 },
        ],
    );
    chunk.insert_dense(
        h_tag,
        &[TickTag { v: 10 }, TickTag { v: 20 }, TickTag { v: 30 }],
    );
    // Window is last_run = 5, this_run = 10:
    // row 0: added=2, changed=3 -> neither added nor changed
    // row 1: added=2, changed=7 -> changed only
    // row 2: added=8, changed=8 -> both added and changed
    chunk.insert_column_ticks(h_pos, &[2, 2, 8], &[3, 7, 8]);
    host.chunks.push(chunk);

    let inst = host
        .load_fn(c"wave3_ticks_mod", |app| {
            // System 0: ChangedMut + WithAdded descriptor check & mutable slice write.
            let rc = app
                .system(c"changed_mut_sys")
                .query_typed::<(ChangedMut<TickPos>, WithAdded<TickTag>)>()
                .build(|ctx| {
                    ctx.single_query::<(ChangedMut<TickPos>, WithAdded<TickTag>)>()
                        .for_each(|(pos, ())| {
                            for p in pos {
                                p.x += 10.0;
                            }
                        })
                });
            assert_eq!(rc, FfiResult::OK);

            // System 1: AddedMut + WithChanged descriptor check.
            let rc = app
                .system(c"added_mut_sys")
                .query_typed::<(AddedMut<TickPos>, WithChanged<TickTag>)>()
                .build(|ctx| {
                    ctx.single_query::<(AddedMut<TickPos>, WithChanged<TickTag>)>()
                        .for_each(|(pos, ())| {
                            for p in pos {
                                p.y += 1.0;
                            }
                        })
                });
            assert_eq!(rc, FfiResult::OK);

            // System 2: Ref<TickPos> per-row tick inspection + Chunk/ChunkIter column_ticks.
            let rc = app
                .system(c"ref_slice_sys")
                .query_typed::<Ref<TickPos>>()
                .build(|ctx| {
                    ctx.single_query::<Ref<TickPos>>().for_each(|r| {
                        assert!(r.has_ticks());
                        assert_eq!(r.len(), 3);
                        assert!(!r.is_added(0));
                        assert!(!r.is_changed(0));
                        assert!(!r.is_added(1));
                        assert!(r.is_changed(1));
                        assert!(r.is_added(2));
                        assert!(r.is_changed(2));
                        assert_eq!(r.row_ticks(1).unwrap().changed, 7);
                        assert!(!r.is_changed(99));
                    })
                });
            assert_eq!(rc, FfiResult::OK);

            // System 3: Mut<TickPos> per-row tick inspection + mutation + Chunk::column_ticks.
            app.system(c"mut_slice_sys")
                .query_typed::<Mut<TickPos>>()
                .build(|ctx| {
                    let iter = ctx.iter();
                    let chunk = iter.next_chunk_view().expect("chunk");
                    let (added, changed, last_run, this_run) =
                        chunk.column_ticks(0).expect("chunk.column_ticks");
                    assert_eq!(added, &[2, 2, 8]);
                    assert_eq!(changed, &[3, 7, 8]);
                    assert_eq!((last_run, this_run), (5, 10));
                    assert_eq!(iter.reset(), FfiResult::OK);
                    iter.typed::<Mut<TickPos>>().for_each(|mut m| {
                        assert!(m.has_ticks());
                        assert!(m.is_changed(1));
                        assert!(m.is_added(2));
                        m[1].y = 42.0;
                    })
                })
        })
        .unwrap();

    // Verify emitted BamTermDesc flags for ChangedMut / WithAdded / AddedMut / WithChanged.
    let s0_terms = &host.systems[0].queries[0];
    assert_eq!(s0_terms[0].mode, AccessMode::WRITE);
    assert!(s0_terms[0].is_changed());
    assert_eq!(s0_terms[1].mode, AccessMode::FILTER_ONLY);
    assert!(s0_terms[1].is_added());

    let s1_terms = &host.systems[1].queries[0];
    assert_eq!(s1_terms[0].mode, AccessMode::WRITE);
    assert!(s1_terms[0].is_added());
    assert_eq!(s1_terms[1].mode, AccessMode::FILTER_ONLY);
    assert!(s1_terms[1].is_changed());

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.run_system(2), FfiResult::OK);
    assert_eq!(host.run_system(3), FfiResult::OK);

    let final_pos = host.chunks[0].read_dense::<TickPos>(h_pos);
    assert_eq!(final_pos[0], TickPos { x: 11.0, y: 1.0 });
    assert_eq!(final_pos[1], TickPos { x: 12.0, y: 42.0 });
    assert_eq!(final_pos[2], TickPos { x: 13.0, y: 1.0 });

    host.unload(inst);

    // Graceful fallback when slot 69 (`iter_column_ticks`) is absent on an older host.
    let mut old_api = make_v03_host_api();
    old_api.iter_column_ticks = None;
    let mut old_host = V03TestHost::with_api(old_api);
    let h_pos_old = old_host.expose_component::<TickPos>("wave3::TickPos", TickPos::UUID, 0, &[]);
    let mut c_old = MockChunk::new(vec![FfiEntity::from_bits(1)]);
    c_old.insert_dense(h_pos_old, &[TickPos { x: 5.0, y: 6.0 }]);
    old_host.chunks.push(c_old);
    let inst_old = old_host
        .load_fn(c"wave3_old_ticks_mod", |app| {
            app.system(c"ref_fallback_sys")
                .query_typed::<Ref<TickPos>>()
                .build(|ctx| {
                    ctx.single_query::<Ref<TickPos>>().for_each(|r| {
                        assert!(!r.has_ticks());
                        assert!(!r.is_added(0));
                        assert!(!r.is_changed(0));
                        assert!(r.row_ticks(0).is_none());
                        assert_eq!(r[0], TickPos { x: 5.0, y: 6.0 });
                    })
                })
        })
        .unwrap();
    assert_eq!(old_host.run_system(0), FfiResult::OK);
    old_host.unload(inst_old);
}

/// Scenario (P1.5): Verifies `SystemBuilder::run_if_with`, `SystemBuilder::run_if_condition`,
/// `RunConditionSystemContext`, and `conditions::{resource_exists, resource_changed, on_message,
/// in_state}` end-to-end, plus rejection of mutable terms in `run_if_with`.
#[test]
fn ecs_aware_run_if_conditions_resource_message_and_state() {
    define_pod_component!(struct GameCfg { speed: u32 } => c"wave3::GameCfg");
    define_pod_component!(struct GameState { mode: u32 } => c"wave3::GameState");
    define_pod_component!(struct JumpMsg { power: u32 } => c"wave3::JumpMsg");

    static RES_EXISTS_RUNS: AtomicU32 = AtomicU32::new(0);
    static RES_CHANGED_RUNS: AtomicU32 = AtomicU32::new(0);
    static MSG_RUNS: AtomicU32 = AtomicU32::new(0);
    static STATE_RUNS: AtomicU32 = AtomicU32::new(0);
    static CUSTOM_WITH_RUNS: AtomicU32 = AtomicU32::new(0);
    RES_EXISTS_RUNS.store(0, Ordering::Relaxed);
    RES_CHANGED_RUNS.store(0, Ordering::Relaxed);
    MSG_RUNS.store(0, Ordering::Relaxed);
    STATE_RUNS.store(0, Ordering::Relaxed);
    CUSTOM_WITH_RUNS.store(0, Ordering::Relaxed);

    let mut host = V03TestHost::new();
    let h_cfg = host.expose_component::<GameCfg>("wave3::GameCfg", GameCfg::UUID, 0, &[]);
    let h_state = host.expose_component::<GameState>("wave3::GameState", GameState::UUID, 0, &[]);
    let h_msg = host.expose_component::<JumpMsg>("wave3::JumpMsg", JumpMsg::UUID, 0, &[]);

    let inst = host
        .load_fn(c"wave3_cond_mod", |app| {
            // System 0: resource_exists::<GameCfg>()
            let rc = app
                .system(c"cond_res_exists")
                .run_if_condition(conditions::resource_exists::<GameCfg>())
                .build(|_ctx| {
                    RES_EXISTS_RUNS.fetch_add(1, Ordering::Relaxed);
                });
            assert_eq!(rc, FfiResult::OK);

            // System 1: resource_changed::<GameCfg>()
            let rc = app
                .system(c"cond_res_changed")
                .run_if_condition(conditions::resource_changed::<GameCfg>())
                .build(|_ctx| {
                    RES_CHANGED_RUNS.fetch_add(1, Ordering::Relaxed);
                });
            assert_eq!(rc, FfiResult::OK);

            // System 2: on_message::<JumpMsg>()
            let rc = app
                .system(c"cond_on_msg")
                .run_if_condition(conditions::on_message::<JumpMsg>())
                .build(|_ctx| {
                    MSG_RUNS.fetch_add(1, Ordering::Relaxed);
                });
            assert_eq!(rc, FfiResult::OK);

            // System 3: in_state::<GameState>(GameState { mode: 2 })
            let rc = app
                .system(c"cond_in_state")
                .run_if_condition(conditions::in_state(GameState { mode: 2 }))
                .build(|_ctx| {
                    STATE_RUNS.fetch_add(1, Ordering::Relaxed);
                });
            assert_eq!(rc, FfiResult::OK);

            // System 4: custom run_if_with reading GameCfg resource directly
            let rc = app
                .system(c"cond_custom_with")
                .run_if_with(
                    |q| q.resource_read::<GameCfg>(),
                    |ctx| ctx.res::<GameCfg>(0).is_some_and(|c| c.speed >= 5),
                )
                .build(|_ctx| {
                    CUSTOM_WITH_RUNS.fetch_add(1, Ordering::Relaxed);
                });
            assert_eq!(rc, FfiResult::OK);

            // Mutable terms in run_if_with must be rejected with PERMISSION_DENIED.
            let bad = app
                .system(c"cond_mutable_rejected")
                .run_if_with(|q| q.resource_write::<GameCfg>(), |_ctx| true)
                .build(|_ctx| {});
            assert_eq!(bad, FfiResult::PERMISSION_DENIED);

            FfiResult::OK
        })
        .unwrap();

    // Initially no resources or messages exist -> all 5 systems skip.
    for idx in 0..5 {
        assert_eq!(host.run_system(idx), FfiResult::OK);
    }
    assert_eq!(RES_EXISTS_RUNS.load(Ordering::Relaxed), 0);
    assert_eq!(RES_CHANGED_RUNS.load(Ordering::Relaxed), 0);
    assert_eq!(MSG_RUNS.load(Ordering::Relaxed), 0);
    assert_eq!(STATE_RUNS.load(Ordering::Relaxed), 0);
    assert_eq!(CUSTOM_WITH_RUNS.load(Ordering::Relaxed), 0);

    // Insert unchanged GameCfg { speed: 3 } (changed tick 2 <= last_run 5) -> exists runs, changed & custom skip.
    let stale_ticks = BamChangeTicks {
        added: 1,
        changed: 2,
        last_run: 5,
        this_run: 10,
    };
    host.set_resource(h_cfg, GameCfg { speed: 3 }, stale_ticks);
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.run_system(4), FfiResult::OK);
    assert_eq!(RES_EXISTS_RUNS.load(Ordering::Relaxed), 1);
    assert_eq!(RES_CHANGED_RUNS.load(Ordering::Relaxed), 0);
    assert_eq!(CUSTOM_WITH_RUNS.load(Ordering::Relaxed), 0);

    // Update GameCfg { speed: 10 } with fresh changed tick 8 -> changed & custom both run!
    let fresh_ticks = BamChangeTicks {
        added: 1,
        changed: 8,
        last_run: 5,
        this_run: 10,
    };
    host.set_resource(h_cfg, GameCfg { speed: 10 }, fresh_ticks);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.run_system(4), FfiResult::OK);
    assert_eq!(RES_CHANGED_RUNS.load(Ordering::Relaxed), 1);
    assert_eq!(CUSTOM_WITH_RUNS.load(Ordering::Relaxed), 1);

    // Push a JumpMsg -> on_message runs once, then skips on next tick (queue drained).
    host.push_message(h_msg, JumpMsg { power: 99 });
    assert_eq!(host.run_system(2), FfiResult::OK);
    assert_eq!(host.run_system(2), FfiResult::OK);
    assert_eq!(MSG_RUNS.load(Ordering::Relaxed), 1);

    // Set GameState mode=1 (skips), then mode=2 (runs).
    host.set_resource(h_state, GameState { mode: 1 }, fresh_ticks);
    assert_eq!(host.run_system(3), FfiResult::OK);
    assert_eq!(STATE_RUNS.load(Ordering::Relaxed), 0);
    host.set_resource(h_state, GameState { mode: 2 }, fresh_ticks);
    assert_eq!(host.run_system(3), FfiResult::OK);
    assert_eq!(STATE_RUNS.load(Ordering::Relaxed), 1);

    host.unload(inst);
}

/// Scenario (P1.7): Verifies `SystemContext::get_many_mut`, `TypedChunkIter::for_each_combination`
/// (within-chunk & cross-chunk), `TypedChunkIter::for_each_combination_mut` (within-chunk mutable
/// pairs), and `SystemBuilder::param_set`.
#[test]
fn get_many_mut_combinations_and_param_set() {
    define_pod_component!(struct Score { val: i32 } => c"wave3::Score");

    let mut host = V03TestHost::new();
    let h_score = host.expose_component::<Score>("wave3::Score", Score::UUID, 0, &[]);

    let e1 = FfiEntity::from_bits(11);
    let e2 = FfiEntity::from_bits(12);
    let e3 = FfiEntity::from_bits(13);
    let e4 = FfiEntity::from_bits(14);

    // Two chunks: [e1, e2, e3] and [e4] so we test both within-chunk and cross-chunk combinations.
    let mut c0 = MockChunk::new(vec![e1, e2, e3]);
    c0.insert_dense(
        h_score,
        &[Score { val: 1 }, Score { val: 2 }, Score { val: 3 }],
    );
    let mut c1 = MockChunk::new(vec![e4]);
    c1.insert_dense(h_score, &[Score { val: 4 }]);
    host.chunks.push(c0);
    host.chunks.push(c1);

    let inst = host
        .load_fn(c"wave3_p17_mod", |app| {
            // System 0: get_many_mut (disjoint, duplicate, null, missing)
            let rc = app
                .system(c"get_many_mut_sys")
                .query_typed::<&mut Score>()
                .build(move |ctx| {
                    // Duplicate entity rejected with ACCESS_CONFLICT.
                    assert_eq!(
                        ctx.get_many_mut::<2, Score>(0, 0, [e1, e1]),
                        Err(FfiResult::ACCESS_CONFLICT)
                    );
                    // Null entity rejected with INVALID_ENTITY.
                    assert_eq!(
                        ctx.get_many_mut::<2, Score>(0, 0, [e1, FfiEntity::NULL]),
                        Err(FfiResult::INVALID_ENTITY)
                    );
                    // Missing entity rejected with NOT_FOUND.
                    assert_eq!(
                        ctx.get_many_mut::<2, Score>(0, 0, [e1, FfiEntity::from_bits(999)]),
                        Err(FfiResult::NOT_FOUND)
                    );
                    // Disjoint entities across chunks succeed and mutate in place.
                    let [a, b] = ctx.get_many_mut::<2, Score>(0, 0, [e1, e4])?;
                    a.val += 100;
                    b.val += 400;
                    Ok(())
                });
            assert_eq!(rc, FfiResult::OK);

            // System 1: for_each_combination::<2> (cross-chunk + within-chunk: C(4,2) = 6 pairs)
            let rc = app
                .system(c"combo_read_sys")
                .query_typed::<(FfiEntity, &Score)>()
                .build(|ctx| {
                    let mut pairs = Vec::new();
                    let status = ctx
                        .single_query::<(FfiEntity, &Score)>()
                        .for_each_combination::<2, _, _>(|[(ea, sa), (eb, sb)]| {
                            pairs.push((ea.to_bits(), sa.val, eb.to_bits(), sb.val));
                        });
                    assert_eq!(status, FfiResult::OK);
                    assert_eq!(pairs.len(), 6);
                    assert_eq!(pairs[0], (11, 101, 12, 2));
                    assert_eq!(pairs[5], (13, 3, 14, 404));
                    FfiResult::OK
                });
            assert_eq!(rc, FfiResult::OK);

            // System 2: for_each_combination_mut within chunk 0 (C(3,2) = 3 pairs)
            let rc = app
                .system(c"combo_mut_sys")
                .query_typed::<(FfiEntity, &mut Score)>()
                .build(|ctx| {
                    let mut pair_count = 0usize;
                    let status = ctx
                        .single_query::<(FfiEntity, &mut Score)>()
                        .for_each_combination_mut(|(ea, sa), (eb, sb)| {
                            assert_ne!(ea, eb);
                            sa.val += 1;
                            sb.val += 10;
                            pair_count += 1;
                        });
                    assert_eq!(status, FfiResult::OK);
                    // Chunk 0 has 3 entities -> 3 pairs; Chunk 1 has 1 entity -> 0 pairs.
                    assert_eq!(pair_count, 3);
                    FfiResult::OK
                });
            assert_eq!(rc, FfiResult::OK);

            // System 3: ParamSetBuilder groups mutually exclusive queries under shared non-zero IDs.
            let sb = app
                .system(c"param_set_sys")
                .query_typed::<&Score>()
                .param_set(|ps| {
                    assert_eq!(ps.group_id(), 1);
                    ps.query_typed::<&mut Score>().query_typed::<&mut Score>();
                })
                .param_set(|ps| {
                    assert_eq!(ps.group_id(), 2);
                    ps.query(|q| q.write::<Score>())
                        .query(|q| q.read::<Score>());
                });
            assert_eq!(sb.param_set_groups(), &[0, 1, 1, 2, 2]);
            sb.build(|_ctx| FfiResult::OK)
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.run_system(2), FfiResult::OK);
    assert_eq!(host.run_system(3), FfiResult::OK);

    // Chunk 0 pairs: (0,1), (0,2), (1,2):
    // idx 0: +1 +1 = +2 -> 101 + 2 = 103
    // idx 1: +10 +1 = +11 -> 2 + 11 = 13
    // idx 2: +10 +10 = +20 -> 3 + 20 = 23
    let c0_scores = host.chunks[0].read_dense::<Score>(h_score);
    assert_eq!(
        c0_scores,
        vec![Score { val: 103 }, Score { val: 13 }, Score { val: 23 }]
    );
    let c1_scores = host.chunks[1].read_dense::<Score>(h_score);
    assert_eq!(c1_scores, vec![Score { val: 404 }]);

    host.unload(inst);
}

/// Scenario (P2.8): Verifies `GuestCommands::{clone_entity, insert_resource,
/// insert_resource_typed, insert_guest_resource, remove_resource, remove_guest_resource}` and
/// `GuestEntityCommands::clone_to` over slots `70..=72`.
#[test]
fn commands_clone_entity_resource_insert_remove_and_clone_to() {
    define_pod_component!(struct GlobalScore { points: u32 } => c"wave3::GlobalScore");

    let mut host = V03TestHost::new();
    let h_res =
        host.expose_component::<GlobalScore>("wave3::GlobalScore", GlobalScore::UUID, 0, &[]);

    let inst = host
        .load_fn(c"wave3_p28_mod", move |app| {
            let typed_res = TypedComponent::<GlobalScore>::validate(app.host(), h_res)
                .expect("typed resource handle");
            app.system(c"cmds_p28_sys").build(move |ctx| {
                let cmds = ctx.commands();
                let src = FfiEntity::from_bits(50);
                let dst1 = FfiEntity::from_bits(51);
                let dst2 = FfiEntity::from_bits(52);

                assert_eq!(cmds.clone_entity(src, dst1), FfiResult::OK);
                assert_eq!(cmds.entity(src).clone_to(dst2).status(), FfiResult::OK);
                assert_eq!(
                    cmds.insert_resource(h_res, &GlobalScore { points: 10 }),
                    FfiResult::OK
                );
                assert_eq!(
                    cmds.insert_resource_typed(typed_res, &GlobalScore { points: 20 }),
                    FfiResult::OK
                );
                assert_eq!(
                    cmds.insert_guest_resource(&GlobalScore { points: 30 }),
                    FfiResult::OK
                );
                assert_eq!(cmds.remove_resource(h_res), FfiResult::OK);
                assert_eq!(cmds.remove_guest_resource::<GlobalScore>(), FfiResult::OK);
                FfiResult::OK
            })
        })
        .unwrap();

    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(
        host.commands,
        vec![
            RecordedCmd::CloneEntity {
                src: FfiEntity::from_bits(50),
                dst: FfiEntity::from_bits(51),
            },
            RecordedCmd::CloneEntity {
                src: FfiEntity::from_bits(50),
                dst: FfiEntity::from_bits(52),
            },
            RecordedCmd::InsertResource {
                resource: h_res,
                bytes: 10u32.to_ne_bytes().to_vec(),
            },
            RecordedCmd::InsertResource {
                resource: h_res,
                bytes: 20u32.to_ne_bytes().to_vec(),
            },
            RecordedCmd::InsertResource {
                resource: h_res,
                bytes: 30u32.to_ne_bytes().to_vec(),
            },
            RecordedCmd::RemoveResource { resource: h_res },
            RecordedCmd::RemoveResource { resource: h_res },
        ]
    );

    host.unload(inst);
}

/// Verifies M3 persistent `BamColumnInfo` & term caching for `TypedChunkIter` across multiple
/// ticks:
/// 1. Tick 1 calls `iter_column_info` at most once per term (not once per chunk) and uses slot 68
///    (`0` legacy `iter_column_readonly` / `iter_column_mutable` calls).
/// 2. Tick 2+ makes **`0` steady-state `iter_column_info` calls**.
/// 3. Dynamic branching mismatch (`TypeId` swap on the same slot) misses cache and returns
///    `FfiResult::TYPE_MISMATCH` without poisoning subsequent valid ticks.
/// 4. `QueryTuple::StaticKey` produces pairwise-distinct `TypeId`s across `&T`, `&mut T`,
///    `Option<&T>`, `Option<&mut T>`, `With<T>`, `Without<T>`, `Changed<T>`, `Added<T>`.
/// 5. `par_for_each_chunk_readonly` reads the frozen cache snapshot with `0` `iter_column_info` calls.
/// 6. Optional duplicate component `(&mut Pos, Option<&mut Pos>)` present in chunk still catches
///    `ACCESS_CONFLICT` even when metadata is cached on tick 2!
#[test]
fn typed_chunk_iter_caches_column_info_across_ticks_and_catches_mismatch() {
    use core::any::TypeId;
    use std::sync::Arc;

    define_pod_component!(struct CachePos { x: f32, y: f32 } => c"m3::CachePos");
    define_pod_component!(struct CacheVel { vx: f32, vy: f32 } => c"m3::CacheVel");
    define_pod_component!(struct WrongPos { x: f64, y: f64 } => c"m3::WrongPos");

    // Verify `StaticKey` TypeIds are strictly distinct across all access modes.
    let keys = [
        TypeId::of::<<&CachePos as QueryTuple>::StaticKey>(),
        TypeId::of::<<&mut CachePos as QueryTuple>::StaticKey>(),
        TypeId::of::<<Option<&CachePos> as QueryTuple>::StaticKey>(),
        TypeId::of::<<Option<&mut CachePos> as QueryTuple>::StaticKey>(),
        TypeId::of::<<With<CachePos> as QueryTuple>::StaticKey>(),
        TypeId::of::<<Without<CachePos> as QueryTuple>::StaticKey>(),
        TypeId::of::<<Changed<CachePos> as QueryTuple>::StaticKey>(),
        TypeId::of::<<Added<CachePos> as QueryTuple>::StaticKey>(),
    ];
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            assert_ne!(keys[i], keys[j], "StaticKey collision at ({i}, {j})");
        }
    }

    let mut host = V03TestHost::with_api(make_v03_host_api_with_batch());
    let h_pos = host.expose_component::<CachePos>("m3::CachePos", CachePos::UUID, 0, &[]);
    let h_vel = host.expose_component::<CacheVel>("m3::CacheVel", CacheVel::UUID, 0, &[]);

    // 3 non-empty chunks so tick 1 proves per-query caching (2 info calls, not 2 * 3 = 6).
    for c_idx in 0..3u64 {
        let mut chunk = MockChunk::new(vec![
            FfiEntity::from_bits(c_idx * 10 + 1),
            FfiEntity::from_bits(c_idx * 10 + 2),
        ]);
        chunk.insert_dense(
            h_pos,
            &[CachePos { x: 1.0, y: 2.0 }, CachePos { x: 3.0, y: 4.0 }],
        );
        chunk.insert_dense(
            h_vel,
            &[CacheVel { vx: 0.5, vy: 1.5 }, CacheVel { vx: 2.5, vy: 3.5 }],
        );
        host.chunks.push(chunk);
    }

    let sys0_tick = Arc::new(AtomicUsize::new(0));
    let sys0_tick_clone = Arc::clone(&sys0_tick);

    let inst = host
        .load_fn(c"m3_typed_cache_mod", move |app| {
            // System 0: (&mut CachePos, &CacheVel) across ticks 1..=4 (tick 3 swaps to WrongPos).
            let rc = app
                .system(c"dense_cached_sys")
                .query_typed::<(&mut CachePos, &CacheVel)>()
                .build(move |ctx| {
                    let tick = sys0_tick_clone.fetch_add(1, Ordering::Relaxed) + 1;
                    if tick == 3 {
                        return ctx
                            .single_query::<(&mut WrongPos, &CacheVel)>()
                            .for_each(|_| FfiResult::OK);
                    }
                    ctx.single_query::<(&mut CachePos, &CacheVel)>().for_each(
                        |(positions, velocities)| {
                            for (p, v) in positions.iter_mut().zip(velocities.iter()) {
                                p.x += v.vx;
                                p.y += v.vy;
                            }
                        },
                    )
                });
            if !rc.is_ok() {
                return rc;
            }

            // System 1: Read-only query warming cache on tick 1, then running `par_for_each_chunk_readonly`
            // on tick 2 with 0 `iter_column_info` calls across workers.
            let sys1_tick = AtomicUsize::new(0);
            let rc = app
                .system(c"par_ro_cached_sys")
                .query_typed::<(&CachePos, &CacheVel)>()
                .build(move |ctx| {
                    let tick = sys1_tick.fetch_add(1, Ordering::Relaxed) + 1;
                    if tick == 1 {
                        ctx.single_query::<(&CachePos, &CacheVel)>()
                            .for_each(|_| FfiResult::OK)
                    } else {
                        ctx.single_query::<(&CachePos, &CacheVel)>()
                            .par_for_each_chunk_readonly(|(pos, vel)| {
                                assert_eq!(pos.len(), vel.len());
                                FfiResult::OK
                            })
                    }
                });
            if !rc.is_ok() {
                return rc;
            }

            // System 2: Hostile duplicate optional component `(&mut CachePos, Option<&mut CachePos>)`.
            app.system(c"dup_optional_write_sys")
                .query(|q| q.write::<CachePos>().optional_write::<CachePos>())
                .build(|ctx| {
                    ctx.single_query::<(&mut CachePos, Option<&mut CachePos>)>()
                        .for_each(|_| FfiResult::OK)
                })
        })
        .unwrap();

    // Tick 1 of System 0 (3 chunks): <= 2 `iter_column_info` calls, 0 legacy col calls!
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 2);
    assert_eq!(host.iter_column_ro_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.iter_column_mut_calls.load(Ordering::Relaxed), 0);

    // Tick 2 of System 0 (3 chunks): 0 steady-state `iter_column_info` calls!
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 2);
    assert_eq!(host.iter_column_ro_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.iter_column_mut_calls.load(Ordering::Relaxed), 0);

    // Tick 3 of System 0 (`WrongPos` mismatch): returns `TYPE_MISMATCH` without extra FFI (hits col_infos).
    assert_eq!(host.run_system(0), FfiResult::TYPE_MISMATCH);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 2);

    // Tick 4 of System 0: cache wasn't poisoned by tick 3's mismatch; still 0 `iter_column_info` calls!
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 2);

    // Verify math from ticks 1, 2, 4 (3 updates of +vx/+vy):
    let updated = host.chunks[0].read_dense::<CachePos>(h_pos);
    assert_eq!(
        updated,
        vec![CachePos { x: 2.5, y: 6.5 }, CachePos { x: 10.5, y: 14.5 }]
    );

    // System 1: Tick 1 warms cache (2 calls), Tick 2 `par_for_each_chunk_readonly` makes 0 calls!
    let before_sys1 = host.iter_column_info_calls.load(Ordering::Relaxed);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(
        host.iter_column_info_calls.load(Ordering::Relaxed) - before_sys1,
        2
    );
    let before_sys1_t2 = host.iter_column_info_calls.load(Ordering::Relaxed);
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(
        host.iter_column_info_calls.load(Ordering::Relaxed) - before_sys1_t2,
        0
    );

    // System 2: Hostile duplicate optional component (`h_pos` present in chunk) rejected with ACCESS_CONFLICT,
    // even on tick 2 when `BamColumnInfo` is cached!
    assert_eq!(host.run_system(2), FfiResult::ACCESS_CONFLICT);
    let before_sys2_t2 = host.iter_column_info_calls.load(Ordering::Relaxed);
    assert_eq!(host.run_system(2), FfiResult::ACCESS_CONFLICT);
    assert_eq!(
        host.iter_column_info_calls.load(Ordering::Relaxed) - before_sys2_t2,
        0
    );

    host.unload(inst);
}

/// Verifies M3 persistent resource metadata caching for `ctx.res_with_ticks::<R>(0)`:
/// 1. Tick 1 (64-call loop): exactly `1` `iter_column_info` call (call #1) and `0` on calls `2..=64`,
///    with `res_ticks_calls == 64`.
/// 2. Tick 2 (64-call loop): **`0` steady-state `iter_column_info` calls**, `res_ticks_calls == 64`.
/// 3. Tick 3 (mismatched resource type `OtherParams`): returns `Err(FfiResult::NOT_FOUND)` cleanly
///    without aliasing or poisoning the cache for Tick 4.
#[test]
fn res_with_ticks_64_call_loop_makes_zero_steady_state_iter_column_info_calls() {
    use std::sync::Arc;

    define_pod_component!(struct SimParamsRes { dt: f32, tick: u32 } => c"m3::SimParamsRes");
    define_pod_component!(struct OtherParamsRes { scale: f32, mode: u32 } => c"m3::OtherParamsRes");

    let mut host = V03TestHost::with_api(make_v03_host_api_with_batch());
    let h_sim =
        host.expose_component::<SimParamsRes>("m3::SimParamsRes", SimParamsRes::UUID, 0, &[]);
    let _h_other =
        host.expose_component::<OtherParamsRes>("m3::OtherParamsRes", OtherParamsRes::UUID, 0, &[]);
    host.set_resource(
        h_sim,
        SimParamsRes { dt: 0.016, tick: 7 },
        BamChangeTicks {
            added: 1,
            changed: 6,
            last_run: 5,
            this_run: 10,
        },
    );

    let tick_counter = Arc::new(AtomicUsize::new(0));
    let tick_clone = Arc::clone(&tick_counter);

    let inst = host
        .load_fn(c"m3_res_cache_mod", move |app| {
            app.system(c"res_roundtrip_sys")
                .query(|q| q.resource_read::<SimParamsRes>())
                .build(move |ctx| {
                    let tick = tick_clone.fetch_add(1, Ordering::Relaxed) + 1;
                    if tick == 3 {
                        assert_eq!(
                            ctx.res_with_ticks::<OtherParamsRes>(0).err(),
                            Some(FfiResult::NOT_FOUND)
                        );
                        return FfiResult::OK;
                    }
                    let mut sum = 0u32;
                    for _ in 0..64 {
                        let res = match ctx.res_with_ticks::<SimParamsRes>(0) {
                            Ok(r) => r,
                            Err(err) => return err,
                        };
                        assert_eq!(res.tick, 7);
                        assert!((res.dt - 0.016).abs() < 1e-6);
                        sum = sum.wrapping_add(res.ticks().this_run + res.tick);
                    }
                    assert_eq!(sum, 64 * (10 + 7));
                    FfiResult::OK
                })
        })
        .unwrap();

    // Tick 1 (64 calls): 1 `iter_column_info` call, 64 `res_ticks` calls.
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.res_ticks_calls.load(Ordering::Relaxed), 64);

    // Tick 2 (64 calls): 0 steady-state `iter_column_info` calls, 64 `res_ticks` calls!
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.res_ticks_calls.load(Ordering::Relaxed), 128);

    // Tick 3 (mismatched `OtherParamsRes`): rejects cleanly without extra FFI or cache poisoning.
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 1);

    // Tick 4 (64 calls of `SimParamsRes` again): still 0 steady-state `iter_column_info` calls!
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.iter_column_info_calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.res_ticks_calls.load(Ordering::Relaxed), 193);

    host.unload(inst);
}

#[test]
fn spawn_batch_column_typed_skips_layout_info_and_enforces_bounds() {
    use bamabi_guest::commands::SpawnBatchColumn;

    define_pod_component!(struct BatchPos { x: f32, y: f32 } => c"wave3::BatchPos");
    define_pod_component!(struct BatchVel { vx: f32, vy: f32 } => c"wave3::BatchVel");

    let mut host = V03TestHost::new();
    let h_pos = host.expose_component::<BatchPos>("wave3::BatchPos", BatchPos::UUID, 0, &[]);
    let h_vel = host.expose_component::<BatchVel>("wave3::BatchVel", BatchVel::UUID, 0, &[]);

    let inst = host
        .load_fn(c"wave3_typed_batch_mod", move |app| {
            let pos_typed = TypedComponent::<BatchPos>::validate(app.host(), h_pos)
                .expect("valid typed BatchPos handle");
            let vel_typed = TypedComponent::<BatchVel>::validate(app.host(), h_vel)
                .expect("valid typed BatchVel handle");

            let st = app.system(c"typed_batch_sys").build(move |ctx| {
                let cmds = ctx.commands();
                let short_pos = [BatchPos { x: 1.0, y: 2.0 }];
                let positions = [
                    BatchPos { x: 1.0, y: 2.0 },
                    BatchPos { x: 3.0, y: 4.0 },
                    BatchPos { x: 5.0, y: 6.0 },
                ];
                let vels = [
                    BatchVel { vx: 0.1, vy: 0.2 },
                    BatchVel { vx: 0.3, vy: 0.4 },
                    BatchVel { vx: 0.5, vy: 0.6 },
                ];

                assert_eq!(
                    cmds.spawn_batch(
                        &[
                            SpawnBatchColumn::typed(pos_typed, &short_pos),
                            SpawnBatchColumn::typed(vel_typed, &vels),
                        ],
                        3,
                    ),
                    Err(FfiResult::INVALID_ARGUMENT)
                );

                let entities = match cmds.spawn_batch(
                    &[
                        SpawnBatchColumn::typed(pos_typed, &positions),
                        SpawnBatchColumn::typed(vel_typed, &vels),
                    ],
                    3,
                ) {
                    Ok(v) => v,
                    Err(err) => return err,
                };
                assert_eq!(entities.len(), 3);
                assert!(entities.iter().all(|e| e.is_valid()));
                assert_ne!(entities[0], entities[1]);
                assert_ne!(entities[1], entities[2]);

                let empty =
                    match cmds.spawn_batch(&[SpawnBatchColumn::typed(pos_typed, &positions)], 0) {
                        Ok(v) => v,
                        Err(err) => return err,
                    };
                assert!(empty.is_empty());
                FfiResult::OK
            });
            if !st.is_ok() {
                return st;
            }

            app.system(c"untyped_batch_sys").build(move |ctx| {
                let cmds = ctx.commands();
                let positions = [
                    BatchPos { x: 7.0, y: 8.0 },
                    BatchPos { x: 9.0, y: 10.0 },
                    BatchPos { x: 11.0, y: 12.0 },
                ];
                let entities =
                    match cmds.spawn_batch(&[SpawnBatchColumn::new(h_pos, &positions)], 3) {
                        Ok(v) => v,
                        Err(err) => return err,
                    };
                assert_eq!(entities.len(), 3);
                FfiResult::OK
            })
        })
        .unwrap();

    // Init-time TypedComponent::validate already ran `component_layout_info` for both components.
    assert!(host.layout_info_calls.load(Ordering::Relaxed) >= 2);
    host.layout_info_calls.store(0, Ordering::Relaxed);
    host.sys_host_ctx_calls.store(0, Ordering::Relaxed);

    // Typed batch path makes 0 `component_layout_info` and 0 `sys_host_ctx` FFI calls on tick.
    assert_eq!(host.run_system(0), FfiResult::OK);
    assert_eq!(host.layout_info_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.sys_host_ctx_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        host.commands,
        vec![
            RecordedCmd::SpawnBatch {
                components: vec![h_pos, h_vel],
                entity_count: 3,
            },
            RecordedCmd::SpawnBatch {
                components: vec![h_pos],
                entity_count: 0,
            },
        ]
    );

    // Untyped batch path contrasts by invoking `component_layout_info` once per column.
    assert_eq!(host.run_system(1), FfiResult::OK);
    assert_eq!(host.layout_info_calls.load(Ordering::Relaxed), 1);

    host.unload(inst);
}
