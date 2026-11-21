//! Non-POD message witness round-trip tests (moved out of `src/iter.rs`, reusing shared mock stubs).

mod common;

use core::ffi::{CStr, c_char, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};

use bamabi_guest_sys::{
    GuestInstance, Host, SparseRows, SparseRowsMut, UuidFilter, ValidatedIter,
    verify_same_mod_dynamic_component,
};
use bamabi_sys::{
    AccessMode, BAM_STORAGE_FLAG_MESSAGE, BamColumnInfo, BamComponent, BamHostCtx, BamIter,
    ComponentUuid, FfiResult, FfiSlice, ModComponent, StrView, SymbolEntry,
};
use common::make_mock_api;

/// Heap-owning non-POD message fixture (`String` + `Vec<u32>`) used to test witness field
/// serialization and reconstruction across `ValidatedIter`.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct MockNonPodMsg {
    label: String,
    nums: Vec<u32>,
}

/// Mock `export_fields` witness symbol emitting `label` (`String`) and `nums` (`Vec<u32>`).
unsafe extern "C" fn mock_export_fields(
    src_ptr: *const c_void,
    sink: Option<
        unsafe extern "C" fn(
            sink_ctx: *mut c_void,
            field_name: StrView,
            type_tag: StrView,
            payload: FfiSlice<u8>,
        ) -> FfiResult,
    >,
    sink_ctx: *mut c_void,
) -> FfiResult {
    // SAFETY: `src_ptr` points to a live `MockNonPodMsg`.
    let msg = unsafe { &*src_ptr.cast::<MockNonPodMsg>() };
    let Some(sink_fn) = sink else {
        return FfiResult::NULL_POINTER;
    };
    // SAFETY: Forwarding live UTF-8 field slice into `sink_fn`.
    let rc = unsafe {
        sink_fn(
            sink_ctx,
            StrView::from_str("label"),
            StrView::from_str("String"),
            FfiSlice::from_slice(msg.label.as_bytes()),
        )
    };
    if !rc.is_ok() {
        return rc;
    }
    let mut raw_nums = Vec::with_capacity(4 + msg.nums.len() * 4);
    raw_nums.extend_from_slice(&(msg.nums.len() as u32).to_le_bytes());
    for &n in &msg.nums {
        raw_nums.extend_from_slice(&n.to_le_bytes());
    }
    // SAFETY: Forwarding encoded `Vec<u32>` slice into `sink_fn`.
    unsafe {
        sink_fn(
            sink_ctx,
            StrView::from_str("nums"),
            StrView::from_str("Vec<u32>"),
            FfiSlice::from_slice(&raw_nums),
        )
    }
}

/// Mock `import_field` witness symbol reconstructing `label` and `nums` on a guest `MockNonPodMsg`.
unsafe extern "C" fn mock_import_field(
    dst_ptr: *mut c_void,
    field_name: StrView,
    _type_tag: StrView,
    payload: FfiSlice<u8>,
) -> FfiResult {
    // SAFETY: `dst_ptr` points to a valid `MockNonPodMsg`.
    let dst = unsafe { &mut *dst_ptr.cast::<MockNonPodMsg>() };
    // SAFETY: `field_name` and `payload` borrow live slices for this synchronous callback.
    let Ok(name) = (unsafe { field_name.try_as_str() }) else {
        return FfiResult::INVALID_UTF8;
    };
    // SAFETY: Same as above for `payload`.
    let Ok(bytes) = (unsafe { payload.try_as_slice() }) else {
        return FfiResult::NULL_POINTER;
    };
    match name {
        "label" => {
            let Ok(s) = core::str::from_utf8(bytes) else {
                return FfiResult::INVALID_UTF8;
            };
            dst.label = s.to_owned();
        }
        "nums" => {
            if bytes.len() < 4 {
                return FfiResult::INVALID_ARGUMENT;
            }
            let cnt = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            if bytes.len() != 4 + cnt * 4 {
                return FfiResult::INVALID_ARGUMENT;
            }
            dst.nums.clear();
            for i in 0..cnt {
                let off = 4 + i * 4;
                dst.nums.push(u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]));
            }
        }
        _ => return FfiResult::INVALID_ARGUMENT,
    }
    FfiResult::OK
}

static MOCK_SYMS: [SymbolEntry; 2] = [
    SymbolEntry {
        name: "bam.v0.mock::NonPodChat.export_fields",
        ptr: mock_export_fields as *const c_void,
    },
    SymbolEntry {
        name: "bam.v0.mock::NonPodChat.import_field",
        ptr: mock_import_field as *const c_void,
    },
];

impl ModComponent for MockNonPodMsg {
    const CANONICAL_NAME: &'static str = "mock::NonPodChat";
    const COMPONENT_CSTR: &'static CStr = c"mock::NonPodChat";
    const UUID: ComponentUuid = ComponentUuid::from_parts("mock::NonPodChat", 48, 8, 1);
    type VTable = ();

    /// Returns the unit dummy VTable.
    fn vtable() -> &'static Self::VTable {
        &()
    }

    /// Returns the mock `export_fields` and `import_field` symbols.
    fn symbols() -> &'static [SymbolEntry] {
        &MOCK_SYMS
    }
}

/// Negative-test message type lacking the host `"bam.v0.<M>.non_pod_message"` marker symbol.
#[derive(Default, Debug, PartialEq, Eq)]
struct MockPodWithoutMarker;

impl ModComponent for MockPodWithoutMarker {
    const CANONICAL_NAME: &'static str = "mock::PodWithoutMarker";
    const COMPONENT_CSTR: &'static CStr = c"mock::PodWithoutMarker";
    const UUID: ComponentUuid = ComponentUuid::from_parts("mock::PodWithoutMarker", 8, 8, 2);
    type VTable = ();

    /// Returns the unit dummy VTable.
    fn vtable() -> &'static Self::VTable {
        &()
    }

    /// Returns an empty symbol slice.
    fn symbols() -> &'static [SymbolEntry] {
        &[]
    }
}

/// Mock world backing `BamIter` for non-POD message read/send tests.
struct MockMsgWorld {
    stored: Vec<MockNonPodMsg>,
    sent_packets: Vec<Vec<u8>>,
}

/// Mock `get_proc_address` exporting `export_fields` and `non_pod_message` marker for `NonPodChat`.
unsafe extern "C" fn mock_get_proc(_ctx: *const BamHostCtx, sym: *const c_char) -> *const c_void {
    // SAFETY: Caller passes valid NUL-terminated CStr.
    let name = unsafe { CStr::from_ptr(sym) };
    match name.to_bytes() {
        b"bam.v0.mock::NonPodChat.export_fields" => mock_export_fields as *const c_void,
        b"bam.v0.mock::NonPodChat.non_pod_message" => mock_msg_read_batch as *const c_void,
        _ => core::ptr::null(),
    }
}

/// Mock `iter_column_info` describing term `0` (`MockNonPodMsg`) and term `1` (`MockPodWithoutMarker`).
unsafe extern "C" fn mock_col_info(
    _iter: *const BamIter,
    term_index: usize,
    out_info: *mut BamColumnInfo,
) -> FfiResult {
    if term_index > 1 {
        return FfiResult::OUT_OF_RANGE;
    }
    let uuid = if term_index == 0 {
        MockNonPodMsg::UUID
    } else {
        MockPodWithoutMarker::UUID
    };
    // SAFETY: `out_info` is writable.
    unsafe {
        *out_info = BamColumnInfo {
            component: BamComponent::from_raw((term_index + 1) as u32),
            uuid,
            size: core::mem::size_of::<*const c_void>(),
            align: core::mem::align_of::<*const c_void>(),
            mode: AccessMode::MESSAGE_WRITE,
            storage_flags: BAM_STORAGE_FLAG_MESSAGE,
            reserved0: 0,
        };
    }
    FfiResult::OK
}

/// Mock `msg_read_batch` writing pointers to `MockMsgWorld::stored` elements into `out_buf`.
unsafe extern "C" fn mock_msg_read_batch(
    iter: *mut BamIter,
    _term_index: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_count: *mut usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockMsgWorld`.
    let world = unsafe { &*iter.cast::<MockMsgWorld>() };
    let take = world.stored.len().min(max_count);
    for (i, stored_msg) in world.stored.iter().enumerate().take(take) {
        let msg_ptr = core::ptr::from_ref(stored_msg).cast::<c_void>();
        // SAFETY: `out_buf` is writable for `take * size_of::<*const c_void>()` bytes;
        // `write_unaligned` preserves full pointer provenance.
        #[allow(clippy::cast_ptr_alignment)]
        unsafe {
            core::ptr::write_unaligned(
                out_buf
                    .add(i * core::mem::size_of::<*const c_void>())
                    .cast::<*const c_void>(),
                msg_ptr,
            );
        }
    }
    // SAFETY: `out_count` is writable.
    unsafe {
        *out_count = take;
    }
    FfiResult::OK
}

/// Mock `msg_send` capturing serialized witness packets into `MockMsgWorld::sent_packets`.
unsafe extern "C" fn mock_msg_send(
    iter: *mut BamIter,
    _term_index: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    // SAFETY: `iter` points to `MockMsgWorld`, `data_ptr` is readable for `data_len`.
    let world = unsafe { &mut *iter.cast::<MockMsgWorld>() };
    // SAFETY: `data_ptr` covers `data_len` bytes.
    let pkt = unsafe { core::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
    world.sent_packets.push(pkt);
    FfiResult::OK
}

/// Verifies end-to-end non-POD message reading (via host `export_fields` + guest `import_field`),
/// sending (encoding length-prefixed witness packets with `field_count == 2`), and fail-closed
/// `TYPE_MISMATCH` rejection when a pointer-sized message lacks the `non_pod_message` marker.
#[test]
fn validated_iter_non_pod_message_read_and_send_encode_witness_packets() {
    let mut api = make_mock_api();
    api.get_proc_address = Some(mock_get_proc);
    api.iter_column_info = Some(mock_col_info);
    api.msg_read_batch = Some(mock_msg_read_batch);
    api.msg_send = Some(mock_msg_send);

    let mut world = MockMsgWorld {
        stored: vec![MockNonPodMsg {
            label: "hello".to_owned(),
            nums: vec![10, 20, 30],
        }],
        sent_packets: Vec::new(),
    };
    let host_ctx = (&raw mut world).cast::<BamHostCtx>();
    // SAFETY: `api` and `host_ctx` live for the test duration.
    let host = unsafe { Host::from_raw(&api, host_ctx) }.unwrap();
    // SAFETY: `raw_iter` points to `MockMsgWorld` used by mock callbacks.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut world).cast::<BamIter>(), &host) };

    let got = iter
        .read_non_pod_messages_checked::<MockNonPodMsg>(
            0,
            Some(UuidFilter::exact(MockNonPodMsg::UUID)),
            8,
        )
        .unwrap();
    assert_eq!(got, world.stored);

    let outgoing = MockNonPodMsg {
        label: "reply".to_owned(),
        nums: vec![7, 8],
    };
    assert_eq!(
        iter.send_non_pod_message_checked::<MockNonPodMsg>(
            0,
            Some(UuidFilter::exact(MockNonPodMsg::UUID)),
            &outgoing,
        ),
        FfiResult::OK
    );
    assert_eq!(world.sent_packets.len(), 1);
    assert_eq!(
        u32::from_le_bytes(world.sent_packets[0][0..4].try_into().unwrap()),
        2
    );

    // 8-byte POD message without `non_pod_message` marker must be rejected with TYPE_MISMATCH.
    assert_eq!(
        iter.read_non_pod_messages_checked::<MockPodWithoutMarker>(
            1,
            Some(UuidFilter::exact(MockPodWithoutMarker::UUID)),
            4,
        ),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(
        iter.send_non_pod_message_checked::<MockPodWithoutMarker>(
            1,
            Some(UuidFilter::exact(MockPodWithoutMarker::UUID)),
            &MockPodWithoutMarker,
        ),
        FfiResult::TYPE_MISMATCH
    );
}

/// Verifies that [`SparseRows`] and [`SparseRowsMut`] construct in `O(1)` without scanning inner
/// row pointers upfront, and lazily reject null and misaligned non-ZST row pointers in `get` /
/// `get_mut` while leaving valid rows accessible.
#[test]
fn sparse_rows_and_mut_validate_inner_pointers_lazily_without_scanning_untouched_rows() {
    let mut vals: [u64; 3] = [10, 20, 40];
    let (v0, rest) = vals.split_at_mut(1);
    let (v1, v2) = rest.split_at_mut(1);

    let valid0 = core::ptr::from_mut(&mut v0[0]).cast::<u8>();
    // Misaligned pointer (+1 byte into v1's 8-byte allocation).
    let misaligned1 = core::ptr::from_mut(&mut v1[0]).cast::<u8>().wrapping_add(1);
    let null2 = core::ptr::null_mut::<u8>();
    let valid3 = core::ptr::from_mut(&mut v2[0]).cast::<u8>();

    let ro_ptrs: [*const u8; 4] = [
        valid0.cast_const(),
        misaligned1.cast_const(),
        null2,
        valid3.cast_const(),
    ];
    // SAFETY: `ro_ptrs` is aligned and readable for 4 entries; valid aligned entries point into `vals`.
    let rows = unsafe { SparseRows::<u64>::from_raw_parts(ro_ptrs.as_ptr(), ro_ptrs.len(), false) }
        .expect("from_raw_parts must succeed in O(1) without scanning inner row pointers");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.get(0), Some(&10));
    assert_eq!(rows.get(1), None);
    assert_eq!(rows.get(2), None);
    assert_eq!(rows.get(3), Some(&40));
    assert_eq!(rows.get(4), None);
    let collected: Vec<u64> = rows.iter().copied().collect();
    assert_eq!(collected, vec![10, 40]);

    let mut_ptrs: [*mut u8; 4] = [valid0, misaligned1, null2, valid3];
    // SAFETY: `mut_ptrs` is aligned and readable for 4 entries; valid aligned entries point to distinct `vals`.
    let mut rows_mut =
        unsafe { SparseRowsMut::<u64>::from_raw_parts(mut_ptrs.as_ptr(), mut_ptrs.len(), false) }
            .expect("from_raw_parts must succeed in O(1) without scanning inner row pointers");
    assert_eq!(rows_mut.get(0), Some(&10));
    assert_eq!(rows_mut.get(1), None);
    assert_eq!(rows_mut.get(2), None);
    assert_eq!(rows_mut.get_mut(1), None);
    assert_eq!(rows_mut.get_mut(2), None);
    *rows_mut.get_mut(0).unwrap() = 11;
    *rows_mut.get_mut(3).unwrap() = 44;
    let mut visited = Vec::new();
    rows_mut.for_each_mut(|idx, v| {
        *v += 1;
        visited.push((idx, *v));
    });
    assert_eq!(visited, vec![(0, 12), (3, 45)]);
    assert_eq!(vals[0], 12);
    assert_eq!(vals[2], 45);
}

/// Verifies that repeated `read_non_pod_messages_checked`, `send_non_pod_message_checked`, and
/// `verify_same_mod_dynamic_component` calls cache resolved `"bam.v0.*"` symbols on
/// [`GuestInstance`] (hitting `get_proc_address` at most once per symbol), and that `PacketBuf`
/// encodes both small (stack) and large (>256B heap-spilled) witness packets identically.
#[test]
fn guest_instance_caches_symbol_lookups_and_spills_large_witness_packets() {
    struct SymbolLookupCounters {
        marker_lookups: AtomicUsize,
        export_lookups: AtomicUsize,
        vtable_lookups: AtomicUsize,
    }

    unsafe extern "C" fn counting_get_proc(
        ctx: *const BamHostCtx,
        sym: *const c_char,
    ) -> *const c_void {
        // SAFETY: `ctx` points to `SymbolLookupCounters` and `sym` is a valid C string.
        let counters = unsafe { &*ctx.cast::<SymbolLookupCounters>() };
        // SAFETY: Caller passes valid NUL-terminated CStr.
        let name = unsafe { CStr::from_ptr(sym) };
        match name.to_bytes() {
            b"bam.v0.mock::NonPodChat.non_pod_message" => {
                counters.marker_lookups.fetch_add(1, Ordering::Relaxed);
                mock_msg_read_batch as *const c_void
            }
            b"bam.v0.mock::NonPodChat.export_fields" => {
                counters.export_lookups.fetch_add(1, Ordering::Relaxed);
                mock_export_fields as *const c_void
            }
            b"bam.v0.mock::NonPodChat.vtable" => {
                counters.vtable_lookups.fetch_add(1, Ordering::Relaxed);
                MockNonPodMsg::erased_vtable_ptr()
            }
            _ => core::ptr::null(),
        }
    }

    let mut api = make_mock_api();
    api.get_proc_address = Some(counting_get_proc);
    api.iter_column_info = Some(mock_col_info);
    api.msg_read_batch = Some(mock_msg_read_batch);
    api.msg_send = Some(mock_msg_send);

    let counters = SymbolLookupCounters {
        marker_lookups: AtomicUsize::new(0),
        export_lookups: AtomicUsize::new(0),
        vtable_lookups: AtomicUsize::new(0),
    };
    let mut world = MockMsgWorld {
        stored: vec![MockNonPodMsg {
            label: "cached".to_owned(),
            nums: vec![1, 2, 3],
        }],
        sent_packets: Vec::new(),
    };
    let host_ctx = (&raw const counters).cast::<BamHostCtx>();
    // SAFETY: `api` and `host_ctx` live for the duration of this test.
    let raw_host = unsafe { Host::from_raw(&api, host_ctx) }.unwrap();
    let instance = GuestInstance::new(raw_host, "cache_test_mod");
    let host = instance.host();

    // SAFETY: `raw_iter` points to `world`, disjoint from `counters`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut world).cast::<BamIter>(), host) };

    for _ in 0..4 {
        let got = iter
            .read_non_pod_messages_checked::<MockNonPodMsg>(
                0,
                Some(UuidFilter::exact(MockNonPodMsg::UUID)),
                4,
            )
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "cached");
    }

    // Small packet (fits in 256-byte stack scratch buffer).
    let small_msg = MockNonPodMsg {
        label: "tiny".to_owned(),
        nums: vec![42],
    };
    // Large packet (> 256 bytes, spills seamlessly to heap Vec<u8>).
    let large_msg = MockNonPodMsg {
        label: "x".repeat(320),
        nums: (0..40).collect(),
    };
    for i in 0..4 {
        let msg = if i % 2 == 0 { &small_msg } else { &large_msg };
        assert_eq!(
            iter.send_non_pod_message_checked::<MockNonPodMsg>(
                0,
                Some(UuidFilter::exact(MockNonPodMsg::UUID)),
                msg,
            ),
            FfiResult::OK
        );
    }

    assert_eq!(world.sent_packets.len(), 4);
    assert!(world.sent_packets[0].len() <= 256);
    assert!(world.sent_packets[1].len() > 256);
    for pkt in &world.sent_packets {
        assert_eq!(u32::from_le_bytes(pkt[0..4].try_into().unwrap()), 2);
    }

    for _ in 0..4 {
        assert!(verify_same_mod_dynamic_component::<MockNonPodMsg>(
            host,
            MockNonPodMsg::UUID,
            core::mem::size_of::<MockNonPodMsg>(),
            core::mem::align_of::<MockNonPodMsg>(),
        ));
    }

    assert_eq!(
        counters.marker_lookups.load(Ordering::Relaxed),
        1,
        "non_pod_message marker should be resolved once per GuestInstance"
    );
    assert_eq!(
        counters.export_lookups.load(Ordering::Relaxed),
        1,
        "host export_fields should be resolved once per GuestInstance"
    );
    assert_eq!(
        counters.vtable_lookups.load(Ordering::Relaxed),
        1,
        "vtable symbol should be resolved once per GuestInstance"
    );
}
