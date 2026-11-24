//! Zero-allocation message drain & `has_messages` verification suite using a thread-guarded
//! `#[global_allocator] CountingAlloc`.

use core::ffi::{CStr, c_char, c_void};
use core::mem::{MaybeUninit, align_of, size_of};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;

use bamabi_guest::prelude::*;
use bamabi_guest::raw::{
    BAM_STORAGE_FLAG_MESSAGE, BamHostApi, BamHostCtx, BamInstance, BamIter, BamLoadCtx,
    BamManifest, BamSystemCtx, BamSystemDesc, BamTermDesc, ComponentDesc, Host, SymbolEntry,
    ValidatedIter, init_guest_instance, shutdown_guest_instance,
};
use bamabi_guest::{build_guest_fn, conditions};

/// Global allocator wrapper counting allocations on the tracked test thread when
/// `TRACKING_ACTIVE` is enabled.
struct CountingAlloc;

static TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static TEST_MUTEX: Mutex<()> = Mutex::new(());

std::thread_local! {
    static TRACKED_THREAD: Cell<bool> = const { Cell::new(false) };
}

#[global_allocator]
static GLOBAL_ALLOC: CountingAlloc = CountingAlloc;

#[inline]
fn record_alloc(size: usize) {
    if TRACKING_ACTIVE.load(Ordering::Acquire)
        && TRACKED_THREAD.try_with(Cell::get).unwrap_or(false)
    {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
    }
}

// SAFETY: Every allocator method delegates straight to `std::alloc::System` with matching layout.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_alloc(layout.size());
        // SAFETY: Same `layout` forwarded to `System::alloc`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_alloc(layout.size());
        // SAFETY: Same `layout` forwarded to `System::alloc_zeroed`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_alloc(new_size);
        // SAFETY: `ptr` was allocated by `System` with `layout`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Runs `f` while counting heap allocations performed on the current thread (with `TEST_MUTEX`
/// held so concurrent test threads cannot interfere).
fn measure_allocs<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    let _lock = TEST_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    TRACKED_THREAD.with(|c| c.set(true));
    ALLOC_COUNT.store(0, Ordering::Release);
    ALLOC_BYTES.store(0, Ordering::Release);
    TRACKING_ACTIVE.store(true, Ordering::Release);

    struct ResetGuard;
    impl Drop for ResetGuard {
        fn drop(&mut self) {
            TRACKING_ACTIVE.store(false, Ordering::Release);
            TRACKED_THREAD.with(|c| c.set(false));
        }
    }
    let guard = ResetGuard;
    let res = f();
    drop(guard);

    (
        res,
        ALLOC_COUNT.load(Ordering::Acquire),
        ALLOC_BYTES.load(Ordering::Acquire),
    )
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Msg {
    seq: u64,
    tag: u32,
    pad: u32,
}

// SAFETY: `Msg` is `#[repr(C)]` of `u64`/`u32` fields with no drop glue or uninit padding.
unsafe impl RawInsertable for Msg {}
// SAFETY: Fixed deterministic C layout matching cross-ABI POD requirements.
unsafe impl AbiPod for Msg {}
impl GuestComponent for Msg {
    const NAME: &'static CStr = c"zero_alloc::Msg";
}

#[derive(Default, Debug, PartialEq, Eq)]
struct DynMsg {
    code: u32,
}

unsafe extern "C" fn dyn_export_fields(
    src_ptr: *const c_void,
    sink: Option<
        unsafe extern "C" fn(
            sink_ctx: *mut c_void,
            field_name: *const StrView,
            type_tag: *const StrView,
            payload: *const FfiSlice<u8>,
        ) -> FfiResult,
    >,
    sink_ctx: *mut c_void,
) -> FfiResult {
    // SAFETY: `src_ptr` points to a live `DynMsg`.
    let msg = unsafe { &*src_ptr.cast::<DynMsg>() };
    let Some(sink_fn) = sink else {
        return FfiResult::NULL_POINTER;
    };
    let bytes = msg.code.to_le_bytes();
    let fname = StrView::from_str("code");
    let ftag = StrView::from_str("u32");
    let payload = FfiSlice::from_slice(&bytes);
    // SAFETY: `bytes` is a stack slice valid for the synchronous callback.
    unsafe {
        sink_fn(
            sink_ctx,
            &raw const fname,
            &raw const ftag,
            &raw const payload,
        )
    }
}

unsafe extern "C" fn dyn_import_field(
    dst_ptr: *mut c_void,
    field_name: *const StrView,
    _type_tag: *const StrView,
    payload: *const FfiSlice<u8>,
) -> FfiResult {
    if field_name.is_null() || payload.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `dst_ptr` points to a live `DynMsg`.
    let dst = unsafe { &mut *dst_ptr.cast::<DynMsg>() };
    // SAFETY: Synchronous slice from `forward_field`.
    let Ok(name) = (unsafe { (*field_name).try_as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    // SAFETY: Synchronous slice from `forward_field`.
    let Ok(bytes) = (unsafe { (*payload).try_as_slice() }) else {
        return FfiResult::NULL_POINTER;
    };
    if name == "code" && bytes.len() == 4 {
        dst.code = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        return FfiResult::OK;
    }
    FfiResult::INVALID_ARGUMENT
}

static DYN_SYMS: [SymbolEntry; 2] = [
    SymbolEntry {
        name: "bam.v0.zero_alloc::DynMsg.export_fields",
        ptr: dyn_export_fields as *const c_void,
    },
    SymbolEntry {
        name: "bam.v0.zero_alloc::DynMsg.import_field",
        ptr: dyn_import_field as *const c_void,
    },
];

impl ModComponent for DynMsg {
    const CANONICAL_NAME: &'static str = "zero_alloc::DynMsg";
    const COMPONENT_CSTR: &'static CStr = c"zero_alloc::DynMsg";
    const UUID: ComponentUuid = ComponentUuid::from_parts("zero_alloc::DynMsg", 4, 4, 1);
    type VTable = ();

    fn vtable() -> &'static Self::VTable {
        &()
    }

    fn symbols() -> &'static [SymbolEntry] {
        &DYN_SYMS
    }
}

const COMP_POD_MSG: BamComponent = BamComponent::from_raw(1);
const COMP_DYN_MSG: BamComponent = BamComponent::from_raw(2);

struct RegisteredSysSlot {
    callback: Option<unsafe extern "C" fn(*mut BamSystemCtx, *mut c_void) -> FfiResult>,
    user_data: *mut c_void,
    run_if_ctx: Option<unsafe extern "C" fn(*mut BamSystemCtx, *mut c_void) -> FfiResult>,
    run_if_ctx_user_data: *mut c_void,
    terms: Vec<BamTermDesc>,
}

struct ZeroAllocIter {
    world: *mut ZeroAllocHost,
    terms_ptr: *const BamTermDesc,
    terms_len: usize,
}

struct ZeroAllocSysFrame {
    world: *mut ZeroAllocHost,
    iter: ZeroAllocIter,
}

struct ZeroAllocHost {
    pod_queue: Vec<Msg>,
    pod_cursor: usize,
    dyn_queue: Vec<DynMsg>,
    dyn_cursor: usize,
    systems: Vec<RegisteredSysSlot>,
}

struct PodQueueCursor<'a> {
    items: &'a [Msg],
    cursor: &'a mut usize,
}

impl<'a> Iterator for PodQueueCursor<'a> {
    type Item = &'a Msg;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.items.get(*self.cursor)?;
        *self.cursor += 1;
        Some(item)
    }
}

struct DynQueueCursor<'a> {
    items: &'a [DynMsg],
    cursor: &'a mut usize,
}

impl<'a> Iterator for DynQueueCursor<'a> {
    type Item = &'a DynMsg;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.items.get(*self.cursor)?;
        *self.cursor += 1;
        Some(item)
    }
}

unsafe extern "C" fn host_get_proc(_ctx: *const BamHostCtx, sym: *const c_char) -> *const c_void {
    // SAFETY: Caller passes NUL-terminated `sym`.
    let name = unsafe { CStr::from_ptr(sym) };
    match name.to_bytes() {
        b"bam.v0.zero_alloc::DynMsg.export_fields" => dyn_export_fields as *const c_void,
        b"bam.v0.zero_alloc::DynMsg.non_pod_message" => host_msg_read_batch as *const c_void,
        _ => core::ptr::null(),
    }
}

unsafe extern "C" fn host_last_error(_: *const BamHostCtx, _: *mut c_char, _: usize) -> usize {
    0
}

unsafe extern "C" fn host_load_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
    load_ctx.cast()
}

unsafe extern "C" fn host_register_comp(
    _: *mut BamLoadCtx,
    _: *const ComponentDesc,
    out: *mut BamComponent,
) -> FfiResult {
    // SAFETY: `out` is writable.
    unsafe { *out = COMP_POD_MSG };
    FfiResult::OK
}

unsafe extern "C" fn host_resolve_comp(
    _: *const BamHostCtx,
    name: *const c_char,
    _uuid: *const ComponentUuid,
    out: *mut BamComponent,
) -> FfiResult {
    if name.is_null() || out.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `name` is valid CStr and `out` is writable.
    let cname = unsafe { CStr::from_ptr(name) };
    let handle = match cname.to_bytes() {
        b"zero_alloc::Msg" => COMP_POD_MSG,
        b"zero_alloc::DynMsg" => COMP_DYN_MSG,
        _ => return FfiResult::NOT_FOUND,
    };
    // SAFETY: `out` is writable.
    unsafe { *out = handle };
    FfiResult::OK
}

unsafe extern "C" fn host_register_sys(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> FfiResult {
    // SAFETY: `load_ctx` points to `ZeroAllocHost` and `desc` is valid.
    let host = unsafe { &mut *load_ctx.cast::<ZeroAllocHost>() };
    // SAFETY: `desc` is valid.
    let d = unsafe { &*desc };
    let terms = if d.query_count > 0 && !d.queries.is_null() {
        // SAFETY: `d.queries` has `d.query_count` entries.
        let q0 = unsafe { &*d.queries };
        if q0.term_count > 0 && !q0.terms.is_null() {
            // SAFETY: `q0.terms` has `q0.term_count` entries.
            unsafe { core::slice::from_raw_parts(q0.terms, q0.term_count) }.to_vec()
        } else {
            Vec::new()
        }
    } else if !d.run_if_query.is_null() {
        // SAFETY: `d.run_if_query` is non-null and points to a valid `BamQueryDesc`.
        let rq = unsafe { &*d.run_if_query };
        if rq.term_count > 0 && !rq.terms.is_null() {
            // SAFETY: `rq.terms` has `rq.term_count` entries.
            unsafe { core::slice::from_raw_parts(rq.terms, rq.term_count) }.to_vec()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    host.systems.push(RegisteredSysSlot {
        callback: d.callback,
        user_data: d.user_data,
        run_if_ctx: d.run_if_ctx,
        run_if_ctx_user_data: d.run_if_ctx_user_data,
        terms,
    });
    FfiResult::OK
}

unsafe extern "C" fn host_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
    let frame_ptr = sys_ctx.cast::<ZeroAllocSysFrame>();
    // SAFETY: `sys_ctx` points to a valid `ZeroAllocSysFrame`; projecting `world` directly avoids borrowing sibling fields.
    unsafe { core::ptr::read(&raw const (*frame_ptr).world) }.cast()
}

unsafe extern "C" fn host_sys_query(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    out_iter: *mut *mut BamIter,
) -> FfiResult {
    if query_index != 0 {
        return FfiResult::OUT_OF_RANGE;
    }
    let frame_ptr = sys_ctx.cast::<ZeroAllocSysFrame>();
    // SAFETY: `sys_ctx` points to `ZeroAllocSysFrame` and `out_iter` is writable.
    unsafe { *out_iter = (&raw mut (*frame_ptr).iter).cast::<BamIter>() };
    FfiResult::OK
}

unsafe extern "C" fn host_sys_commands(
    _: *mut BamSystemCtx,
    out_cmds: *mut *mut bamabi_guest::raw::BamCommands,
) -> FfiResult {
    // SAFETY: `out_cmds` is writable.
    unsafe { *out_cmds = core::ptr::null_mut() };
    FfiResult::OK
}

unsafe extern "C" fn host_iter_next(_: *mut BamIter, _: *mut usize) -> FfiBool {
    FfiBool::FALSE
}

unsafe extern "C" fn host_iter_reset(_: *mut BamIter) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_iter_entities(_: *const BamIter) -> *const FfiEntity {
    core::ptr::null()
}

unsafe extern "C" fn host_iter_col_ro(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}

unsafe extern "C" fn host_iter_col_mut(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}

unsafe extern "C" fn host_iter_mark(_: *mut BamIter, _: usize, _: usize, _: usize) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_iter_rows_ro(_: *mut BamIter, _: usize) -> *const *const u8 {
    core::ptr::null()
}

unsafe extern "C" fn host_iter_rows_mut(_: *mut BamIter, _: usize) -> *const *mut u8 {
    core::ptr::null_mut()
}

unsafe extern "C" fn host_iter_info(
    iter: *const BamIter,
    term_index: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult {
    // SAFETY: `iter` points to `ZeroAllocIter`.
    let it = unsafe { &*iter.cast::<ZeroAllocIter>() };
    if term_index >= it.terms_len {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: `it.terms_ptr` covers `it.terms_len` terms.
    let term = unsafe { *it.terms_ptr.add(term_index) };
    let info = match term.component {
        COMP_POD_MSG => BamColumnInfo {
            component: COMP_POD_MSG,
            uuid: Msg::UUID,
            size: size_of::<Msg>(),
            align: align_of::<Msg>(),
            mode: term.mode,
            storage_flags: BAM_STORAGE_FLAG_MESSAGE,
            reserved0: 0,
        },
        COMP_DYN_MSG => BamColumnInfo {
            component: COMP_DYN_MSG,
            uuid: DynMsg::UUID,
            size: size_of::<*const c_void>(),
            align: align_of::<*const c_void>(),
            mode: term.mode,
            storage_flags: BAM_STORAGE_FLAG_MESSAGE,
            reserved0: 0,
        },
        _ => return FfiResult::INVALID_COMPONENT,
    };
    // SAFETY: `out_info` is writable.
    unsafe { *out_info = info };
    FfiResult::OK
}

unsafe extern "C" fn host_res_ptr(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}

unsafe extern "C" fn host_res_mut_ptr(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}

unsafe extern "C" fn host_cmd_spawn_empty(
    _: *mut bamabi_guest::raw::BamCommands,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_cmd_spawn(
    _: *mut bamabi_guest::raw::BamCommands,
    _: *const BamComponent,
    _: *const *const u8,
    _: usize,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_cmd_despawn(
    _: *mut bamabi_guest::raw::BamCommands,
    _: FfiEntity,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_cmd_insert(
    _: *mut bamabi_guest::raw::BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_cmd_insert_default(
    _: *mut bamabi_guest::raw::BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_cmd_remove(
    _: *mut bamabi_guest::raw::BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn host_msg_read_batch(
    iter: *mut BamIter,
    term_index: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    // SAFETY: `iter` points to `ZeroAllocIter`.
    let it = unsafe { &*iter.cast::<ZeroAllocIter>() };
    if term_index >= it.terms_len {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: `it.terms_ptr` covers `it.terms_len` terms and `it.world` points to `ZeroAllocHost`.
    let term = unsafe { *it.terms_ptr.add(term_index) };
    // SAFETY: Exclusive mock world access during single-threaded system dispatch.
    let world = unsafe { &mut *it.world };

    let mut read_count = 0usize;
    match term.component {
        COMP_POD_MSG => {
            let cursor_iter = PodQueueCursor {
                items: &world.pod_queue,
                cursor: &mut world.pod_cursor,
            };
            for (i, msg) in cursor_iter.take(max_count).enumerate() {
                // SAFETY: `out_buf` has `max_count * size_of::<Msg>()` writable bytes.
                unsafe {
                    core::ptr::write_unaligned(
                        out_buf.add(i * size_of::<Msg>()).cast::<Msg>(),
                        *msg,
                    );
                }
                read_count = i + 1;
            }
        }
        COMP_DYN_MSG => {
            let cursor_iter = DynQueueCursor {
                items: &world.dyn_queue,
                cursor: &mut world.dyn_cursor,
            };
            for (i, msg) in cursor_iter.take(max_count).enumerate() {
                let ptr = core::ptr::from_ref(msg).cast::<c_void>();
                // SAFETY: `out_buf` has `max_count * size_of::<*const c_void>()` writable bytes.
                unsafe {
                    core::ptr::write_unaligned(
                        out_buf
                            .add(i * size_of::<*const c_void>())
                            .cast::<*const c_void>(),
                        ptr,
                    );
                }
                read_count = i + 1;
            }
        }
        _ => return FfiResult::INVALID_COMPONENT,
    }
    // SAFETY: `out_read` is writable.
    unsafe { *out_read = read_count };
    FfiResult::OK
}

fn make_zero_alloc_api() -> BamHostApi {
    // SAFETY: `BamHostApi` is `#[repr(C)]` over `u32` and `Option<extern "C" fn>`.
    let mut api: BamHostApi = unsafe { MaybeUninit::zeroed().assume_init() };
    api.struct_size = size_of::<BamHostApi>() as u32;
    api.abi_version = ABI_VERSION;
    api.get_proc_address = Some(host_get_proc);
    api.last_error = Some(host_last_error);
    api.load_host_ctx = Some(host_load_ctx);
    api.register_component = Some(host_register_comp);
    api.resolve_component = Some(host_resolve_comp);
    api.register_system = Some(host_register_sys);
    api.sys_host_ctx = Some(host_sys_host_ctx);
    api.sys_query = Some(host_sys_query);
    api.sys_commands = Some(host_sys_commands);
    api.iter_next_chunk = Some(host_iter_next);
    api.iter_reset = Some(host_iter_reset);
    api.iter_entities = Some(host_iter_entities);
    api.iter_column_readonly = Some(host_iter_col_ro);
    api.iter_column_mutable = Some(host_iter_col_mut);
    api.iter_column_mutable_untracked = Some(host_iter_col_mut);
    api.iter_mark_changed_range = Some(host_iter_mark);
    api.iter_column_rows_readonly = Some(host_iter_rows_ro);
    api.iter_column_rows_mutable = Some(host_iter_rows_mut);
    api.iter_column_info = Some(host_iter_info);
    api.res_ptr = Some(host_res_ptr);
    api.res_mut_ptr = Some(host_res_mut_ptr);
    api.cmd_spawn_empty = Some(host_cmd_spawn_empty);
    api.cmd_spawn = Some(host_cmd_spawn);
    api.cmd_despawn = Some(host_cmd_despawn);
    api.cmd_insert = Some(host_cmd_insert);
    api.cmd_insert_default = Some(host_cmd_insert_default);
    api.cmd_remove = Some(host_cmd_remove);
    api.msg_read_batch = Some(host_msg_read_batch);
    api
}

fn run_sys_no_alloc(world: &mut ZeroAllocHost, sys_idx: usize) -> FfiResult {
    let (cb, ud, terms_ptr, terms_len) = {
        let slot = &world.systems[sys_idx];
        (
            slot.callback.expect("system callback"),
            slot.user_data,
            slot.terms.as_ptr(),
            slot.terms.len(),
        )
    };
    let world_ptr = core::ptr::from_mut(world);
    let mut frame = ZeroAllocSysFrame {
        world: world_ptr,
        iter: ZeroAllocIter {
            world: world_ptr,
            terms_ptr,
            terms_len,
        },
    };
    // SAFETY: `frame` is pinned on stack and `ud` belongs to registered `GuestInstance` slot.
    unsafe { cb((&raw mut frame).cast::<BamSystemCtx>(), ud) }
}

fn run_if_no_alloc(world: &mut ZeroAllocHost, sys_idx: usize) -> bool {
    let (pred, ud, terms_ptr, terms_len) = {
        let slot = &world.systems[sys_idx];
        (
            slot.run_if_ctx.expect("run_if_ctx callback"),
            slot.run_if_ctx_user_data,
            slot.terms.as_ptr(),
            slot.terms.len(),
        )
    };
    let world_ptr = core::ptr::from_mut(world);
    let mut frame = ZeroAllocSysFrame {
        world: world_ptr,
        iter: ZeroAllocIter {
            world: world_ptr,
            terms_ptr,
            terms_len,
        },
    };
    // SAFETY: `frame` is pinned on stack and `ud` belongs to registered `GuestInstance` slot.
    unsafe { pred((&raw mut frame).cast::<BamSystemCtx>(), ud) }.is_ok()
}

#[test]
fn zero_alloc_message_reads_drains_and_run_condition_has_messages() {
    let api = make_zero_alloc_api();
    let mut world = ZeroAllocHost {
        pod_queue: Vec::new(),
        pod_cursor: 0,
        dyn_queue: Vec::new(),
        dyn_cursor: 0,
        systems: Vec::new(),
    };
    let manifest = BamManifest::new(c"zero_alloc_mod".as_ptr(), c"0.3.0".as_ptr());
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static COND_RUNS: AtomicU32 = AtomicU32::new(0);
    COND_RUNS.store(0, Ordering::Relaxed);

    // Pre-reserved buffers owned by test closures so paths (b) & (c) avoid any closure capture allocs.
    // SAFETY: `api` and `world` outlive `inst`; `init_guest_instance` initializes `inst` once.
    let status = unsafe {
        init_guest_instance(
            &api,
            core::ptr::from_mut(&mut world).cast::<BamLoadCtx>(),
            &mut inst,
            &manifest,
            |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    // System 0: (a) idle read_messages & drain_messages_with_options (N = 0)
                    assert_eq!(
                        app.system(c"idle_pod").message_read::<Msg>().build(|ctx| {
                            let a = ctx.read_messages::<Msg>(0)?;
                            assert!(a.is_empty());
                            let b = ctx.drain_messages_with_options::<Msg>(
                                0,
                                DrainOptions::new(32, 128),
                            )?;
                            assert!(b.is_empty());
                            Ok(())
                        }),
                        FfiResult::OK
                    );

                    // System 1: (b) read_messages_into into stack [Msg; 64] (N = 64)
                    assert_eq!(
                        app.system(c"into_stack_slice")
                            .message_read::<Msg>()
                            .build(|ctx| {
                                let mut buf = [Msg::default(); 64];
                                let n = ctx.read_messages_into::<Msg>(0, &mut buf)?;
                                assert_eq!(n, 64);
                                for (i, item) in buf.iter().enumerate() {
                                    assert_eq!(item.seq, i as u64);
                                }
                                Ok(())
                            }),
                        FfiResult::OK
                    );

                    // System 2: (c) read_messages_into_vec into pre-reserved Vec (cap=256, N=256)
                    let mut pre_vec_batch = Vec::<Msg>::with_capacity(256);
                    assert_eq!(
                        app.system(c"into_vec_batch")
                            .message_read::<Msg>()
                            .build(move |ctx| {
                                pre_vec_batch.clear();
                                let n =
                                    ctx.read_messages_into_vec::<Msg>(0, &mut pre_vec_batch, 256)?;
                                assert_eq!(n, 256);
                                assert_eq!(pre_vec_batch.len(), 256);
                                for (i, item) in pre_vec_batch.iter().enumerate() {
                                    assert_eq!(item.seq, i as u64);
                                }
                                Ok(())
                            }),
                        FfiResult::OK
                    );

                    // System 3: (c) drain_messages_into_vec into pre-reserved Vec (cap=256, N=256)
                    let mut pre_vec_drain = Vec::<Msg>::with_capacity(256);
                    assert_eq!(
                        app.system(c"drain_into_vec")
                            .message_read::<Msg>()
                            .build(move |ctx| {
                                pre_vec_drain.clear();
                                let n = ctx.drain_messages_into_vec::<Msg>(
                                    0,
                                    &mut pre_vec_drain,
                                    DrainOptions::default(),
                                )?;
                                assert_eq!(n, 256);
                                assert_eq!(pre_vec_drain.len(), 256);
                                for (i, item) in pre_vec_drain.iter().enumerate() {
                                    assert_eq!(item.seq, i as u64);
                                }
                                Ok(())
                            }),
                        FfiResult::OK
                    );

                    // System 4: (d) idle read_non_pod_messages (N = 0)
                    assert_eq!(
                        app.system(c"idle_non_pod")
                            .message_read_dynamic::<DynMsg>()
                            .build(|ctx| {
                                let msgs = ctx.read_non_pod_messages::<DynMsg>(0)?;
                                assert!(msgs.is_empty());
                                Ok(())
                            }),
                        FfiResult::OK
                    );

                    // System 5: (e) RunConditionSystemContext::has_messages (conditions::on_message)
                    assert_eq!(
                        app.system(c"cond_on_msg")
                            .run_if_condition(conditions::on_message::<Msg>())
                            .build(|_ctx| {
                                COND_RUNS.fetch_add(1, Ordering::Relaxed);
                            }),
                        FfiResult::OK
                    );

                    FfiResult::OK
                })
            },
        )
    };
    assert_eq!(status, FfiResult::OK);
    assert_eq!(world.systems.len(), 6);

    // (a) Idle `read_messages` / `drain_messages` (N = 0) -> 0 allocations, 0 bytes (even on tick 1!)
    {
        world.pod_queue.clear();
        world.pod_cursor = 0;
        let (rc, allocs, bytes) = measure_allocs(|| run_sys_no_alloc(&mut world, 0));
        assert_eq!(rc, FfiResult::OK);
        assert_eq!(allocs, 0, "(a) idle read_messages must perform 0 allocs");
        assert_eq!(bytes, 0, "(a) idle read_messages must allocate 0 bytes");
    }

    // (b) `read_messages_into` into stack `[Msg; 64]` (N = 64) -> 0 allocations, 0 bytes
    {
        world.pod_queue = (0..64)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let (rc, allocs, bytes) = measure_allocs(|| run_sys_no_alloc(&mut world, 1));
        assert_eq!(rc, FfiResult::OK);
        assert_eq!(allocs, 0, "(b) read_messages_into must perform 0 allocs");
        assert_eq!(bytes, 0, "(b) read_messages_into must allocate 0 bytes");

        // Also verify `ChunkIter::read_messages_into_uninit` into `[MaybeUninit<Msg>; 32]` -> 0 allocs.
        world.pod_queue = (0..32)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let world_ptr = core::ptr::from_mut(&mut world);
        // SAFETY: `api` and `world_ptr` outlive `standalone_host` and `chunk_iter`.
        let standalone_host =
            unsafe { Host::from_raw(&api, world_ptr.cast::<BamHostCtx>()) }.unwrap();
        let term = BamTermDesc::new(COMP_POD_MSG, AccessMode::MESSAGE_READ);
        let mut iter_state = ZeroAllocIter {
            world: world_ptr,
            terms_ptr: &raw const term,
            terms_len: 1,
        };
        // SAFETY: `iter_state` outlives `chunk_iter`.
        let mut chunk_iter = bamabi_guest::ChunkIter::from_validated(unsafe {
            ValidatedIter::from_raw((&raw mut iter_state).cast::<BamIter>(), &standalone_host)
        });
        let mut uninit_buf = [const { MaybeUninit::<Msg>::uninit() }; 32];
        let (uninit_res, allocs_u, bytes_u) =
            measure_allocs(|| chunk_iter.read_messages_into_uninit::<Msg>(0, &mut uninit_buf));
        assert_eq!(uninit_res, Ok(32));
        assert_eq!(
            allocs_u, 0,
            "(b) read_messages_into_uninit must perform 0 allocs"
        );
        assert_eq!(
            bytes_u, 0,
            "(b) read_messages_into_uninit must allocate 0 bytes"
        );
        for (i, slot) in uninit_buf.iter().enumerate() {
            // SAFETY: `read_messages_into_uninit` returned `Ok(32)`, initializing all 32 slots.
            assert_eq!(unsafe { slot.assume_init_ref() }.seq, i as u64);
        }
    }

    // (c1) `read_messages_into_vec` into pre-reserved `Vec<Msg>` (cap = 256, N = 256) -> 0 allocs
    {
        world.pod_queue = (0..256)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let (rc, allocs, bytes) = measure_allocs(|| run_sys_no_alloc(&mut world, 2));
        assert_eq!(rc, FfiResult::OK);
        assert_eq!(
            allocs, 0,
            "(c) read_messages_into_vec with cap=256, N=256 must perform 0 allocs"
        );
        assert_eq!(bytes, 0);
    }

    // (c2) `drain_messages_into_vec` into pre-reserved `Vec<Msg>` (cap = 256, N = 256) -> 0 allocs
    {
        world.pod_queue = (0..256)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let (rc, allocs, bytes) = measure_allocs(|| run_sys_no_alloc(&mut world, 3));
        assert_eq!(rc, FfiResult::OK);
        assert_eq!(
            allocs, 0,
            "(c) drain_messages_into_vec with cap=256, N=256 must perform 0 allocs"
        );
        assert_eq!(bytes, 0);
    }

    // (d) Idle `read_non_pod_messages` (N = 0): warmup populates `QuerySlotScratch` + `Host` symbol
    // cache on tick 1; steady-state tick 2+ AND standalone `ChunkIter` both perform 0 allocs.
    {
        world.dyn_queue.clear();
        world.dyn_cursor = 0;
        assert_eq!(run_sys_no_alloc(&mut world, 4), FfiResult::OK);

        let (rc, allocs, bytes) = measure_allocs(|| run_sys_no_alloc(&mut world, 4));
        assert_eq!(rc, FfiResult::OK);
        assert_eq!(
            allocs, 0,
            "(d) steady-state idle read_non_pod_messages must perform 0 allocs"
        );
        assert_eq!(bytes, 0);

        // Also verify standalone `ChunkIter` idle `read_non_pod_messages` performs 0 allocs.
        let world_ptr = core::ptr::from_mut(&mut world);
        // SAFETY: `api` and `world_ptr` outlive `standalone_host` and `iter`.
        let standalone_host =
            unsafe { Host::from_raw(&api, world_ptr.cast::<BamHostCtx>()) }.unwrap();
        let term = BamTermDesc::new(COMP_DYN_MSG, AccessMode::MESSAGE_READ);
        let mut iter_state = ZeroAllocIter {
            world: world_ptr,
            terms_ptr: &raw const term,
            terms_len: 1,
        };
        // SAFETY: `iter_state` outlives `chunk_iter`.
        let mut chunk_iter = bamabi_guest::ChunkIter::from_validated(unsafe {
            ValidatedIter::from_raw((&raw mut iter_state).cast::<BamIter>(), &standalone_host)
        });
        let (msgs_res, allocs2, bytes2) =
            measure_allocs(|| chunk_iter.read_non_pod_messages::<DynMsg>(0));
        assert_eq!(msgs_res, Ok(Vec::new()));
        assert_eq!(
            allocs2, 0,
            "(d) standalone idle read_non_pod_messages must perform 0 allocs"
        );
        assert_eq!(bytes2, 0);
    }

    // (e) `RunConditionSystemContext::has_messages`:
    //   - N = 0 -> false (0 allocs)
    //   - N = 150 on frame 1 -> true (0 allocs), frame 2 -> false (0 allocs)
    //   - N = 5 on frame 1 -> true (0 allocs), frame 2 -> false (0 allocs)
    {
        world.pod_queue.clear();
        world.pod_cursor = 0;
        let (has0, allocs0, bytes0) = measure_allocs(|| run_if_no_alloc(&mut world, 5));
        assert!(!has0);
        assert_eq!(allocs0, 0);
        assert_eq!(bytes0, 0);

        // Frame 1 with N = 150 -> drains all 150 across stack probe loops with 0 allocs!
        world.pod_queue = (0..150)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let (has_150_f1, allocs_150_f1, bytes_150_f1) =
            measure_allocs(|| run_if_no_alloc(&mut world, 5));
        assert!(has_150_f1);
        assert_eq!(allocs_150_f1, 0);
        assert_eq!(bytes_150_f1, 0);
        assert_eq!(
            world.pod_cursor, 150,
            "has_messages must drain all 150 queued messages on frame 1"
        );

        // Frame 2 with no new messages -> must see 0 and return false with 0 allocs!
        let (has_150_f2, allocs_150_f2, bytes_150_f2) =
            measure_allocs(|| run_if_no_alloc(&mut world, 5));
        assert!(!has_150_f2);
        assert_eq!(allocs_150_f2, 0);
        assert_eq!(bytes_150_f2, 0);

        // Frame 1 with N = 5 -> true (0 allocs) & drains all 5, then frame 2 -> false (0 allocs)
        world.pod_queue = (0..5)
            .map(|i| Msg {
                seq: i,
                tag: i as u32,
                pad: 0,
            })
            .collect();
        world.pod_cursor = 0;
        let (has_5_f1, allocs_5_f1, bytes_5_f1) = measure_allocs(|| run_if_no_alloc(&mut world, 5));
        assert!(has_5_f1);
        assert_eq!(allocs_5_f1, 0);
        assert_eq!(bytes_5_f1, 0);
        assert_eq!(world.pod_cursor, 5);

        let (has_5_f2, allocs_5_f2, bytes_5_f2) = measure_allocs(|| run_if_no_alloc(&mut world, 5));
        assert!(!has_5_f2);
        assert_eq!(allocs_5_f2, 0);
        assert_eq!(bytes_5_f2, 0);
    }

    // SAFETY: `inst` was returned by `init_guest_instance` and is torn down once here.
    unsafe { shutdown_guest_instance(inst) };
}
