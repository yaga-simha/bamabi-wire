//! End-to-end ABI `0.4.0` integration tests covering `bamabi_guest` fast-path terms,
//! `SelectiveMut<T>` coalescing, `UntrackedMut<T>`, `Sparse<T>`, 20-term `QueryTuple`,
//! `Chunk::columns_dispenser`, `Chunk::component_rows(&self)`, `ChunkIter::next_chunk_batch`,
//! `SystemContext::split`, handle-based messages, ticks/time/named-objects/symbols/relations,
//! batch commands + splat spawn, `TypedObserverBuilder::{build_mut_with_ctx, run_if_with}`,
//! `EcsRunCondition::{and, or, xor}`, `GuestApp` component caching, and `SCHEMA_FINGERPRINT`
//! / `BAM_FIELDS` propagation.

#![allow(unsafe_code)]

use core::ffi::{CStr, c_char, c_void};
use core::mem::{align_of, size_of};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::collections::HashMap;

use bamabi_guest::raw::{
    ABI_VERSION, AbiPod, AccessMode, BAM_COMPONENT_FLAG_SPARSE_SET, BAM_SPAWN_COL_FLAG_SPLAT,
    BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE, BAM_STORAGE_FLAG_SPARSE_SET,
    BAM_TERM_FLAG_ADDED, BAM_TERM_FLAG_CHANGED, BamChunkTaskExFn, BamChunkTaskFn, BamColumnInfo,
    BamCommands, BamComponent, BamFieldDesc, BamFieldKind, BamHostApi, BamHostCtx, BamInstance,
    BamIter, BamLayoutInfo, BamLoadCtx, BamManifest, BamObserverCtxMutFn, BamObserverDesc,
    BamParOptions, BamRowRange, BamSpawnColumnDesc, BamSymbol, BamSystemCtx, BamSystemDesc,
    BamSystemFn, BamTermDesc, BamTimeInfo, ComponentDesc, ComponentUuid, FfiBool, FfiEntity,
    FfiResult, RawInsertable, StrView, init_guest_instance, shutdown_guest_instance,
};
use bamabi_guest::{
    Chunk, GuestApp, GuestComponent, GuestPlugin, SelectiveMut, Sparse, SpawnBatchColumn,
    TypedComponent, UntrackedMut, build_guest_fn, build_guest_plugin,
    conditions::{on_message, resource_exists},
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pos {
    x: f32,
    y: f32,
}
// SAFETY: `Pos` is `#[repr(C)]` over two `f32` primitives with zero padding.
unsafe impl RawInsertable for Pos {}
// SAFETY: All fields are `AbiPod` with zero padding.
unsafe impl AbiPod for Pos {}

static POS_FIELDS: [BamFieldDesc; 2] = [
    BamFieldDesc::new(
        BamFieldKind::F32,
        StrView::from_str("x"),
        StrView::from_str("f32"),
        0,
        4,
        4,
    ),
    BamFieldDesc::new(
        BamFieldKind::F32,
        StrView::from_str("y"),
        StrView::from_str("f32"),
        4,
        4,
        4,
    ),
];

impl GuestComponent for Pos {
    const NAME: &'static CStr = c"v04::Pos";
    const UUID: ComponentUuid = ComponentUuid::from_parts("v04::Pos", 8, 4, 0x1111);
    const SCHEMA_FINGERPRINT: u64 = 0xDEAD_BEEF_CAFE_0001;
    const BAM_FIELDS: &'static [BamFieldDesc] = &POS_FIELDS;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct SparseTag {
    id: u32,
}
// SAFETY: `SparseTag` is `#[repr(C)]` over `u32` with zero padding.
unsafe impl RawInsertable for SparseTag {}
// SAFETY: Single `u32` primitive, zero padding.
unsafe impl AbiPod for SparseTag {}
impl GuestComponent for SparseTag {
    const NAME: &'static CStr = c"v04::SparseTag";
    const FLAGS: u32 = BAM_COMPONENT_FLAG_SPARSE_SET;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct SparseVel {
    dx: f32,
}
// SAFETY: `SparseVel` is `#[repr(C)]` over `f32` with zero padding.
unsafe impl RawInsertable for SparseVel {}
// SAFETY: Single `f32` primitive, zero padding.
unsafe impl AbiPod for SparseVel {}
impl GuestComponent for SparseVel {
    const NAME: &'static CStr = c"v04::SparseVel";
    const FLAGS: u32 = BAM_COMPONENT_FLAG_SPARSE_SET;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct TickEvent {
    delta: u32,
}
// SAFETY: `TickEvent` is `#[repr(C)]` over `u32`.
unsafe impl RawInsertable for TickEvent {}
// SAFETY: Single `u32` primitive, zero padding.
unsafe impl AbiPod for TickEvent {}
impl GuestComponent for TickEvent {
    const NAME: &'static CStr = c"v04::TickEvent";
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct SimConfig {
    gravity: f32,
}
// SAFETY: `SimConfig` is `#[repr(C)]` over `f32`.
unsafe impl RawInsertable for SimConfig {}
// SAFETY: Single `f32` primitive, zero padding.
unsafe impl AbiPod for SimConfig {}
impl GuestComponent for SimConfig {
    const NAME: &'static CStr = c"v04::SimConfig";
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct MsgItem {
    seq: u32,
}
// SAFETY: `MsgItem` is `#[repr(C)]` over `u32`.
unsafe impl RawInsertable for MsgItem {}
// SAFETY: Single `u32` primitive, zero padding.
unsafe impl AbiPod for MsgItem {}
impl GuestComponent for MsgItem {
    const NAME: &'static CStr = c"v04::MsgItem";
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct RelEdge {
    weight: f32,
}
// SAFETY: `RelEdge` is `#[repr(C)]` over `f32`.
unsafe impl RawInsertable for RelEdge {}
// SAFETY: Single `f32` primitive, zero padding.
unsafe impl AbiPod for RelEdge {}
impl GuestComponent for RelEdge {
    const NAME: &'static CStr = c"v04::RelEdge";
}

macro_rules! def_unit_u32_comp {
    ($ident:ident, $cname:literal) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct $ident(u32);
        // SAFETY: `#[repr(C)]` transparent-layout `u32` wrapper with zero padding.
        unsafe impl RawInsertable for $ident {}
        // SAFETY: `u32` has zero padding and all bit patterns valid.
        unsafe impl AbiPod for $ident {}
        impl GuestComponent for $ident {
            const NAME: &'static CStr = $cname;
        }
    };
}

def_unit_u32_comp!(C0, c"v04::C0");
def_unit_u32_comp!(C1, c"v04::C1");
def_unit_u32_comp!(C2, c"v04::C2");
def_unit_u32_comp!(C3, c"v04::C3");
def_unit_u32_comp!(C4, c"v04::C4");
def_unit_u32_comp!(C5, c"v04::C5");
def_unit_u32_comp!(C6, c"v04::C6");
def_unit_u32_comp!(C7, c"v04::C7");
def_unit_u32_comp!(C8, c"v04::C8");
def_unit_u32_comp!(C9, c"v04::C9");
def_unit_u32_comp!(C10, c"v04::C10");
def_unit_u32_comp!(C11, c"v04::C11");
def_unit_u32_comp!(C12, c"v04::C12");
def_unit_u32_comp!(C13, c"v04::C13");
def_unit_u32_comp!(C14, c"v04::C14");
def_unit_u32_comp!(C15, c"v04::C15");
def_unit_u32_comp!(C16, c"v04::C16");
def_unit_u32_comp!(C17, c"v04::C17");
def_unit_u32_comp!(C18, c"v04::C18");

#[derive(Clone)]
struct MockRegComp {
    handle: BamComponent,
    uuid: ComponentUuid,
    size: usize,
    align: usize,
    storage_flags: u32,
    schema_fingerprint: u64,
    field_count: usize,
}

#[derive(Default)]
struct MockIterState {
    step: usize,
    entities: Vec<FfiEntity>,
    terms: Vec<BamTermDesc>,
    dense_cols: HashMap<usize, AlignedBytes>,
    sparse_ro_ptrs: HashMap<usize, Vec<*const u8>>,
    sparse_rw_ptrs: HashMap<usize, Vec<*mut u8>>,
    messages: HashMap<usize, AlignedBytes>,
    msg_elem_size: HashMap<usize, usize>,
    msg_cursor: HashMap<usize, usize>,
    sent_messages: Vec<(usize, Vec<u8>)>,
    col_mutable_calls: usize,
    col_untracked_calls: usize,
    mark_changed_calls: Vec<(usize, usize, usize)>,
    mark_ranges_calls: Vec<(usize, Vec<BamRowRange>)>,
    mark_ranges_status_seq: Vec<FfiResult>,
    mark_range_status_seq: Vec<FfiResult>,
    batch_calls: usize,
    last_batch_mask: Option<u32>,
    par_calls: usize,
    par_readonly_calls: usize,
    par_ex_calls: usize,
    last_par_options: Option<BamParOptions>,
}

struct MockObserverEntry {
    desc: BamObserverDesc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MockCmdRecord {
    DespawnAll(Vec<BamTermDesc>),
    DespawnBatch(Vec<FfiEntity>),
    InsertBatch {
        entities: Vec<FfiEntity>,
        component: BamComponent,
        elem_size: usize,
        flags: u32,
        bytes: Vec<u8>,
    },
    RemoveBatch {
        entities: Vec<FfiEntity>,
        component: BamComponent,
    },
    SpawnBatchEx {
        cols: Vec<(BamComponent, u32, usize, Vec<u8>)>,
        entity_count: usize,
        out_entities: Vec<FfiEntity>,
    },
}

struct AlignedBytes {
    words: Vec<u128>,
    len: usize,
}

impl AlignedBytes {
    fn from_pod<T: AbiPod>(items: &[T]) -> Self {
        let len = core::mem::size_of_val(items);
        let mut raw = Vec::with_capacity(len);
        for item in items {
            raw.extend_from_slice(item.as_bytes());
        }
        let words = raw
            .chunks(16)
            .map(|chunk| {
                let mut buf = [0u8; 16];
                buf[..chunk.len()].copy_from_slice(chunk);
                u128::from_ne_bytes(buf)
            })
            .collect();
        Self { words, len }
    }

    fn as_ptr(&self) -> *const u8 {
        self.words.as_ptr().cast::<u8>()
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.words.as_mut_ptr().cast::<u8>()
    }

    fn len(&self) -> usize {
        self.len
    }

    fn as_pod_slice<T: AbiPod>(&self) -> &[T] {
        assert!(core::mem::align_of::<T>() <= core::mem::align_of::<u128>());
        let count = self.len / core::mem::size_of::<T>();
        // SAFETY: `words` is `u128`-aligned (`>= align_of::<T>()`), `len` bytes are initialized `T`s.
        unsafe { core::slice::from_raw_parts(self.words.as_ptr().cast::<T>(), count) }
    }
}

struct MockWorld {
    next_handle: u32,
    next_entity_id: u64,
    comps_by_name: HashMap<String, BamComponent>,
    comps_by_handle: HashMap<BamComponent, MockRegComp>,
    resolve_calls: AtomicUsize,
    registered_resources: Vec<BamComponent>,
    registered_messages: Vec<BamComponent>,
    registered_systems: Vec<(String, BamSystemFn, *mut c_void, Vec<Vec<BamTermDesc>>)>,
    registered_observers: Vec<MockObserverEntry>,
    named_objects: HashMap<String, FfiEntity>,
    symbols: Vec<String>,
    symbol_map: HashMap<String, BamSymbol>,
    ticks: (u32, u32),
    time_info: BamTimeInfo,
    relations_targets: HashMap<(FfiEntity, BamComponent), Vec<FfiEntity>>,
    relations_sources: HashMap<(FfiEntity, BamComponent), Vec<FfiEntity>>,
    relation_edges: HashMap<(FfiEntity, BamComponent, FfiEntity), AlignedBytes>,
    commands: Vec<MockCmdRecord>,
    iters: Vec<core::pin::Pin<Box<MockIterState>>>,
}

impl Default for MockWorld {
    fn default() -> Self {
        Self {
            next_handle: 1,
            next_entity_id: 100,
            comps_by_name: HashMap::new(),
            comps_by_handle: HashMap::new(),
            resolve_calls: AtomicUsize::new(0),
            registered_resources: Vec::new(),
            registered_messages: Vec::new(),
            registered_systems: Vec::new(),
            registered_observers: Vec::new(),
            named_objects: HashMap::new(),
            symbols: Vec::new(),
            symbol_map: HashMap::new(),
            ticks: (40, 42),
            time_info: BamTimeInfo::new(),
            relations_targets: HashMap::new(),
            relations_sources: HashMap::new(),
            relation_edges: HashMap::new(),
            commands: Vec::new(),
            iters: Vec::new(),
        }
    }
}

impl MockWorld {
    fn register_comp_inner(
        &mut self,
        desc: &ComponentDesc,
        extra_storage_flags: u32,
    ) -> BamComponent {
        // SAFETY: `desc.name` is valid C string in test harness.
        let name = unsafe { CStr::from_ptr(desc.name) }
            .to_str()
            .expect("utf8")
            .to_owned();
        if let Some(&existing) = self.comps_by_name.get(&name) {
            return existing;
        }
        let handle = BamComponent::from_raw(self.next_handle);
        self.next_handle += 1;
        let mut storage_flags = extra_storage_flags;
        if (desc.flags & BAM_COMPONENT_FLAG_SPARSE_SET) != 0 {
            storage_flags |= BAM_STORAGE_FLAG_SPARSE_SET;
        }
        self.comps_by_name.insert(name, handle);
        self.comps_by_handle.insert(
            handle,
            MockRegComp {
                handle,
                uuid: desc.uuid,
                size: desc.size,
                align: desc.align,
                storage_flags,
                schema_fingerprint: desc.schema_fingerprint,
                field_count: desc.field_count,
            },
        );
        handle
    }

    fn alloc_static_comp<T: GuestComponent>(&mut self, extra_storage_flags: u32) -> BamComponent {
        let name = T::NAME.to_str().expect("utf8").to_owned();
        if let Some(&existing) = self.comps_by_name.get(&name) {
            return existing;
        }
        let handle = BamComponent::from_raw(self.next_handle);
        self.next_handle += 1;
        let mut storage_flags = extra_storage_flags;
        if (T::FLAGS & BAM_COMPONENT_FLAG_SPARSE_SET) != 0 {
            storage_flags |= BAM_STORAGE_FLAG_SPARSE_SET;
        }
        self.comps_by_name.insert(name, handle);
        self.comps_by_handle.insert(
            handle,
            MockRegComp {
                handle,
                uuid: T::UUID,
                size: size_of::<T>(),
                align: align_of::<T>(),
                storage_flags,
                schema_fingerprint: T::SCHEMA_FINGERPRINT,
                field_count: T::BAM_FIELDS.len(),
            },
        );
        handle
    }
}

#[repr(transparent)]
struct MockHost(core::cell::UnsafeCell<MockWorld>);

impl MockHost {
    fn new(world: MockWorld) -> Self {
        Self(core::cell::UnsafeCell::new(world))
    }

    fn as_ptr(&self) -> *const Self {
        core::ptr::from_ref(self)
    }

    /// # Safety
    /// No active `&mut MockWorld` may overlap the returned borrow.
    unsafe fn world(&self) -> &MockWorld {
        // SAFETY: Caller guarantees no overlapping mutable borrow is active.
        unsafe { &*self.0.get() }
    }

    /// Returns a raw `*mut MockWorld` pointer rooted at the underlying `UnsafeCell`.
    fn world_ptr(&self) -> *mut MockWorld {
        self.0.get()
    }
}

unsafe extern "C" fn h_get_proc_address(_: *const BamHostCtx, _: *const c_char) -> *const c_void {
    core::ptr::null()
}
unsafe extern "C" fn h_last_error(_: *const BamHostCtx, buf: *mut c_char, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        // SAFETY: `buf` writable for at least 1 byte.
        unsafe { *buf = 0 };
    }
    0
}
unsafe extern "C" fn h_load_host_ctx(lc: *const BamLoadCtx) -> *const BamHostCtx {
    lc.cast::<BamHostCtx>()
}
unsafe extern "C" fn h_register_component(
    lc: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out: *mut BamComponent,
) -> FfiResult {
    // SAFETY: Non-null test harness pointers (`lc` points to `MockHost`).
    let (w, d) = unsafe { (&mut *(*lc.cast::<MockHost>()).world_ptr(), &*desc) };
    let h = w.register_comp_inner(d, 0);
    // SAFETY: Writable output handle.
    unsafe { *out = h };
    FfiResult::OK
}
unsafe extern "C" fn h_register_resource(
    lc: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out: *mut BamComponent,
) -> FfiResult {
    // SAFETY: Non-null test harness pointers (`lc` points to `MockHost`).
    let (w, d) = unsafe { (&mut *(*lc.cast::<MockHost>()).world_ptr(), &*desc) };
    let h = w.register_comp_inner(d, BAM_STORAGE_FLAG_RESOURCE);
    w.registered_resources.push(h);
    // SAFETY: Writable output handle.
    unsafe { *out = h };
    FfiResult::OK
}
unsafe extern "C" fn h_register_message(
    lc: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out: *mut BamComponent,
) -> FfiResult {
    // SAFETY: Non-null test harness pointers (`lc` points to `MockHost`).
    let (w, d) = unsafe { (&mut *(*lc.cast::<MockHost>()).world_ptr(), &*desc) };
    let h = w.register_comp_inner(d, BAM_STORAGE_FLAG_MESSAGE);
    w.registered_messages.push(h);
    // SAFETY: Writable output handle.
    unsafe { *out = h };
    FfiResult::OK
}
unsafe extern "C" fn h_resolve_component(
    ctx: *const BamHostCtx,
    name: *const c_char,
    uuid: *const ComponentUuid,
    out: *mut BamComponent,
) -> FfiResult {
    // SAFETY: Non-null test harness context (`ctx` points to `MockHost`).
    let w = unsafe { (*ctx.cast::<MockHost>()).world() };
    w.resolve_calls.fetch_add(1, Ordering::SeqCst);
    if !name.is_null() {
        // SAFETY: Valid C string from caller.
        let s = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
        if let Some(&h) = w.comps_by_name.get(s) {
            // SAFETY: Writable out handle.
            unsafe { *out = h };
            return FfiResult::OK;
        }
    }
    if !uuid.is_null() {
        // SAFETY: Readable UUID pointer.
        let target_uuid = unsafe { *uuid };
        if let Some(c) = w.comps_by_handle.values().find(|c| c.uuid == target_uuid) {
            // SAFETY: Writable out handle.
            unsafe { *out = c.handle };
            return FfiResult::OK;
        }
    }
    FfiResult::NOT_FOUND
}
unsafe extern "C" fn h_component_layout_info(
    ctx: *const BamHostCtx,
    comp: BamComponent,
    out_info: *mut BamLayoutInfo,
) -> FfiResult {
    // SAFETY: Non-null test harness context (`ctx` points to `MockHost`) and writable `out_info`.
    let w = unsafe { (*ctx.cast::<MockHost>()).world() };
    let Some(reg) = w.comps_by_handle.get(&comp) else {
        return FfiResult::INVALID_COMPONENT;
    };
    let mut info = BamLayoutInfo::empty();
    info.flags = reg.storage_flags;
    info.uuid = reg.uuid;
    info.size = reg.size;
    info.align = reg.align;
    info.schema_fingerprint = reg.schema_fingerprint;
    info.field_count = reg.field_count;
    // SAFETY: Valid writable `out_info`.
    unsafe { *out_info = info };
    FfiResult::OK
}
unsafe extern "C" fn h_register_system(
    lc: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> FfiResult {
    // SAFETY: Non-null test harness pointers (`lc` points to `MockHost`).
    let (w, d) = unsafe { (&mut *(*lc.cast::<MockHost>()).world_ptr(), &*desc) };
    // SAFETY: Valid C string.
    let name = unsafe { CStr::from_ptr(d.name) }
        .to_str()
        .unwrap_or("")
        .to_owned();
    let mut qs = Vec::new();
    if d.query_count > 0 && !d.queries.is_null() {
        // SAFETY: `d.queries` has `d.query_count` entries.
        for q in unsafe { core::slice::from_raw_parts(d.queries, d.query_count) } {
            let terms = if q.term_count > 0 && !q.terms.is_null() {
                // SAFETY: `q.terms` has `q.term_count` entries.
                unsafe { core::slice::from_raw_parts(q.terms, q.term_count) }.to_vec()
            } else {
                Vec::new()
            };
            qs.push(terms);
        }
    }
    if let Some(cb) = d.callback {
        w.registered_systems.push((name, cb, d.user_data, qs));
    }
    FfiResult::OK
}
unsafe extern "C" fn h_register_observer(
    lc: *mut BamLoadCtx,
    desc: *const BamObserverDesc,
) -> FfiResult {
    // SAFETY: Non-null test harness pointers (`lc` points to `MockHost`).
    let (w, d) = unsafe { (&mut *(*lc.cast::<MockHost>()).world_ptr(), &*desc) };
    w.registered_observers.push(MockObserverEntry { desc: *d });
    FfiResult::OK
}

struct MockSysCtx<'a> {
    host: &'a MockHost,
    iter_indices: Vec<usize>,
}

unsafe extern "C" fn h_sys_host_ctx(sc: *const BamSystemCtx) -> *const BamHostCtx {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    ctx.host.as_ptr().cast::<BamHostCtx>()
}
unsafe extern "C" fn h_sys_query(
    sc: *mut BamSystemCtx,
    idx: usize,
    out_iter: *mut *mut BamIter,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    let Some(&it_idx) = ctx.iter_indices.get(idx) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let ptr = {
        // SAFETY: Non-overlapping short-lived borrow of `MockWorld::iters`.
        let w = unsafe { &mut *ctx.host.world_ptr() };
        let Some(slot) = w.iters.get_mut(it_idx) else {
            return FfiResult::OUT_OF_RANGE;
        };
        core::ptr::from_mut(slot.as_mut().get_mut()).cast::<BamIter>()
    };
    // SAFETY: Writable out pointer.
    unsafe { *out_iter = ptr };
    FfiResult::OK
}
unsafe extern "C" fn h_sys_commands(
    sc: *mut BamSystemCtx,
    out_cmds: *mut *mut BamCommands,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    let ptr = ctx.host.as_ptr().cast::<BamCommands>().cast_mut();
    // SAFETY: Writable out pointer.
    unsafe { *out_cmds = ptr };
    FfiResult::OK
}
unsafe extern "C" fn h_iter_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    if st.step == 0 && !st.entities.is_empty() {
        st.step = 1;
        // SAFETY: Writable `out_len`.
        unsafe { *out_len = st.entities.len() };
        FfiBool::TRUE
    } else {
        // SAFETY: Writable `out_len`.
        unsafe { *out_len = 0 };
        FfiBool::FALSE
    }
}
unsafe extern "C" fn h_iter_next_chunk_batch(
    iter: *mut BamIter,
    out_len: *mut usize,
    out_entities: *mut *const FfiEntity,
    out_col_ptrs: *mut *mut u8,
    col_count: usize,
    mark_changed_mask: u32,
) -> FfiBool {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.batch_calls += 1;
    st.last_batch_mask = Some(mark_changed_mask);
    if st.step == 0 && !st.entities.is_empty() {
        st.step = 1;
        // SAFETY: Writable `out_len`.
        unsafe { *out_len = st.entities.len() };
        if !out_entities.is_null() {
            // SAFETY: Writable `out_entities`.
            unsafe { *out_entities = st.entities.as_ptr() };
        }
        if !out_col_ptrs.is_null() {
            // SAFETY: Writable `col_count` pointers.
            let slice = unsafe { core::slice::from_raw_parts_mut(out_col_ptrs, col_count) };
            for (idx, slot) in slice.iter_mut().enumerate() {
                *slot = st
                    .dense_cols
                    .get_mut(&idx)
                    .map_or(core::ptr::null_mut(), |v| v.as_mut_ptr());
            }
        }
        FfiBool::TRUE
    } else {
        // SAFETY: Writable `out_len`.
        unsafe { *out_len = 0 };
        if !out_entities.is_null() {
            // SAFETY: Writable `out_entities`.
            unsafe { *out_entities = core::ptr::null() };
        }
        if !out_col_ptrs.is_null() {
            // SAFETY: Zeroing slice on exhaustion.
            let slice = unsafe { core::slice::from_raw_parts_mut(out_col_ptrs, col_count) };
            slice.fill(core::ptr::null_mut());
        }
        FfiBool::FALSE
    }
}
unsafe extern "C" fn h_iter_reset(iter: *mut BamIter) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.step = 0;
    FfiResult::OK
}
unsafe extern "C" fn h_iter_entities(iter: *const BamIter) -> *const FfiEntity {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    st.entities.as_ptr()
}
unsafe extern "C" fn h_iter_col_ro(iter: *const BamIter, term: usize) -> *const u8 {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    st.dense_cols
        .get(&term)
        .map_or(core::ptr::null(), |v| v.as_ptr())
}
unsafe extern "C" fn h_iter_col_mut(iter: *mut BamIter, term: usize) -> *mut u8 {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.col_mutable_calls += 1;
    st.dense_cols
        .get_mut(&term)
        .map_or(core::ptr::null_mut(), |v| v.as_mut_ptr())
}
unsafe extern "C" fn h_iter_col_mut_untracked(iter: *mut BamIter, term: usize) -> *mut u8 {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.col_untracked_calls += 1;
    st.dense_cols
        .get_mut(&term)
        .map_or(core::ptr::null_mut(), |v| v.as_mut_ptr())
}
unsafe extern "C" fn h_iter_mark_changed_range(
    iter: *mut BamIter,
    term: usize,
    start: usize,
    count: usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.mark_changed_calls.push((term, start, count));
    let call_idx = st.mark_changed_calls.len() - 1;
    st.mark_range_status_seq
        .get(call_idx)
        .copied()
        .unwrap_or(FfiResult::OK)
}
unsafe extern "C" fn h_iter_mark_changed_ranges(
    iter: *mut BamIter,
    term: usize,
    ranges_ptr: *const BamRowRange,
    count: usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let ranges = if count > 0 {
        // SAFETY: Valid slice of `count` ranges.
        unsafe { core::slice::from_raw_parts(ranges_ptr, count) }.to_vec()
    } else {
        Vec::new()
    };
    st.mark_ranges_calls.push((term, ranges));
    let call_idx = st.mark_ranges_calls.len() - 1;
    st.mark_ranges_status_seq
        .get(call_idx)
        .copied()
        .unwrap_or(FfiResult::OK)
}
unsafe extern "C" fn h_iter_col_rows_ro(iter: *mut BamIter, term: usize) -> *const *const u8 {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    st.sparse_ro_ptrs
        .get(&term)
        .map_or(core::ptr::null(), |v| v.as_ptr())
}
unsafe extern "C" fn h_iter_col_rows_mut(iter: *mut BamIter, term: usize) -> *const *mut u8 {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    st.sparse_rw_ptrs
        .get(&term)
        .map_or(core::ptr::null(), |v| v.as_ptr())
}
unsafe extern "C" fn h_iter_col_rows_mut_untracked(
    iter: *mut BamIter,
    term: usize,
) -> *const *mut u8 {
    // SAFETY: Forwards to `h_iter_col_rows_mut`.
    unsafe { h_iter_col_rows_mut(iter, term) }
}
unsafe extern "C" fn h_iter_col_info(
    iter: *const BamIter,
    term: usize,
    out_info: *mut BamColumnInfo,
    world: &MockWorld,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let Some(td) = st.terms.get(term) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let Some(comp) = world.comps_by_handle.get(&td.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    // SAFETY: Writable out `BamColumnInfo`.
    unsafe {
        *out_info = BamColumnInfo {
            component: comp.handle,
            mode: td.mode,
            uuid: comp.uuid,
            size: comp.size,
            align: comp.align,
            storage_flags: comp.storage_flags,
            reserved0: 0,
        };
    }
    FfiResult::OK
}
unsafe extern "C" fn h_par_for_each(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    ud: *mut c_void,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.par_calls += 1;
    st.step = 0;
    let Some(f) = chunk_fn else {
        return FfiResult::NULL_POINTER;
    };
    // SAFETY: Synchronous invocation.
    unsafe { f(iter, ud) }
}
unsafe extern "C" fn h_par_for_each_ro(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskFn>,
    ud: *mut c_void,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.par_readonly_calls += 1;
    st.step = 0;
    let Some(f) = chunk_fn else {
        return FfiResult::NULL_POINTER;
    };
    // SAFETY: Synchronous invocation.
    unsafe { f(iter, ud) }
}
unsafe extern "C" fn h_par_for_each_ex(
    iter: *mut BamIter,
    chunk_fn: Option<BamChunkTaskExFn>,
    ud: *mut c_void,
    opts: *const BamParOptions,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.par_ex_calls += 1;
    if !opts.is_null() {
        // SAFETY: Readable options struct.
        st.last_par_options = Some(unsafe { *opts });
    }
    st.step = 0;
    let Some(f) = chunk_fn else {
        return FfiResult::NULL_POINTER;
    };
    // SAFETY: Synchronous invocation.
    unsafe { f(iter, core::ptr::null_mut(), 3, ud) }
}
unsafe extern "C" fn h_msg_read_batch(
    iter: *mut BamIter,
    term: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let Some(bytes) = st.messages.get(&term) else {
        // SAFETY: Writable `out_read`.
        unsafe { *out_read = 0 };
        return FfiResult::OK;
    };
    let elem = *st.msg_elem_size.get(&term).unwrap_or(&4);
    let total = bytes.len() / elem;
    let cursor = *st.msg_cursor.get(&term).unwrap_or(&0);
    let take = (total.saturating_sub(cursor)).min(max_count);
    if take > 0 {
        let start = cursor * elem;
        let len = take * elem;
        // SAFETY: Disjoint mock buffer and caller out buffer.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr().add(start), out_buf, len) };
    }
    st.msg_cursor.insert(term, cursor + take);
    // SAFETY: Writable `out_read`.
    unsafe { *out_read = take };
    FfiResult::OK
}
unsafe extern "C" fn h_msg_read_slice(
    iter: *mut BamIter,
    term: usize,
    out_ptr: *mut *const u8,
    out_count: *mut usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let Some(bytes) = st.messages.get(&term) else {
        // SAFETY: Writable outputs.
        unsafe {
            *out_ptr = core::ptr::null();
            *out_count = 0;
        }
        return FfiResult::OK;
    };
    let elem = *st.msg_elem_size.get(&term).unwrap_or(&4);
    // SAFETY: Writable outputs.
    unsafe {
        *out_ptr = bytes.as_ptr();
        *out_count = bytes.len() / elem;
    }
    FfiResult::OK
}
unsafe extern "C" fn h_msg_send(
    iter: *mut BamIter,
    term: usize,
    data: *const u8,
    len: usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockIterState`.
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    // SAFETY: Readable payload bytes.
    let slice = unsafe { core::slice::from_raw_parts(data, len) };
    st.sent_messages.push((term, slice.to_vec()));
    FfiResult::OK
}
unsafe extern "C" fn h_resolve_named_object(
    ctx: *const BamHostCtx,
    name: *const StrView,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    // SAFETY: `ctx` points to `MockHost` and `name` is valid non-null pointer.
    let (w, sv) = unsafe { ((*ctx.cast::<MockHost>()).world(), &*name) };
    // SAFETY: `sv` borrows valid caller UTF-8 bytes for the duration of this call.
    let Ok(s) = (unsafe { sv.try_as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    match w.named_objects.get(s) {
        Some(&e) => {
            // SAFETY: Writable output.
            unsafe { *out_entity = e };
            FfiResult::OK
        }
        None => FfiResult::NOT_FOUND,
    }
}
unsafe extern "C" fn h_intern_symbol(
    ctx: *const BamHostCtx,
    sym: *const StrView,
    out: *mut BamSymbol,
) -> FfiResult {
    // SAFETY: `ctx` points to `MockHost` and `sym` is valid non-null pointer.
    let (w, sv) = unsafe { (&mut *(*ctx.cast::<MockHost>()).world_ptr(), &*sym) };
    // SAFETY: `sv` borrows valid caller UTF-8 bytes for the duration of this call.
    let Ok(s) = (unsafe { sv.try_as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    if let Some(&id) = w.symbol_map.get(s) {
        // SAFETY: Writable output.
        unsafe { *out = id };
        return FfiResult::OK;
    }
    let id = BamSymbol::from_raw(w.symbols.len() as u32);
    w.symbols.push(s.to_owned());
    w.symbol_map.insert(s.to_owned(), id);
    // SAFETY: Writable output.
    unsafe { *out = id };
    FfiResult::OK
}
unsafe extern "C" fn h_resolve_symbol_str(
    ctx: *const BamHostCtx,
    sym: BamSymbol,
    out_str: *mut StrView,
) -> FfiResult {
    // SAFETY: Non-null test pointers (`ctx` points to `MockHost`).
    let w = unsafe { (*ctx.cast::<MockHost>()).world() };
    let Some(s) = w.symbols.get(sym.to_raw() as usize) else {
        return FfiResult::NOT_FOUND;
    };
    // SAFETY: `w.symbols` string stays pinned for test duration; writable `out_str`.
    unsafe { *out_str = StrView::from_str(s.as_str()) };
    FfiResult::OK
}
unsafe extern "C" fn h_sys_current_ticks(
    sc: *const BamSystemCtx,
    out_last: *mut u32,
    out_this: *mut u32,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    // SAFETY: Valid `MockHost` ref.
    let ticks = unsafe { ctx.host.world() }.ticks;
    // SAFETY: Writable outputs.
    unsafe {
        *out_last = ticks.0;
        *out_this = ticks.1;
    }
    FfiResult::OK
}
unsafe extern "C" fn h_sys_time_info(
    sc: *const BamSystemCtx,
    out_info: *mut BamTimeInfo,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    // SAFETY: Valid `MockHost` ref.
    let info = unsafe { ctx.host.world() }.time_info;
    // SAFETY: Writable output.
    unsafe { *out_info = info };
    FfiResult::OK
}
unsafe extern "C" fn h_sys_relation_targets(
    sc: *const BamSystemCtx,
    e: FfiEntity,
    rel: BamComponent,
    out_ptr: *mut *const FfiEntity,
    out_len: *mut usize,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    // SAFETY: Valid `MockHost` ref.
    let slice = unsafe { ctx.host.world() }
        .relations_targets
        .get(&(e, rel))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    // SAFETY: Writable outputs.
    unsafe {
        *out_ptr = slice.as_ptr();
        *out_len = slice.len();
    }
    FfiResult::OK
}
unsafe extern "C" fn h_sys_relation_sources(
    sc: *const BamSystemCtx,
    t: FfiEntity,
    rel: BamComponent,
    out_ptr: *mut *const FfiEntity,
    out_len: *mut usize,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    // SAFETY: Valid `MockHost` ref.
    let slice = unsafe { ctx.host.world() }
        .relations_sources
        .get(&(t, rel))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    // SAFETY: Writable outputs.
    unsafe {
        *out_ptr = slice.as_ptr();
        *out_len = slice.len();
    }
    FfiResult::OK
}
unsafe extern "C" fn h_sys_relation_edge_data(
    sc: *const BamSystemCtx,
    e: FfiEntity,
    rel: BamComponent,
    t: FfiEntity,
    out_ptr: *mut *const c_void,
    out_size: *mut usize,
) -> FfiResult {
    // SAFETY: `sc` points to `MockSysCtx`.
    let ctx = unsafe { &*sc.cast::<MockSysCtx<'_>>() };
    // SAFETY: Valid `MockHost` ref.
    let Some(bytes) = (unsafe { ctx.host.world() })
        .relation_edges
        .get(&(e, rel, t))
    else {
        return FfiResult::NOT_FOUND;
    };
    // SAFETY: Writable outputs.
    unsafe {
        *out_ptr = bytes.as_ptr().cast::<c_void>();
        *out_size = bytes.len();
    }
    FfiResult::OK
}
unsafe extern "C" fn h_cmd_despawn_all(
    cmds: *mut BamCommands,
    terms: *const BamTermDesc,
    count: usize,
) -> FfiResult {
    // SAFETY: `cmds` points to `MockHost`.
    let w = unsafe { &mut *(*cmds.cast::<MockHost>()).world_ptr() };
    // SAFETY: Readable `count` terms.
    let slice = unsafe { core::slice::from_raw_parts(terms, count) }.to_vec();
    w.commands.push(MockCmdRecord::DespawnAll(slice));
    FfiResult::OK
}
unsafe extern "C" fn h_cmd_despawn_batch(
    cmds: *mut BamCommands,
    ents: *const FfiEntity,
    count: usize,
) -> FfiResult {
    // SAFETY: `cmds` points to `MockHost`.
    let w = unsafe { &mut *(*cmds.cast::<MockHost>()).world_ptr() };
    // SAFETY: Readable `count` entities.
    let slice = unsafe { core::slice::from_raw_parts(ents, count) }.to_vec();
    w.commands.push(MockCmdRecord::DespawnBatch(slice));
    FfiResult::OK
}
unsafe extern "C" fn h_cmd_insert_batch(
    cmds: *mut BamCommands,
    ents: *const FfiEntity,
    ecount: usize,
    comp: BamComponent,
    data: *const u8,
    elem_size: usize,
    flags: u32,
) -> FfiResult {
    // SAFETY: `cmds` points to `MockHost`.
    let w = unsafe { &mut *(*cmds.cast::<MockHost>()).world_ptr() };
    // SAFETY: Readable entity slice.
    let eslice = unsafe { core::slice::from_raw_parts(ents, ecount) }.to_vec();
    let total_bytes = if (flags & BAM_SPAWN_COL_FLAG_SPLAT) != 0 {
        elem_size
    } else {
        ecount * elem_size
    };
    let bytes = if total_bytes > 0 {
        // SAFETY: Readable payload slice.
        unsafe { core::slice::from_raw_parts(data, total_bytes) }.to_vec()
    } else {
        Vec::new()
    };
    w.commands.push(MockCmdRecord::InsertBatch {
        entities: eslice,
        component: comp,
        elem_size,
        flags,
        bytes,
    });
    FfiResult::OK
}
unsafe extern "C" fn h_cmd_remove_batch(
    cmds: *mut BamCommands,
    ents: *const FfiEntity,
    ecount: usize,
    comp: BamComponent,
) -> FfiResult {
    // SAFETY: `cmds` points to `MockHost`.
    let w = unsafe { &mut *(*cmds.cast::<MockHost>()).world_ptr() };
    // SAFETY: Readable entity slice.
    let eslice = unsafe { core::slice::from_raw_parts(ents, ecount) }.to_vec();
    w.commands.push(MockCmdRecord::RemoveBatch {
        entities: eslice,
        component: comp,
    });
    FfiResult::OK
}
unsafe extern "C" fn h_cmd_spawn_batch_ex(
    cmds: *mut BamCommands,
    cols: *const BamSpawnColumnDesc,
    col_count: usize,
    ecount: usize,
    out_ents: *mut FfiEntity,
) -> FfiResult {
    // SAFETY: `cmds` points to `MockHost`.
    let w = unsafe { &mut *(*cmds.cast::<MockHost>()).world_ptr() };
    // SAFETY: Readable columns.
    let cslice = unsafe { core::slice::from_raw_parts(cols, col_count) };
    let mut rec_cols = Vec::with_capacity(col_count);
    for col in cslice {
        let len = if col.is_splat() {
            col.elem_size
        } else {
            ecount * col.elem_size
        };
        let bytes = if len > 0 {
            // SAFETY: Readable column buffer.
            unsafe { core::slice::from_raw_parts(col.column_ptr, len) }.to_vec()
        } else {
            Vec::new()
        };
        rec_cols.push((col.component, col.flags, col.elem_size, bytes));
    }
    let mut spawned = Vec::with_capacity(ecount);
    for _ in 0..ecount {
        let e = FfiEntity::from_bits(w.next_entity_id);
        w.next_entity_id += 1;
        spawned.push(e);
    }
    if !out_ents.is_null() && ecount > 0 {
        // SAFETY: Writable output entities buffer.
        unsafe { core::slice::from_raw_parts_mut(out_ents, ecount) }.copy_from_slice(&spawned);
    }
    w.commands.push(MockCmdRecord::SpawnBatchEx {
        cols: rec_cols,
        entity_count: ecount,
        out_entities: spawned,
    });
    FfiResult::OK
}

std::thread_local! {
    static CURRENT_MOCK_WORLD: core::cell::Cell<*const MockHost> = const { core::cell::Cell::new(core::ptr::null()) };
}

unsafe extern "C" fn h_iter_col_info_tls(
    iter: *const BamIter,
    term: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult {
    let hptr = CURRENT_MOCK_WORLD.with(core::cell::Cell::get);
    if hptr.is_null() {
        return FfiResult::INVALID_STATE;
    }
    // SAFETY: `CURRENT_MOCK_WORLD` points to active `MockHost` for the scope of each test.
    unsafe { h_iter_col_info(iter, term, out_info, (*hptr).world()) }
}

struct MockWorldScope;
impl MockWorldScope {
    fn enter(host: &MockHost) -> Self {
        CURRENT_MOCK_WORLD.with(|c| c.set(host.as_ptr()));
        Self
    }
}
impl Drop for MockWorldScope {
    fn drop(&mut self) {
        CURRENT_MOCK_WORLD.with(|c| c.set(core::ptr::null()));
    }
}

unsafe extern "C" fn stub_col_ticks(
    _: *const BamIter,
    _: usize,
    _: *mut *const u32,
    _: *mut *const u32,
    _: *mut u32,
    _: *mut u32,
) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_res_ptr(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_res_mut_ptr(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_cmd_spawn_empty(_: *mut BamCommands, _: *mut FfiEntity) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_cmd_spawn(
    _: *mut BamCommands,
    _: *const BamComponent,
    _: *const *const u8,
    _: usize,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_cmd_despawn(_: *mut BamCommands, _: FfiEntity) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_cmd_insert(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_cmd_insert_default(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::UNSUPPORTED
}
unsafe extern "C" fn stub_cmd_remove(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::UNSUPPORTED
}

fn make_v04_mock_api() -> BamHostApi {
    BamHostApi {
        struct_size: size_of::<BamHostApi>() as u32,
        abi_version: ABI_VERSION,
        get_proc_address: Some(h_get_proc_address),
        last_error: Some(h_last_error),
        load_host_ctx: Some(h_load_host_ctx),
        register_component: Some(h_register_component),
        resolve_component: Some(h_resolve_component),
        register_system: Some(h_register_system),
        sys_host_ctx: Some(h_sys_host_ctx),
        sys_query: Some(h_sys_query),
        sys_commands: Some(h_sys_commands),
        iter_next_chunk: Some(h_iter_next_chunk),
        iter_reset: Some(h_iter_reset),
        iter_entities: Some(h_iter_entities),
        iter_column_readonly: Some(h_iter_col_ro),
        iter_column_mutable: Some(h_iter_col_mut),
        iter_column_mutable_untracked: Some(h_iter_col_mut_untracked),
        iter_mark_changed_range: Some(h_iter_mark_changed_range),
        iter_column_rows_readonly: Some(h_iter_col_rows_ro),
        iter_column_rows_mutable: Some(h_iter_col_rows_mut),
        iter_column_info: Some(h_iter_col_info_tls),
        res_ptr: Some(stub_res_ptr),
        res_mut_ptr: Some(stub_res_mut_ptr),
        cmd_spawn_empty: Some(stub_cmd_spawn_empty),
        cmd_spawn: Some(stub_cmd_spawn),
        cmd_despawn: Some(stub_cmd_despawn),
        cmd_insert: Some(stub_cmd_insert),
        cmd_insert_default: Some(stub_cmd_insert_default),
        cmd_remove: Some(stub_cmd_remove),
        par_for_each_chunk: Some(h_par_for_each),
        sys_entity_get: None,
        sys_entity_get_mut: None,
        msg_read_batch: Some(h_msg_read_batch),
        msg_send: Some(h_msg_send),
        msg_write_batch: None,
        res_ticks: None,
        sys_res_ptr: None,
        sys_res_mut_ptr: None,
        register_observer: Some(h_register_observer),
        cmd_trigger: None,
        cmd_spawn_persistent: None,
        cmd_spawn_batch: None,
        cmd_despawn_all: Some(h_cmd_despawn_all),
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
        component_layout_info: Some(h_component_layout_info),
        register_relation: None,
        intern_symbol: Some(h_intern_symbol),
        bind_named_object: None,
        iter_count: None,
        iter_up_cascade: None,
        par_for_each_chunk_readonly: Some(h_par_for_each_ro),
        register_system_handle: None,
        configure_schedule_set: None,
        mount_system_in_schedule: None,
        cmd_checkpoint: None,
        cmd_rollback: None,
        cmd_insert_relation: None,
        cmd_remove_relation: None,
        migrate_component_column: None,
        iter_next_chunk_batch: Some(h_iter_next_chunk_batch),
        iter_column_ticks: Some(stub_col_ticks),
        cmd_clone_entity: None,
        cmd_insert_resource: None,
        cmd_remove_resource: None,
        register_resource: Some(h_register_resource),
        register_message: Some(h_register_message),
        resolve_named_object: Some(h_resolve_named_object),
        resolve_symbol_str: Some(h_resolve_symbol_str),
        iter_column_rows_mutable_untracked: Some(h_iter_col_rows_mut_untracked),
        iter_mark_changed_ranges: Some(h_iter_mark_changed_ranges),
        sys_current_ticks: Some(h_sys_current_ticks),
        sys_time_info: Some(h_sys_time_info),
        par_for_each_chunk_ex: Some(h_par_for_each_ex),
        sys_relation_targets: Some(h_sys_relation_targets),
        sys_relation_sources: Some(h_sys_relation_sources),
        sys_relation_edge_data: Some(h_sys_relation_edge_data),
        cmd_despawn_batch: Some(h_cmd_despawn_batch),
        cmd_insert_batch: Some(h_cmd_insert_batch),
        cmd_remove_batch: Some(h_cmd_remove_batch),
        msg_read_slice: Some(h_msg_read_slice),
        cmd_spawn_batch_ex: Some(h_cmd_spawn_batch_ex),
    }
}

fn pod_slice_bytes<T: AbiPod>(items: &[T]) -> AlignedBytes {
    AlignedBytes::from_pod(items)
}

fn init_with_app<F>(
    api: &BamHostApi,
    host: &MockHost,
    name: &'static CStr,
    f: F,
) -> *mut BamInstance
where
    F: FnOnce(&mut GuestApp<'_>) -> Result<(), FfiResult>,
{
    let mut inst_ptr: *mut BamInstance = core::ptr::null_mut();
    let load_ctx = host.as_ptr().cast::<BamLoadCtx>().cast_mut();
    let manifest = BamManifest::new(name.as_ptr(), c"0.4.0".as_ptr());
    // SAFETY: Valid mock API/world pointers pinned for duration of call.
    let res = unsafe {
        init_guest_instance(
            core::ptr::from_ref(api),
            load_ctx,
            &raw mut inst_ptr,
            &manifest,
            |inst, lctx| {
                build_guest_fn(inst, lctx, |app| match f(app) {
                    Ok(()) => FfiResult::OK,
                    Err(e) => e,
                })
            },
        )
    };
    assert_eq!(res, FfiResult::OK);
    inst_ptr
}

#[test]
fn untracked_mut_and_selective_mut_coalescing_behavior() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);

    let ents: Vec<FfiEntity> = (1..=5).map(FfiEntity::from_bits).collect();
    let initial_pos = vec![
        Pos { x: 1.0, y: 0.0 },
        Pos { x: 2.0, y: 0.0 },
        Pos { x: 3.0, y: 0.0 },
        Pos { x: 4.0, y: 0.0 },
        Pos { x: 5.0, y: 0.0 },
    ];

    world.iters.push(Box::pin(MockIterState {
        entities: ents.clone(),
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&initial_pos))]),
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: ents.clone(),
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&initial_pos))]),
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: ents,
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&initial_pos))]),
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let inst_ptr = init_with_app(&api, &host, c"v04_test", |app| {
        let st = app
            .system(c"untracked_and_selective")
            .query(|q| q.write::<Pos>())
            .query(|q| q.write::<Pos>())
            .query(|q| q.write::<Pos>())
            .build(|ctx| {
                // 0: UntrackedMut<Pos> must use batch mask 0 and never mark changed.
                let mut q0 = ctx.query_typed::<UntrackedMut<Pos>>(0).unwrap();
                q0.for_each(|pos| {
                    pos[0].y += 10.0;
                });

                // 1: SelectiveMut<Pos> reads row 2 cleanly and mutates rows 0, 1, 3, 4 -> [(0,2), (3,2)].
                let mut q1 = ctx.query_typed::<SelectiveMut<Pos>>(1).unwrap();
                q1.for_each(|mut sel| {
                    assert_eq!(sel.len(), 5);
                    assert_eq!(sel.get(2).unwrap().x, 3.0);
                    sel.get_mut(0).unwrap().y += 1.0;
                    sel.get_mut(1).unwrap().y += 1.0;
                    sel.get_mut(3).unwrap().y += 1.0;
                    sel.get_mut(4).unwrap().y += 1.0;
                });

                // 2: Raw Chunk::columns_dispenser().next_write_untracked hits slot 77 directly.
                let q2 = ctx.query(2).unwrap();
                if let Some(mut chunk) = q2.next_chunk_view() {
                    let mut disp = chunk.columns_dispenser();
                    let raw_pos = disp.next_write_untracked::<Pos>(None).unwrap();
                    raw_pos[0].x += 5.0;
                }
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });
    // SAFETY: `host.world()` has no conflicting mutable borrow after `init_with_app`.
    let (cb, ud) = unsafe {
        let w = host.world();
        assert_eq!(w.registered_systems.len(), 1);
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1, 2],
    };
    // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
    let status = unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) };
    assert_eq!(status, FfiResult::OK);

    // SAFETY: `cb` returned; reading post-tick state from `MockHost`.
    let w = unsafe { host.world() };
    assert_eq!(w.iters[0].last_batch_mask, Some(0));
    assert_eq!(w.iters[0].col_mutable_calls, 0);
    assert!(w.iters[0].mark_changed_calls.is_empty());
    assert!(w.iters[0].mark_ranges_calls.is_empty());

    assert_eq!(w.iters[1].last_batch_mask, Some(0));
    assert_eq!(w.iters[1].col_mutable_calls, 0);
    assert_eq!(w.iters[1].mark_ranges_calls.len(), 1);
    assert_eq!(
        w.iters[1].mark_ranges_calls[0],
        (0, vec![BamRowRange::new(0, 2), BamRowRange::new(3, 2)])
    );

    assert_eq!(w.iters[2].col_untracked_calls, 1);
    assert_eq!(w.iters[2].col_mutable_calls, 0);
    assert!(w.iters[2].mark_changed_calls.is_empty());

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}

#[test]
fn sparse_query_terms_dispenser_component_rows_and_next_chunk_batch() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);
    let tag_h = world.alloc_static_comp::<SparseTag>(0);
    let vel_h = world.alloc_static_comp::<SparseVel>(0);

    let ents = vec![FfiEntity::from_bits(11), FfiEntity::from_bits(22)];
    let pos_data = vec![Pos { x: 1.0, y: 2.0 }, Pos { x: 3.0, y: 4.0 }];
    let tags = [SparseTag { id: 7 }, SparseTag { id: 9 }];
    let mut vels = [SparseVel { dx: 0.5 }, SparseVel { dx: 1.5 }];
    let [v0, v1] = &mut vels;
    let tag_ptrs = vec![
        core::ptr::from_ref(&tags[0]).cast::<u8>(),
        core::ptr::from_ref(&tags[1]).cast::<u8>(),
    ];
    let vel_ptrs = vec![
        core::ptr::from_mut(v0).cast::<u8>(),
        core::ptr::from_mut(v1).cast::<u8>(),
    ];

    world.iters.push(Box::pin(MockIterState {
        entities: ents,
        terms: vec![
            BamTermDesc::new(pos_h, AccessMode::WRITE),
            BamTermDesc::new(tag_h, AccessMode::READ),
            BamTermDesc::new(vel_h, AccessMode::WRITE),
        ],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_data))]),
        sparse_ro_ptrs: HashMap::from([(1usize, tag_ptrs)]),
        sparse_rw_ptrs: HashMap::from([(2usize, vel_ptrs)]),
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let inst_ptr = init_with_app(&api, &host, c"v04_sparse", |app| {
        let st = app
            .system(c"sparse_and_chunk_api")
            .query(|q| q.write::<Pos>().read::<SparseTag>().write::<SparseVel>())
            .build(|ctx| {
                // 1. TypedChunkIter with Sparse<&T> and Sparse<&mut T>
                let mut tq = ctx
                    .query_typed::<(&mut Pos, Sparse<&SparseTag>, Sparse<&mut SparseVel>)>(0)
                    .unwrap();
                tq.for_each(|(p, t, mut v)| {
                    assert_eq!(p.len(), 2);
                    assert_eq!(t.get(0).unwrap().id, 7);
                    assert_eq!(t.get(1).unwrap().id, 9);
                    v.get_mut(0).unwrap().dx += 10.0;
                    p[1].x += v.get(1).unwrap().dx;
                });

                // 2. Chunk::component_rows(&self) + Chunk::columns_dispenser() + next_chunk_batch + entities_and_component_mut
                let raw_q = ctx.query(0).unwrap();
                assert_eq!(raw_q.reset(), FfiResult::OK);
                let batch_len = raw_q.next_chunk_batch(1, true, 0b1);
                assert_eq!(batch_len, Some(2));

                assert_eq!(raw_q.reset(), FfiResult::OK);
                while let Some(mut chunk) = raw_q.next_chunk_view() {
                    // Works on `&Chunk`!
                    let ro_sparse = Chunk::component_rows::<SparseTag>(&chunk, 1).unwrap();
                    assert_eq!(ro_sparse.get(0).unwrap().id, 7);

                    // Safe multi-column dispenser:
                    {
                        let mut disp = chunk.columns_dispenser();
                        let _pos_mut = disp.next_write::<Pos>(None).unwrap();
                        let disp_tag = disp
                            .next_sparse_read::<SparseTag>(AccessMode::READ, None)
                            .unwrap();
                        assert_eq!(disp_tag.get(1).unwrap().id, 9);
                    }

                    // Chunk entities_and_component_mut:
                    let (ents, pos_mut) = chunk.entities_and_component_mut::<Pos>(0).unwrap();
                    assert_eq!(ents.len(), 2);
                    pos_mut[0].y += 100.0;
                }
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });

    // SAFETY: `cb` lookup on `MockHost` after `init_with_app`.
    let (cb, ud) = unsafe {
        let w = host.world();
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0],
    };
    assert_eq!(
        // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) },
        FfiResult::OK
    );
    assert!((vels[0].dx - 10.5).abs() < 1e-6);
    // SAFETY: `cb` returned; reading `MockHost`.
    assert!(unsafe { host.world() }.iters[0].batch_calls >= 1);

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}

#[test]
fn twenty_term_query_tuple_and_builder_flags_and_par_options() {
    let mut world = MockWorld::default();
    let h0 = world.alloc_static_comp::<C0>(0);
    let _ = world.alloc_static_comp::<C1>(0);
    let _ = world.alloc_static_comp::<C2>(0);
    let _ = world.alloc_static_comp::<C3>(0);
    let _ = world.alloc_static_comp::<C4>(0);
    let _ = world.alloc_static_comp::<C5>(0);
    let _ = world.alloc_static_comp::<C6>(0);
    let _ = world.alloc_static_comp::<C7>(0);
    let _ = world.alloc_static_comp::<C8>(0);
    let _ = world.alloc_static_comp::<C9>(0);
    let _ = world.alloc_static_comp::<C10>(0);
    let _ = world.alloc_static_comp::<C11>(0);
    let _ = world.alloc_static_comp::<C12>(0);
    let _ = world.alloc_static_comp::<C13>(0);
    let _ = world.alloc_static_comp::<C14>(0);
    let _ = world.alloc_static_comp::<C15>(0);
    let _ = world.alloc_static_comp::<C16>(0);
    let _ = world.alloc_static_comp::<C17>(0);
    let _ = world.alloc_static_comp::<C18>(0);
    let pos_h = world.alloc_static_comp::<Pos>(0);

    let ents = vec![FfiEntity::from_bits(77)];
    let mut dense_cols = HashMap::new();
    let mut terms = Vec::new();
    for i in 0..19 {
        terms.push(BamTermDesc::new(
            BamComponent::from_raw(1 + i as u32),
            AccessMode::READ,
        ));
        dense_cols.insert(i, pod_slice_bytes(&[C0(10 + i as u32)]));
    }

    world.iters.push(Box::pin(MockIterState {
        entities: ents.clone(),
        terms,
        dense_cols,
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: ents,
        terms: vec![BamTermDesc::new(pos_h, AccessMode::READ)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&[Pos { x: 4.0, y: 5.0 }]))]),
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let par_sum = std::sync::Arc::new(AtomicU32::new(0));
    let par_sum_sys = std::sync::Arc::clone(&par_sum);

    let inst_ptr = init_with_app(&api, &host, c"v04_20term", |app| {
        let st1 = app
            .system(c"twenty_terms_and_par")
            .query(|q| {
                q.read::<C0>()
                    .read::<C1>()
                    .read::<C2>()
                    .read::<C3>()
                    .read::<C4>()
                    .read::<C5>()
                    .read::<C6>()
                    .read::<C7>()
                    .read::<C8>()
                    .read::<C9>()
                    .read::<C10>()
                    .read::<C11>()
                    .read::<C12>()
                    .read::<C13>()
                    .read::<C14>()
                    .read::<C15>()
                    .read::<C16>()
                    .read::<C17>()
                    .read::<C18>()
            })
            .query(|q| q.read::<Pos>())
            .query(|q| {
                q.term_with_flags(h0, AccessMode::READ, BAM_TERM_FLAG_CHANGED)
                    .changed_write::<Pos>()
                    .added_write::<C1>()
                    .with_changed::<C2>()
            })
            .build(move |ctx| {
                type Q20<'w> = (
                    FfiEntity,
                    &'w C0,
                    &'w C1,
                    &'w C2,
                    &'w C3,
                    &'w C4,
                    &'w C5,
                    &'w C6,
                    &'w C7,
                    &'w C8,
                    &'w C9,
                    &'w C10,
                    &'w C11,
                    &'w C12,
                    &'w C13,
                    &'w C14,
                    &'w C15,
                    &'w C16,
                    &'w C17,
                    &'w C18,
                );
                let mut q0 = ctx.query_typed::<Q20<'_>>(0).unwrap();
                let mut seen = 0u32;
                q0.for_each(|(e, c0, .., c18)| {
                    assert_eq!(e[0], FfiEntity::from_bits(77));
                    assert_eq!(c0[0].0, 10);
                    assert_eq!(c18[0].0, 28);
                    seen += 1;
                });
                assert_eq!(seen, 1);

                let mut q1 = ctx.query_typed::<&Pos>(1).unwrap();
                let par_for_ro = std::sync::Arc::clone(&par_sum_sys);
                assert_eq!(
                    q1.par_for_each_chunk_readonly(|positions| {
                        par_for_ro.fetch_add(positions[0].x as u32, Ordering::Relaxed);
                    }),
                    FfiResult::OK
                );
                let opts = BamParOptions::new()
                    .with_chunk_size(16)
                    .with_max_partitions(2);
                let par_for_opts = std::sync::Arc::clone(&par_sum_sys);
                assert_eq!(
                    q1.par_for_each_with_options(&opts, |positions, _cmds, partition_idx| {
                        assert_eq!(partition_idx, 3);
                        par_for_opts.fetch_add(positions[0].y as u32, Ordering::Relaxed);
                    }),
                    FfiResult::OK
                );
            });
        if !st1.is_ok() {
            return Err(st1);
        }
        Ok(())
    });

    // SAFETY: `init_with_app` returned; inspecting `MockHost`.
    let (cb, ud) = unsafe {
        let w = host.world();
        let qb_terms = &w.registered_systems[0].3[2];
        assert_eq!(qb_terms[0].flags, BAM_TERM_FLAG_CHANGED);
        assert_eq!(qb_terms[1].mode, AccessMode::WRITE);
        assert_eq!(qb_terms[1].flags, BAM_TERM_FLAG_CHANGED);
        assert_eq!(qb_terms[2].mode, AccessMode::WRITE);
        assert_eq!(qb_terms[2].flags, BAM_TERM_FLAG_ADDED);
        assert_eq!(qb_terms[3].mode, AccessMode::FILTER_ONLY);
        assert_eq!(qb_terms[3].flags, BAM_TERM_FLAG_CHANGED);
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1],
    };
    assert_eq!(
        // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) },
        FfiResult::OK
    );
    assert_eq!(par_sum.load(Ordering::Relaxed), 9);
    // SAFETY: `cb` returned; reading `MockHost`.
    let w = unsafe { host.world() };
    assert_eq!(w.iters[1].par_readonly_calls, 1);
    assert_eq!(w.iters[1].par_ex_calls, 1);
    assert!(w.iters[1].batch_calls >= 2);
    assert_eq!(w.iters[1].last_par_options.unwrap().chunk_size, 16);

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}

#[test]
fn context_split_messages_by_handle_ticks_time_relations_commands_and_app_caches() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);
    let msg_h = world.alloc_static_comp::<MsgItem>(BAM_STORAGE_FLAG_MESSAGE);
    let rel_h = world.alloc_static_comp::<RelEdge>(0);
    let e_a = FfiEntity::from_bits(501);
    let e_b = FfiEntity::from_bits(502);
    world.named_objects.insert("spawn_point".to_owned(), e_a);
    world.relations_targets.insert((e_a, rel_h), vec![e_b]);
    world.relations_sources.insert((e_b, rel_h), vec![e_a]);
    world.relation_edges.insert(
        (e_a, rel_h, e_b),
        AlignedBytes::from_pod(&[RelEdge { weight: 3.5 }]),
    );
    world.time_info.delta_secs = 0.016;
    world.time_info.elapsed_secs = 12.5;

    let msgs = vec![MsgItem { seq: 101 }, MsgItem { seq: 202 }];
    world.iters.push(Box::pin(MockIterState {
        entities: vec![e_a],
        terms: vec![BamTermDesc::new(pos_h, AccessMode::READ)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&[Pos { x: 1.0, y: 2.0 }]))]),
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: vec![e_a],
        terms: vec![BamTermDesc::new(msg_h, AccessMode::MESSAGE_READ)],
        messages: HashMap::from([(0usize, pod_slice_bytes(&msgs))]),
        msg_elem_size: HashMap::from([(0usize, size_of::<MsgItem>())]),
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let mut cached_msg_handle: Option<TypedComponent<MsgItem>> = None;

    let inst_ptr = init_with_app(&api, &host, c"v04_ctx", |app| {
        let res_h = app.register_resource_typed::<SimConfig>()?;
        assert!(res_h.raw().is_valid());
        let msg_reg_h = app.register_message_typed::<TickEvent>()?;
        assert!(msg_reg_h.raw().is_valid());

        // Cache hits: calling cached_component::<Pos>() twice only hits host resolve_component once.
        let c1 = app.cached_component::<Pos>()?;
        let c2 = app.cached_component::<Pos>()?;
        assert_eq!(c1.raw(), c2.raw());
        cached_msg_handle = Some(app.cached_component::<MsgItem>()?);

        // Verify Pos's SCHEMA_FINGERPRINT + BAM_FIELDS were propagated on register_component:
        let pos_reg = app.register_component::<Pos>()?;
        assert_eq!(pos_reg, pos_h);

        let st = app
            .system(c"split_and_batch_cmds")
            .query(|q| q.read::<Pos>())
            .message_read::<MsgItem>()
            .build(move |ctx| {
                let split = ctx.split();
                assert!(split.queries[0].as_mut().unwrap().next_chunk().is_some());
                assert_eq!(split.sys.current_ticks(), Ok((40, 42)));
                let tinfo = split.sys.time_info().unwrap();
                assert!((tinfo.delta_secs - 0.016).abs() < 1e-6);
                assert_eq!(split.sys.resolve_named_object("spawn_point"), Ok(e_a));

                let sym = split.sys.intern_symbol("hero.jump").unwrap();
                let sym_str = split.sys.resolve_symbol_str(sym).unwrap();
                assert_eq!(sym_str, "hero.jump");

                assert_eq!(split.sys.relation_targets(e_a, rel_h), Ok(&[e_b][..]));
                assert_eq!(split.sys.relation_sources(e_b, rel_h), Ok(&[e_a][..]));
                assert_eq!(
                    split.sys.relation_edge_data::<RelEdge>(e_a, rel_h, e_b),
                    Ok(&RelEdge { weight: 3.5 })
                );

                split
                    .commands
                    .despawn_all_with(|q| q.with::<Pos>())
                    .unwrap();
                split.commands.despawn_batch(&[e_a, e_b]).unwrap();
                split
                    .commands
                    .insert_batch_splat_typed(&[e_a, e_b], c1, &Pos { x: 9.0, y: 9.0 })
                    .unwrap();
                split.commands.remove_batch(&[e_a], c1.raw()).unwrap();
                let splat_col = SpawnBatchColumn::splat_typed(c1, &Pos { x: 2.0, y: 3.0 });
                let mut spawned = [FfiEntity::NULL; 2];
                split
                    .commands
                    .spawn_batch_ex(&[splat_col], 2, Some(&mut spawned))
                    .unwrap();
                assert!(spawned[0].is_valid() && spawned[1].is_valid());

                let mh = cached_msg_handle.unwrap();
                let out_msgs = ctx.read_messages_by_handle::<MsgItem>(mh.raw()).unwrap();
                assert_eq!(out_msgs, vec![MsgItem { seq: 101 }, MsgItem { seq: 202 }]);
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });

    // SAFETY: `init_with_app` returned; inspecting `MockHost`.
    let (cb, ud) = unsafe {
        let w = host.world();
        assert_eq!(w.registered_resources.len(), 1);
        assert_eq!(w.registered_messages.len(), 1);
        assert_eq!(w.resolve_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            w.comps_by_handle.get(&pos_h).unwrap().schema_fingerprint,
            Pos::SCHEMA_FINGERPRINT
        );
        assert_eq!(w.comps_by_handle.get(&pos_h).unwrap().field_count, 2);
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1],
    };
    assert_eq!(
        // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) },
        FfiResult::OK
    );

    // SAFETY: `cb` returned; inspecting `MockHost`.
    let w = unsafe { host.world() };
    assert_eq!(w.commands.len(), 5);
    assert!(matches!(w.commands[0], MockCmdRecord::DespawnAll(_)));
    assert_eq!(w.commands[1], MockCmdRecord::DespawnBatch(vec![e_a, e_b]));
    assert!(matches!(
        &w.commands[2],
        MockCmdRecord::InsertBatch { flags, elem_size: 8, .. } if *flags == BAM_SPAWN_COL_FLAG_SPLAT
    ));
    assert_eq!(
        w.commands[3],
        MockCmdRecord::RemoveBatch {
            entities: vec![e_a],
            component: pos_h,
        }
    );
    assert!(matches!(
        &w.commands[4],
        MockCmdRecord::SpawnBatchEx {
            entity_count: 2,
            ..
        }
    ));

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}

#[test]
fn observer_mut_with_ctx_run_if_and_ecs_run_condition_combinators() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);
    let tag_h = world.alloc_static_comp::<SparseTag>(0);
    let _ev_h = world.alloc_static_comp::<TickEvent>(0);
    let _cfg_h = world.alloc_static_comp::<SimConfig>(BAM_STORAGE_FLAG_RESOURCE);
    let _msg_h = world.alloc_static_comp::<MsgItem>(BAM_STORAGE_FLAG_MESSAGE);

    world.iters.push(Box::pin(MockIterState {
        entities: vec![FfiEntity::from_bits(1)],
        terms: vec![BamTermDesc::new(pos_h, AccessMode::READ)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&[Pos { x: 1.0, y: 0.0 }]))]),
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: Vec::new(),
        terms: vec![BamTermDesc::new(tag_h, AccessMode::READ)],
        ..MockIterState::default()
    }));
    world.iters.push(Box::pin(MockIterState {
        entities: vec![FfiEntity::from_bits(1)],
        terms: vec![BamTermDesc::new(pos_h, AccessMode::READ)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&[Pos { x: 1.0, y: 0.0 }]))]),
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let obs_ran = std::sync::Arc::new(AtomicBool::new(false));
    let obs_ran_cb = std::sync::Arc::clone(&obs_ran);

    let inst_ptr = init_with_app(&api, &host, c"v04_obs", |app| {
        let cond_a = resource_exists::<SimConfig>();
        let cond_b = on_message::<MsgItem>();
        let _combined = cond_a.or(cond_b);
        let cond_and = resource_exists::<SimConfig>().and(on_message::<MsgItem>());
        let cond_xor = resource_exists::<SimConfig>().xor(on_message::<MsgItem>());
        let _ = (cond_and, cond_xor);

        app.observe_event::<TickEvent>("v04_tick_obs")
            .query(|q| q.read::<Pos>())
            .run_if_with(
                |q| q.read::<Pos>(),
                |rctx| rctx.query().and_then(|it| it.next_chunk()).is_some(),
            )
            .build_mut_with_ctx(move |_tgt, ev, prop, ctx| {
                ev.delta += 5;
                *prop = true;
                let mut q = ctx.query_typed::<&Pos>(0).unwrap();
                q.for_each(|p| {
                    assert_eq!(p[0].x, 1.0);
                });
                obs_ran_cb.store(true, Ordering::SeqCst);
            })?;
        Ok(())
    });

    // SAFETY: `init_with_app` returned; inspecting `MockHost`.
    let obs_desc = unsafe {
        let w = host.world();
        assert_eq!(w.registered_observers.len(), 1);
        w.registered_observers[0].desc
    };
    let ctx_mut_fn: BamObserverCtxMutFn = obs_desc.ctx_mut_callback.expect("ctx_mut_callback set");

    let mut ev = TickEvent { delta: 10 };
    let mut prop = FfiBool::FALSE;
    // SAFETY: Exclusive short-lived mutation before `MockSysCtx` borrows `&host`.
    unsafe {
        let w = &mut *host.world_ptr();
        w.iters[0].step = 0;
        w.iters[2].step = 0;
    }
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 2],
    };
    // SAFETY: Valid test observer pointers and initialized `TickEvent` / `FfiBool` storage.
    let res = unsafe {
        ctx_mut_fn(
            FfiEntity::from_bits(1),
            (&raw mut ev).cast::<u8>(),
            &raw mut prop,
            core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(),
            obs_desc.user_data,
        )
    };
    assert_eq!(res, FfiResult::OK);
    assert_eq!(ev.delta, 15);
    assert_eq!(prop, FfiBool::TRUE);
    assert!(obs_ran.load(Ordering::SeqCst));

    obs_ran.store(false, Ordering::SeqCst);
    // SAFETY: Exclusive short-lived mutation before `MockSysCtx` borrows `&host`.
    unsafe {
        let w = &mut *host.world_ptr();
        w.iters[0].step = 0;
        w.iters[1].step = 0;
    }
    let mut sys_ctx_skip = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1],
    };
    // SAFETY: Valid test observer pointers and initialized `TickEvent` / `FfiBool` storage.
    let res2 = unsafe {
        ctx_mut_fn(
            FfiEntity::from_bits(1),
            (&raw mut ev).cast::<u8>(),
            &raw mut prop,
            core::ptr::from_mut(&mut sys_ctx_skip).cast::<BamSystemCtx>(),
            obs_desc.user_data,
        )
    };
    assert_eq!(res2, FfiResult::OK);
    assert!(!obs_ran.load(Ordering::SeqCst));
    assert_eq!(ev.delta, 15);

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}

#[derive(Default)]
struct DummyPlugin;
impl GuestPlugin for DummyPlugin {
    const NAME: &'static CStr = c"dummy";
    fn build(&mut self, _app: &mut GuestApp<'_>) -> FfiResult {
        FfiResult::OK
    }
}

#[test]
fn spawn_batch_column_root_reexport_and_plugin_compile() {
    let pos = Pos { x: 1.0, y: 2.0 };
    // SAFETY: Mock component handle for unit signature check.
    let typed = unsafe { TypedComponent::<Pos>::from_raw_unchecked(BamComponent::from_raw(1)) };
    let col = SpawnBatchColumn::splat_typed(typed, &pos);
    assert_eq!(col.flags(), BAM_SPAWN_COL_FLAG_SPLAT);
    let _f1: fn(
        &mut bamabi_guest::raw::GuestInstance,
        bamabi_guest::raw::ValidatedLoadCtx<'_>,
        fn(&mut GuestApp<'_>) -> FfiResult,
    ) -> FfiResult = build_guest_fn;
    let _f2: fn(
        &mut bamabi_guest::raw::GuestInstance,
        bamabi_guest::raw::ValidatedLoadCtx<'_>,
    ) -> FfiResult = build_guest_plugin::<DummyPlugin>;
}

#[test]
fn selective_mut_bounded_inline_batches_over_cap_and_slot15_fallback() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);

    let ents_100: Vec<FfiEntity> = (1..=100).map(FfiEntity::from_bits).collect();
    let pos_100: Vec<Pos> = (0..100)
        .map(|i| Pos {
            x: i as f32,
            y: 0.0,
        })
        .collect();
    let ents_40: Vec<FfiEntity> = (1..=40).map(FfiEntity::from_bits).collect();
    let pos_40: Vec<Pos> = (0..40)
        .map(|i| Pos {
            x: i as f32,
            y: 0.0,
        })
        .collect();

    // Iter 0: 50 non-adjacent dirty runs across 100 rows via Slot 78 -> [16, 16, 16, 2].
    world.iters.push(Box::pin(MockIterState {
        entities: ents_100,
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_100))]),
        ..MockIterState::default()
    }));
    // Iter 1: 20 non-adjacent dirty runs across 40 rows when Slot 78 returns UNSUPPORTED -> Slot 15.
    world.iters.push(Box::pin(MockIterState {
        entities: ents_40.clone(),
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_40))]),
        mark_ranges_status_seq: vec![FfiResult::UNSUPPORTED; 4],
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let inst_ptr = init_with_app(&api, &host, c"v04_sel_inline", |app| {
        let st = app
            .system(c"selective_inline_batches")
            .query(|q| q.write::<Pos>())
            .query(|q| q.write::<Pos>())
            .build(|ctx| {
                let mut q0 = ctx.query_typed::<SelectiveMut<Pos>>(0).unwrap();
                q0.for_each(|mut sel| {
                    assert_eq!(sel.len(), 100);
                    for row in 0..sel.len() {
                        if row % 2 == 0 {
                            sel.get_mut(row).unwrap().y += 1.0;
                        } else {
                            assert_eq!(sel.get(row).unwrap().y, 0.0);
                        }
                    }
                });

                let mut q1 = ctx.query_typed::<SelectiveMut<Pos>>(1).unwrap();
                q1.for_each(|mut sel| {
                    assert_eq!(sel.len(), 40);
                    for row in (0..sel.len()).step_by(2) {
                        sel.get_mut(row).unwrap().y += 2.0;
                    }
                });
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });

    // SAFETY: `host.world()` has no conflicting mutable borrow after `init_with_app`.
    let (cb, ud) = unsafe {
        let w = host.world();
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1],
    };
    // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
    let status = unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) };
    assert_eq!(status, FfiResult::OK);

    let expected_1: Vec<(usize, usize, usize)> = (0..20).map(|i| (0, i * 2, 1)).collect();
    {
        // SAFETY: `cb` returned; reading post-tick state from `MockHost`.
        let w = unsafe { host.world() };
        assert_eq!(w.iters[0].mark_ranges_calls.len(), 4);
        assert!(w.iters[0].mark_changed_calls.is_empty());
        let batch_lens: Vec<usize> = w.iters[0]
            .mark_ranges_calls
            .iter()
            .map(|(term, runs)| {
                assert_eq!(*term, 0);
                runs.len()
            })
            .collect();
        assert_eq!(batch_lens, vec![16, 16, 16, 2]);
        let flattened_0: Vec<BamRowRange> = w.iters[0]
            .mark_ranges_calls
            .iter()
            .flat_map(|(_, runs)| runs.iter().copied())
            .collect();
        let expected_0: Vec<BamRowRange> = (0..50).map(|i| BamRowRange::new(i * 2, 1)).collect();
        assert_eq!(flattened_0, expected_0);

        // Verify mutated vs untouched rows in iter 0 dense buffer.
        let final_pos_0 = w.iters[0].dense_cols[&0].as_pod_slice::<Pos>();
        assert_eq!(final_pos_0.len(), 100);
        for (idx, p) in final_pos_0.iter().enumerate() {
            let expected_y = if idx % 2 == 0 { 1.0 } else { 0.0 };
            assert_eq!(p.y, expected_y);
        }

        // Iter 1 fell back to Slot 15 on both batches ([16, 4]).
        assert_eq!(w.iters[1].mark_ranges_calls.len(), 2);
        assert_eq!(w.iters[1].mark_changed_calls, expected_1);
    }

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };

    // Also verify when Slot 78 (`iter_mark_changed_ranges`) is `None` altogether.
    let mut world_no78 = MockWorld::default();
    let pos_h2 = world_no78.alloc_static_comp::<Pos>(0);
    world_no78.iters.push(Box::pin(MockIterState {
        entities: ents_40,
        terms: vec![BamTermDesc::new(pos_h2, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_40))]),
        ..MockIterState::default()
    }));
    let host2 = MockHost::new(world_no78);
    let _scope2 = MockWorldScope::enter(&host2);
    let mut api_no78 = make_v04_mock_api();
    api_no78.iter_mark_changed_ranges = None;
    let inst_ptr2 = init_with_app(&api_no78, &host2, c"v04_sel_no78", |app| {
        let st = app
            .system(c"selective_slot15_only")
            .query(|q| q.write::<Pos>())
            .build(|ctx| {
                let mut q = ctx.query_typed::<SelectiveMut<Pos>>(0).unwrap();
                q.for_each(|mut sel| {
                    for row in (0..sel.len()).step_by(2) {
                        sel.get_mut(row).unwrap().y += 3.0;
                    }
                });
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });
    // SAFETY: `host2.world()` has no conflicting mutable borrow after `init_with_app`.
    let (cb2, ud2) = unsafe {
        let w = host2.world();
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx2 = MockSysCtx {
        host: &host2,
        iter_indices: vec![0],
    };
    // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
    let status2 = unsafe {
        cb2(
            core::ptr::from_mut(&mut sys_ctx2).cast::<BamSystemCtx>(),
            ud2,
        )
    };
    assert_eq!(status2, FfiResult::OK);
    {
        // SAFETY: `cb2` returned; reading post-tick state from `MockHost`.
        let w2 = unsafe { host2.world() };
        assert!(w2.iters[0].mark_ranges_calls.is_empty());
        assert_eq!(w2.iters[0].mark_changed_calls, expected_1);
    }
    // SAFETY: `inst_ptr2` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr2) };
}

#[test]
fn selective_mut_mid_chunk_flush_error_latching() {
    let mut world = MockWorld::default();
    let pos_h = world.alloc_static_comp::<Pos>(0);

    let ents_80: Vec<FfiEntity> = (1..=80).map(FfiEntity::from_bits).collect();
    let pos_80: Vec<Pos> = (0..80)
        .map(|i| Pos {
            x: i as f32,
            y: 0.0,
        })
        .collect();
    let ents_40: Vec<FfiEntity> = (1..=40).map(FfiEntity::from_bits).collect();
    let pos_40: Vec<Pos> = (0..40)
        .map(|i| Pos {
            x: i as f32,
            y: 0.0,
        })
        .collect();

    // Iter 0: 40 non-adjacent runs -> 3 batches [16, 16, 8]; batch 1 fails with INVALID_ARGUMENT,
    // batch 2 fails with INVALID_STATE (must NOT overwrite first latched error), tail batch 3 OK.
    world.iters.push(Box::pin(MockIterState {
        entities: ents_80,
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_80))]),
        mark_ranges_status_seq: vec![
            FfiResult::INVALID_ARGUMENT,
            FfiResult::INVALID_STATE,
            FfiResult::OK,
        ],
        ..MockIterState::default()
    }));

    // Iter 1: 20 non-adjacent runs -> Slot 78 returns UNSUPPORTED, Slot 15 fails on run #5 of batch 1.
    let mut slot15_statuses = vec![FfiResult::OK; 4];
    slot15_statuses.push(FfiResult::OUT_OF_RANGE);
    world.iters.push(Box::pin(MockIterState {
        entities: ents_40,
        terms: vec![BamTermDesc::new(pos_h, AccessMode::WRITE)],
        dense_cols: HashMap::from([(0usize, pod_slice_bytes(&pos_40))]),
        mark_ranges_status_seq: vec![FfiResult::UNSUPPORTED; 4],
        mark_range_status_seq: slot15_statuses,
        ..MockIterState::default()
    }));

    let host = MockHost::new(world);
    let _scope = MockWorldScope::enter(&host);
    let api = make_v04_mock_api();
    let inst_ptr = init_with_app(&api, &host, c"v04_sel_err", |app| {
        let st = app
            .system(c"selective_error_latching")
            .query(|q| q.write::<Pos>())
            .query(|q| q.write::<Pos>())
            .build(|ctx| {
                let q0 = ctx.query(0).unwrap();
                let mut chunk0 = q0.next_chunk_view().unwrap();
                {
                    let mut disp = chunk0.columns_dispenser();
                    let mut sel = disp.next_selective_write::<Pos>(None).unwrap();
                    for row in (0..sel.len()).step_by(2) {
                        sel.get_mut(row).unwrap().y += 1.0;
                    }
                    assert_eq!(sel.flush(), FfiResult::INVALID_ARGUMENT);
                    assert_eq!(sel.flush(), FfiResult::OK);
                }

                let q1 = ctx.query(1).unwrap();
                let mut chunk1 = q1.next_chunk_view().unwrap();
                {
                    let mut disp = chunk1.columns_dispenser();
                    let mut sel = disp.next_selective_write::<Pos>(None).unwrap();
                    for row in (0..sel.len()).step_by(2) {
                        sel.get_mut(row).unwrap().y += 1.0;
                    }
                    assert_eq!(sel.flush(), FfiResult::OUT_OF_RANGE);
                    assert_eq!(sel.flush(), FfiResult::OK);
                }
            });
        if !st.is_ok() {
            return Err(st);
        }
        Ok(())
    });

    // SAFETY: `host.world()` has no conflicting mutable borrow after `init_with_app`.
    let (cb, ud) = unsafe {
        let w = host.world();
        (w.registered_systems[0].1, w.registered_systems[0].2)
    };
    let mut sys_ctx = MockSysCtx {
        host: &host,
        iter_indices: vec![0, 1],
    };
    // SAFETY: Valid `MockSysCtx` pointer and registered system `user_data`.
    let status = unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) };
    assert_eq!(status, FfiResult::OK);

    {
        // SAFETY: `cb` returned; reading post-tick state from `MockHost`.
        let w = unsafe { host.world() };
        let batch_lens_0: Vec<usize> = w.iters[0]
            .mark_ranges_calls
            .iter()
            .map(|(_, runs)| runs.len())
            .collect();
        assert_eq!(batch_lens_0, vec![16, 16, 8]);
        // Batch 1 stopped at 5th Slot-15 call (`OUT_OF_RANGE`), then tail batch (4 runs) ran -> 5 + 4 = 9 calls.
        assert_eq!(w.iters[1].mark_changed_calls.len(), 9);
    }

    // SAFETY: `inst_ptr` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_ptr) };
}
