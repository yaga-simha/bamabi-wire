//! Kani formal verification proof harnesses for `bamabi_sys` (`PO-W1`..`PO-W8`).
//!
//! Discharges every Layer 0 wire proof obligation codified in the Bamabi Formal Verification
//! Specification over full symbolic integer spaces (`u32`, `usize`) and all 13 `ExtensibleStruct`
//! wire types, plus every `unsafe` boundary primitive (`FfiSlice`, `StrView`, `FfiOption`,
//! `raw_struct_size`, `raw_abi_version`) in `bamabi_sys`.

use core::{
    ffi::{c_char, c_void},
    mem::{align_of, offset_of, size_of},
};

use crate::{
    ABI_VERSION, ABI_VERSION_MAJOR, ABI_VERSION_MINOR, ABI_VERSION_PATCH, AccessMode,
    BAM_COMPONENT_FLAG_IMMUTABLE, BAM_COMPONENT_FLAG_SPARSE_SET, BAM_COMPONENT_FLAG_SUMMARY_TICK,
    BAM_COMPONENT_KNOWN_FLAGS, BAM_HOST_API_SLOT_COUNT, BAM_MANIFEST_KNOWN_FLAGS,
    BAM_MAX_CHANGE_AGE, BAM_OBSERVER_KNOWN_FLAGS, BAM_QUERY_KNOWN_FLAGS, BAM_RELATION_KNOWN_FLAGS,
    BAM_REQUIRED_FLAG_CUSTOM_BYTES, BAM_REQUIRED_FLAG_INIT_FN, BAM_REQUIRED_FLAG_RESOLVE_BY_UUID,
    BAM_REQUIRED_KNOWN_FLAGS, BAM_SCHEDULE_SET_FLAG_PARALLEL, BAM_SCHEDULE_SET_FLAG_SEQUENCE,
    BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK, BAM_SCHEDULE_SET_KNOWN_FLAGS, BAM_SYSTEM_KNOWN_FLAGS,
    BAM_TERM_KNOWN_FLAGS, BAM_TRIGGER_KNOWN_FLAGS, BamChangeTicks, BamCommands, BamComponent,
    BamEventKind, BamFieldDesc, BamFieldKind, BamHostApi, BamHostCtx, BamIter, BamLayoutInfo,
    BamLoadCtx, BamManifest, BamObserverDesc, BamQueryDesc, BamRelationDesc, BamRequiredComponent,
    BamScheduleSetDesc, BamSymbol, BamSystem, BamSystemCtx, BamSystemDesc, BamSystemSet,
    BamTermDesc, BamTriggerDesc, ComponentDesc, ComponentUuid, ExtensibleStruct, FfiAssetHandle,
    FfiAssetLoadState, FfiBool, FfiEntity, FfiOption, FfiResult, FfiSlice, MAX_QUERIES_PER_SYSTEM,
    MAX_TERMS_PER_QUERY, ScheduleId, StrView, VTableHeader, abi_version_is_compatible,
    abi_version_major, abi_version_minor, abi_version_patch, offsets, pack_abi_version,
    read_descriptor, read_extensible_struct, unpack_abi_version,
};

// ============================================================================
// PO-W1: ABI Version Packing & Unpacking Bijectivity
// ============================================================================

/// PO-W1: For all valid `(maj, min, pat)`, `pack_abi_version` / `unpack_abi_version` form a
/// lossless bijection matching individual field accessors with zero bit truncation.
#[kani::proof]
fn po_w1_pack_unpack_abi_version_bijection() {
    let maj: u32 = kani::any();
    let min: u32 = kani::any();
    let pat: u32 = kani::any();
    kani::assume(maj <= 0xFF);
    kani::assume(min <= 0xFF);
    kani::assume(pat <= 0xFFFF);

    let packed = pack_abi_version(maj, min, pat);
    assert_eq!(unpack_abi_version(packed), (maj, min, pat));
    assert_eq!(abi_version_major(packed), maj);
    assert_eq!(abi_version_minor(packed), min);
    assert_eq!(abi_version_patch(packed), pat);

    // Reverse direction over the entire 32-bit word domain.
    let raw: u32 = kani::any();
    let (u_maj, u_min, u_pat) = unpack_abi_version(raw);
    assert!(u_maj <= 0xFF);
    assert!(u_min <= 0xFF);
    assert!(u_pat <= 0xFFFF);
    assert_eq!(pack_abi_version(u_maj, u_min, u_pat), raw);

    // Current ABI_VERSION constant matches its declared components.
    assert_eq!(
        unpack_abi_version(ABI_VERSION),
        (ABI_VERSION_MAJOR, ABI_VERSION_MINOR, ABI_VERSION_PATCH)
    );
}

/// PO-W1 (negative): `pack_abi_version` strictly rejects `major > 0xFF` without silent truncation.
#[kani::proof]
#[kani::should_panic]
fn po_w1_pack_abi_version_rejects_major_overflow() {
    let maj: u32 = kani::any();
    let min: u32 = kani::any();
    let pat: u32 = kani::any();
    kani::assume(maj > 0xFF);
    kani::assume(min <= 0xFF);
    kani::assume(pat <= 0xFFFF);
    let _ = pack_abi_version(maj, min, pat);
}

/// PO-W1 (negative): `pack_abi_version` strictly rejects `minor > 0xFF` without silent truncation.
#[kani::proof]
#[kani::should_panic]
fn po_w1_pack_abi_version_rejects_minor_overflow() {
    let maj: u32 = kani::any();
    let min: u32 = kani::any();
    let pat: u32 = kani::any();
    kani::assume(maj <= 0xFF);
    kani::assume(min > 0xFF);
    kani::assume(pat <= 0xFFFF);
    let _ = pack_abi_version(maj, min, pat);
}

/// PO-W1 (negative): `pack_abi_version` strictly rejects `patch > 0xFFFF` without silent truncation.
#[kani::proof]
#[kani::should_panic]
fn po_w1_pack_abi_version_rejects_patch_overflow() {
    let maj: u32 = kani::any();
    let min: u32 = kani::any();
    let pat: u32 = kani::any();
    kani::assume(maj <= 0xFF);
    kani::assume(min <= 0xFF);
    kani::assume(pat > 0xFFFF);
    let _ = pack_abi_version(maj, min, pat);
}

// ============================================================================
// PO-W2: SemVer ABI Compatibility Matrix & Order-Theoretic Laws
// ============================================================================

/// PO-W2: Exact characterization of `abi_version_is_compatible` over all `u32 x u32` inputs,
/// plus reflexivity, patch-invariance, symmetry (for major == 0), antisymmetry in minor (for
/// major >= 1), and transitivity.
#[kani::proof]
fn po_w2_abi_version_compatibility_laws() {
    let h: u32 = kani::any();
    let g: u32 = kani::any();
    let (h_maj, h_min, _) = unpack_abi_version(h);
    let (g_maj, g_min, _) = unpack_abi_version(g);

    let compat = abi_version_is_compatible(h, g);
    if h_maj == 0 || g_maj == 0 {
        assert_eq!(compat, h_maj == 0 && g_maj == 0 && h_min == g_min);
    } else {
        assert_eq!(compat, h_maj == g_maj && g_min <= h_min);
    }

    // Reflexivity across all 2^32 packed words.
    assert!(abi_version_is_compatible(h, h));

    // Patch-invariance.
    let p1: u32 = kani::any();
    let p2: u32 = kani::any();
    kani::assume(p1 <= 0xFFFF && p2 <= 0xFFFF);
    assert_eq!(
        compat,
        abi_version_is_compatible(
            pack_abi_version(h_maj, h_min, p1),
            pack_abi_version(g_maj, g_min, p2)
        )
    );

    // Pre-1.0 symmetry and post-1.0 minor antisymmetry.
    let rev = abi_version_is_compatible(g, h);
    if h_maj == 0 || g_maj == 0 {
        assert_eq!(compat, rev);
    } else if compat && rev {
        assert_eq!(h_maj, g_maj);
        assert_eq!(h_min, g_min);
    }

    // Transitivity across arbitrary 3-way host/guest chains.
    let c: u32 = kani::any();
    if compat && abi_version_is_compatible(g, c) {
        assert!(abi_version_is_compatible(h, c));
    }
}

// ============================================================================
// PO-W3: ExtensibleStruct Prefix Reading, Zero-Extension & Tail Truncation
// ============================================================================

/// Generic symbolic verifier for `read_extensible_struct<T>` and `read_descriptor<T>`
/// parameterized by `FULL = size_of::<T>()`, `MIN = T::MIN_V0_2_SIZE`, `FULL_P1 = FULL + 1`,
/// and `MIN_P1 = MIN + 1` (avoids unstable `generic_const_exprs`).
fn verify_extensible_struct_contract<
    T: ExtensibleStruct,
    const FULL: usize,
    const MIN: usize,
    const FULL_P1: usize,
    const MIN_P1: usize,
>() {
    assert_eq!(size_of::<T>(), FULL);
    assert_eq!(T::MIN_V0_2_SIZE, MIN);
    assert_eq!(FULL_P1, FULL + 1);
    assert_eq!(MIN_P1, MIN + 1);
    assert!(size_of::<u32>() <= MIN);
    assert!(MIN <= FULL);

    // (a) Null pointer always returns Err(NULL_POINTER) without dereferencing for any struct_size.
    let any_size: usize = kani::any();
    // SAFETY: Null pointer is explicitly handled without dereferencing.
    let null_res = unsafe { read_extensible_struct::<T>(core::ptr::null(), any_size) };
    assert!(matches!(null_res, Err(FfiResult::NULL_POINTER)));
    // SAFETY: Null pointer is explicitly handled without dereferencing.
    let null_desc_res = unsafe { read_descriptor::<T>(core::ptr::null()) };
    assert!(matches!(null_desc_res, Err(FfiResult::NULL_POINTER)));

    // (b) struct_size < MIN always returns Err(INVALID_VERSION) without reading past 0 / 4 bytes.
    let small_size: usize = kani::any();
    kani::assume(small_size < MIN);
    let tiny = [0xAAu8; 1];
    // SAFETY: `small_size < T::MIN_V0_2_SIZE`, so `read_extensible_struct` rejects before copying.
    let under_res = unsafe { read_extensible_struct::<T>(tiny.as_ptr(), small_size) };
    assert!(matches!(under_res, Err(FfiResult::INVALID_VERSION)));

    let small_u32: u32 = kani::any();
    kani::assume((small_u32 as usize) < MIN);
    let mut unaligned_hdr = [0xEEu8; 5];
    unaligned_hdr[1..5].copy_from_slice(&small_u32.to_ne_bytes());
    // SAFETY: Offset +1 provides 4 readable unaligned bytes for the leading `struct_size: u32`,
    // and `small_u32 < MIN` guarantees rejection before copying any further bytes.
    let under_desc =
        unsafe { read_descriptor::<T>(unaligned_hdr.as_ptr().add(1).cast::<c_void>()) };
    assert!(matches!(under_desc, Err(FfiResult::INVALID_VERSION)));

    // (c) & (d) Arbitrary valid prefix copy_len in MIN..=FULL with strict tail zero-extension.
    let src: [u8; FULL] = kani::any();
    let req_size: usize = kani::any();
    kani::assume(req_size >= MIN);
    let copy_len = if req_size < FULL { req_size } else { FULL };

    // SAFETY: `src` provides `FULL >= copy_len` initialized readable bytes.
    let val = unsafe { read_extensible_struct::<T>(src.as_ptr(), req_size) }
        .expect("valid struct_size >= MIN_V0_2_SIZE must succeed");
    let mut out_bytes = [0x55u8; FULL];
    // SAFETY: `val` is initialized and `out_bytes` is a non-overlapping stack buffer of `FULL` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::from_ref(&val).cast::<u8>(),
            out_bytes.as_mut_ptr(),
            FULL,
        );
    }
    let check_idx: usize = kani::any();
    kani::assume(check_idx < FULL);
    if check_idx < copy_len {
        assert_eq!(out_bytes[check_idx], src[check_idx]);
    } else {
        assert_eq!(out_bytes[check_idx], 0);
    }

    // (e) Exact MIN-sized older-version buffer (unaligned +1) never reads out of bounds and
    // zero-extends every byte in MIN..FULL.
    let mut old_buf = [0xCCu8; MIN_P1];
    let old_payload: [u8; MIN] = kani::any();
    old_buf[1..MIN_P1].copy_from_slice(&old_payload);
    old_buf[1..5].copy_from_slice(&(MIN as u32).to_ne_bytes());
    // SAFETY: `old_buf[1..MIN_P1]` holds exactly `MIN` readable bytes with `struct_size == MIN`.
    let old_val = unsafe { read_descriptor::<T>(old_buf.as_ptr().add(1).cast::<c_void>()) }
        .expect("MIN_V0_2_SIZE prefix must succeed");
    let mut old_out = [0x77u8; FULL];
    // SAFETY: `old_val` is initialized and `old_out` is a distinct stack buffer of `FULL` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::from_ref(&old_val).cast::<u8>(),
            old_out.as_mut_ptr(),
            FULL,
        );
    }
    let old_idx: usize = kani::any();
    kani::assume(old_idx < FULL);
    if old_idx < MIN {
        assert_eq!(old_out[old_idx], old_buf[1 + old_idx]);
    } else {
        assert_eq!(old_out[old_idx], 0);
    }

    // (f) Future-version truncation via `read_descriptor` with arbitrary `declared_size > FULL`
    // up to `u32::MAX` never reads past `FULL` bytes.
    let future_size: u32 = kani::any();
    kani::assume((future_size as usize) >= FULL);
    let mut future_buf = [0xBBu8; FULL_P1];
    let future_payload: [u8; FULL] = kani::any();
    future_buf[1..FULL_P1].copy_from_slice(&future_payload);
    future_buf[1..5].copy_from_slice(&future_size.to_ne_bytes());
    // SAFETY: `future_buf[1..FULL_P1]` holds `FULL` bytes (`min(future_size, FULL) == FULL`).
    let future_val = unsafe { read_descriptor::<T>(future_buf.as_ptr().add(1).cast::<c_void>()) }
        .expect("future struct_size >= FULL must truncate cleanly");
    let mut future_out = [0u8; FULL];
    // SAFETY: `future_val` is initialized and `future_out` is a distinct stack buffer of `FULL` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::from_ref(&future_val).cast::<u8>(),
            future_out.as_mut_ptr(),
            FULL,
        );
    }
    let future_idx: usize = kani::any();
    kani::assume(future_idx < FULL);
    assert_eq!(future_out[future_idx], future_buf[1 + future_idx]);
}

macro_rules! po_w3_extensible_harness {
    ($fn_name:ident, $ty:ty) => {
        #[kani::proof]
        fn $fn_name() {
            verify_extensible_struct_contract::<
                $ty,
                { size_of::<$ty>() },
                { <$ty as ExtensibleStruct>::MIN_V0_2_SIZE },
                { size_of::<$ty>() + 1 },
                { <$ty as ExtensibleStruct>::MIN_V0_2_SIZE + 1 },
            >();
        }
    };
}

po_w3_extensible_harness!(po_w3_extensible_bam_manifest, BamManifest);
po_w3_extensible_harness!(po_w3_extensible_bam_host_api, BamHostApi);
po_w3_extensible_harness!(po_w3_extensible_bam_system_desc, BamSystemDesc);
po_w3_extensible_harness!(po_w3_extensible_bam_observer_desc, BamObserverDesc);
po_w3_extensible_harness!(po_w3_extensible_bam_trigger_desc, BamTriggerDesc);
po_w3_extensible_harness!(po_w3_extensible_bam_query_desc, BamQueryDesc);
po_w3_extensible_harness!(po_w3_extensible_component_desc, ComponentDesc);
po_w3_extensible_harness!(po_w3_extensible_vtable_header, VTableHeader);
po_w3_extensible_harness!(po_w3_extensible_bam_field_desc, BamFieldDesc);
po_w3_extensible_harness!(
    po_w3_extensible_bam_required_component,
    BamRequiredComponent
);
po_w3_extensible_harness!(po_w3_extensible_bam_relation_desc, BamRelationDesc);
po_w3_extensible_harness!(po_w3_extensible_bam_layout_info, BamLayoutInfo);
po_w3_extensible_harness!(po_w3_extensible_bam_schedule_set_desc, BamScheduleSetDesc);

/// PO-W3 (header probes): `BamHostApi::raw_struct_size` and `BamHostApi::raw_abi_version` handle
/// null without dereferencing, never read offset 4 when `struct_size < 8`, and faithfully read
/// minimal 4-byte and 8-byte allocations without forming `&BamHostApi`.
#[kani::proof]
fn po_w3_host_api_raw_header_reads() {
    // SAFETY: Null pointers are explicitly rejected before dereferencing.
    assert_eq!(
        unsafe { BamHostApi::raw_struct_size(core::ptr::null()) },
        None
    );
    // SAFETY: Null pointers are explicitly rejected before dereferencing.
    assert_eq!(
        unsafe { BamHostApi::raw_abi_version(core::ptr::null()) },
        None
    );

    // 4-byte allocation with struct_size < 8: must never read offset 4.
    #[repr(C, align(8))]
    struct OnlySizeHeader {
        struct_size: u32,
    }
    let small_sz: u32 = kani::any();
    kani::assume(small_sz < 8);
    let hdr4 = OnlySizeHeader {
        struct_size: small_sz,
    };
    let ptr4 = core::ptr::from_ref(&hdr4).cast::<BamHostApi>();
    // SAFETY: `ptr4` is 8-byte aligned and readable for 4 bytes (`struct_size < 8`).
    assert_eq!(unsafe { BamHostApi::raw_struct_size(ptr4) }, Some(small_sz));
    // SAFETY: `raw_abi_version` checks `struct_size < 8` before touching offset 4.
    assert_eq!(unsafe { BamHostApi::raw_abi_version(ptr4) }, None);

    // 8-byte allocation with arbitrary struct_size >= 8 and arbitrary abi_version.
    #[repr(C, align(8))]
    struct FullHeader {
        struct_size: u32,
        abi_version: u32,
    }
    let full_sz: u32 = kani::any();
    kani::assume(full_sz >= 8);
    let abi_ver: u32 = kani::any();
    let hdr8 = FullHeader {
        struct_size: full_sz,
        abi_version: abi_ver,
    };
    let ptr8 = core::ptr::from_ref(&hdr8).cast::<BamHostApi>();
    // SAFETY: `ptr8` is 8-byte aligned and readable for 8 bytes (`struct_size >= 8`).
    assert_eq!(unsafe { BamHostApi::raw_struct_size(ptr8) }, Some(full_sz));
    // SAFETY: `ptr8` is 8-byte aligned and readable for 8 bytes (`struct_size >= 8`).
    assert_eq!(unsafe { BamHostApi::raw_abi_version(ptr8) }, Some(abi_ver));
}

// ============================================================================
// PO-W4: Overflow-Free `covers_slot` Arithmetic Across All Extensible Structs
// ============================================================================

/// PO-W4: `covers_slot` never overflows for any `(struct_size, slot_offset, slot_size)` and
/// returns `true` iff `slot_offset + slot_size <= struct_size` in mathematical integer arithmetic.
#[kani::proof]
fn po_w4_covers_slot_exact_and_overflow_free() {
    let struct_size: u32 = kani::any();
    let slot_offset: usize = kani::any();
    let slot_size: usize = kani::any();

    let expected = (slot_offset as u128) + (slot_size as u128) <= (struct_size as u128);

    // SAFETY: All-bits-zero is valid for every `ExtensibleStruct`.
    let mut host_api: BamHostApi = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
    host_api.struct_size = struct_size;
    assert_eq!(host_api.covers_slot(slot_offset, slot_size), expected);

    let mut manifest = BamManifest::new(core::ptr::null(), core::ptr::null());
    manifest.struct_size = struct_size;
    assert_eq!(manifest.covers_slot(slot_offset, slot_size), expected);

    let mut comp = ComponentDesc::new(core::ptr::null(), ComponentUuid::ZERO, 0, 1);
    comp.struct_size = struct_size;
    assert_eq!(comp.covers_slot(slot_offset, slot_size), expected);

    // SAFETY: All-bits-zero is valid for `BamSystemDesc`.
    let mut sys: BamSystemDesc = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
    sys.struct_size = struct_size;
    assert_eq!(sys.covers_slot(slot_offset, slot_size), expected);

    // SAFETY: All-bits-zero is valid for `BamObserverDesc`.
    let mut obs: BamObserverDesc = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
    obs.struct_size = struct_size;
    assert_eq!(obs.covers_slot(slot_offset, slot_size), expected);

    let mut trig = BamTriggerDesc::new(BamComponent::INVALID, core::ptr::null_mut(), 0);
    trig.struct_size = struct_size;
    assert_eq!(trig.covers_slot(slot_offset, slot_size), expected);
}

// ============================================================================
// PO-W5: Complete Wire Descriptor & Handshake Validation Contracts
// ============================================================================

/// PO-W5 (BamManifest): Exact specification of `BamManifest::validate_for_host` over all field
/// combinations.
#[kani::proof]
fn po_w5_bam_manifest_validate_for_host() {
    let name_byte: c_char = 0;
    let ver_byte: c_char = 0;
    let name_non_null: bool = kani::any();
    let ver_non_null: bool = kani::any();

    let host_version: u32 = kani::any();
    let manifest = BamManifest {
        struct_size: kani::any(),
        abi_version: kani::any(),
        name: if name_non_null {
            &raw const name_byte
        } else {
            core::ptr::null()
        },
        version: if ver_non_null {
            &raw const ver_byte
        } else {
            core::ptr::null()
        },
        flags: kani::any(),
        reserved0: kani::any(),
        save_state: None,
        restore_state: None,
    };

    let res = manifest.validate_for_host(host_version);
    if (manifest.struct_size as usize) < BamManifest::MIN_V0_2_SIZE {
        assert_eq!(res, FfiResult::INVALID_VERSION);
    } else if !abi_version_is_compatible(host_version, manifest.abi_version) {
        assert_eq!(res, FfiResult::INVALID_VERSION);
    } else if (manifest.flags & !BAM_MANIFEST_KNOWN_FLAGS) != 0 {
        assert_eq!(res, FfiResult::UNSUPPORTED);
    } else if manifest.reserved0 != 0 {
        assert_eq!(res, FfiResult::INVALID_ARGUMENT);
    } else if !name_non_null || !ver_non_null {
        assert_eq!(res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(res, FfiResult::OK);
    }
}

// Stubs for symbolically populating the 27 mandatory core v0.2 slots of `BamHostApi`.
unsafe extern "C" fn stub_get_proc(_: *const BamHostCtx, _: *const c_char) -> *const c_void {
    core::ptr::null()
}
unsafe extern "C" fn stub_last_error(_: *const BamHostCtx, _: *mut c_char, _: usize) -> usize {
    0
}
unsafe extern "C" fn stub_load_host_ctx(_: *const BamLoadCtx) -> *const BamHostCtx {
    core::ptr::null()
}
unsafe extern "C" fn stub_register_comp(
    _: *mut BamLoadCtx,
    _: *const ComponentDesc,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_resolve_comp(
    _: *const BamHostCtx,
    _: *const c_char,
    _: *const ComponentUuid,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_register_sys(_: *mut BamLoadCtx, _: *const BamSystemDesc) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_host_ctx(_: *const BamSystemCtx) -> *const BamHostCtx {
    core::ptr::null()
}
unsafe extern "C" fn stub_sys_query(
    _: *mut BamSystemCtx,
    _: usize,
    _: *mut *mut BamIter,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_commands(
    _: *mut BamSystemCtx,
    _: *mut *mut BamCommands,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_next(_: *mut BamIter, _: *mut usize) -> FfiBool {
    FfiBool::FALSE
}
unsafe extern "C" fn stub_iter_reset(_: *mut BamIter) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_entities(_: *const BamIter) -> *const FfiEntity {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_col_ro(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_col_mut(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_iter_mark(_: *mut BamIter, _: usize, _: usize, _: usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_rows_ro(_: *mut BamIter, _: usize) -> *const *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_rows_mut(_: *mut BamIter, _: usize) -> *const *mut u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_info(
    _: *const BamIter,
    _: usize,
    _: *mut crate::BamColumnInfo,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_res_ptr(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_res_mut_ptr(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_cmd_spawn_empty(_: *mut BamCommands, _: *mut FfiEntity) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_spawn(
    _: *mut BamCommands,
    _: *const BamComponent,
    _: *const *const u8,
    _: usize,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_despawn(_: *mut BamCommands, _: FfiEntity) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert_default(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_remove(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}

/// PO-W5 (BamHostApi): Exact specification of `BamHostApi::validate_for_guest` over all
/// `struct_size`, `abi_version`, `guest_version`, and all 2^27 subsets of the 27 mandatory core
/// v0.2 slots.
#[kani::proof]
fn po_w5_bam_host_api_validate_for_guest() {
    let core_mask: u32 = kani::any();
    let all_core_present = (core_mask & ((1u32 << 27) - 1)) == ((1u32 << 27) - 1);

    // SAFETY: All-bits-zero initializes all 68 `Option<fn>` slots to `None`.
    let mut api: BamHostApi = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
    api.struct_size = kani::any();
    api.abi_version = kani::any();
    let guest_version: u32 = kani::any();

    if (core_mask & (1 << 0)) != 0 {
        api.get_proc_address = Some(stub_get_proc);
    }
    if (core_mask & (1 << 1)) != 0 {
        api.last_error = Some(stub_last_error);
    }
    if (core_mask & (1 << 2)) != 0 {
        api.load_host_ctx = Some(stub_load_host_ctx);
    }
    if (core_mask & (1 << 3)) != 0 {
        api.register_component = Some(stub_register_comp);
    }
    if (core_mask & (1 << 4)) != 0 {
        api.resolve_component = Some(stub_resolve_comp);
    }
    if (core_mask & (1 << 5)) != 0 {
        api.register_system = Some(stub_register_sys);
    }
    if (core_mask & (1 << 6)) != 0 {
        api.sys_host_ctx = Some(stub_sys_host_ctx);
    }
    if (core_mask & (1 << 7)) != 0 {
        api.sys_query = Some(stub_sys_query);
    }
    if (core_mask & (1 << 8)) != 0 {
        api.sys_commands = Some(stub_sys_commands);
    }
    if (core_mask & (1 << 9)) != 0 {
        api.iter_next_chunk = Some(stub_iter_next);
    }
    if (core_mask & (1 << 10)) != 0 {
        api.iter_reset = Some(stub_iter_reset);
    }
    if (core_mask & (1 << 11)) != 0 {
        api.iter_entities = Some(stub_iter_entities);
    }
    if (core_mask & (1 << 12)) != 0 {
        api.iter_column_readonly = Some(stub_iter_col_ro);
    }
    if (core_mask & (1 << 13)) != 0 {
        api.iter_column_mutable = Some(stub_iter_col_mut);
    }
    if (core_mask & (1 << 14)) != 0 {
        api.iter_column_mutable_untracked = Some(stub_iter_col_mut);
    }
    if (core_mask & (1 << 15)) != 0 {
        api.iter_mark_changed_range = Some(stub_iter_mark);
    }
    if (core_mask & (1 << 16)) != 0 {
        api.iter_column_rows_readonly = Some(stub_iter_rows_ro);
    }
    if (core_mask & (1 << 17)) != 0 {
        api.iter_column_rows_mutable = Some(stub_iter_rows_mut);
    }
    if (core_mask & (1 << 18)) != 0 {
        api.iter_column_info = Some(stub_iter_info);
    }
    if (core_mask & (1 << 19)) != 0 {
        api.res_ptr = Some(stub_res_ptr);
    }
    if (core_mask & (1 << 20)) != 0 {
        api.res_mut_ptr = Some(stub_res_mut_ptr);
    }
    if (core_mask & (1 << 21)) != 0 {
        api.cmd_spawn_empty = Some(stub_cmd_spawn_empty);
    }
    if (core_mask & (1 << 22)) != 0 {
        api.cmd_spawn = Some(stub_cmd_spawn);
    }
    if (core_mask & (1 << 23)) != 0 {
        api.cmd_despawn = Some(stub_cmd_despawn);
    }
    if (core_mask & (1 << 24)) != 0 {
        api.cmd_insert = Some(stub_cmd_insert);
    }
    if (core_mask & (1 << 25)) != 0 {
        api.cmd_insert_default = Some(stub_cmd_insert_default);
    }
    if (core_mask & (1 << 26)) != 0 {
        api.cmd_remove = Some(stub_cmd_remove);
    }

    assert_eq!(api.has_all_v0_2_slots(), all_core_present);
    let res = api.validate_for_guest(guest_version);
    if (api.struct_size as usize) < BamHostApi::MIN_V0_2_SIZE {
        assert_eq!(res, FfiResult::INVALID_VERSION);
    } else if !abi_version_is_compatible(api.abi_version, guest_version) {
        assert_eq!(res, FfiResult::INVALID_VERSION);
    } else if !all_core_present {
        assert_eq!(res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(res, FfiResult::OK);
    }
}

/// PO-W5 (ComponentDesc, BamRelationDesc, BamScheduleSetDesc, VTableHeader): Complete validation
/// contracts over arbitrary sizes, alignments, flags, reserved words, and pointer presence.
#[kani::proof]
fn po_w5_component_relation_schedule_and_vtable_validation() {
    let dummy_char: c_char = 0;
    let dummy_comp = BamComponent::from_raw(1);
    let dummy_field = BamFieldDesc::new(
        crate::BamFieldKind::U32,
        StrView::empty(),
        StrView::empty(),
        0,
        4,
        4,
    );
    let dummy_req = BamRequiredComponent::by_handle(dummy_comp);
    let dummy_set = BamSystemSet::from_raw(0);

    // 1. ComponentDesc::validate
    let name_ok: bool = kani::any();
    let req_ptr_ok: bool = kani::any();
    let fields_ptr_ok: bool = kani::any();
    let req_init_ptr_ok: bool = kani::any();
    let mut comp = ComponentDesc::new(
        if name_ok {
            &raw const dummy_char
        } else {
            core::ptr::null()
        },
        ComponentUuid::ZERO,
        kani::any(),
        kani::any(),
    );
    comp.struct_size = kani::any();
    comp.flags = kani::any();
    comp.required_components = if req_ptr_ok {
        &raw const dummy_comp
    } else {
        core::ptr::null()
    };
    comp.required_component_count = kani::any();
    comp.fields = if fields_ptr_ok {
        &raw const dummy_field
    } else {
        core::ptr::null()
    };
    comp.field_count = kani::any();
    comp.required_init_descs = if req_init_ptr_ok {
        &raw const dummy_req
    } else {
        core::ptr::null()
    };
    comp.required_init_desc_count = kani::any();

    let comp_res = comp.validate();
    assert_eq!(comp.is_valid(), comp_res.is_ok());
    let comp_layout_ok = comp.align.is_power_of_two()
        && comp.size.is_multiple_of(comp.align)
        && core::alloc::Layout::from_size_align(comp.size, comp.align).is_ok();
    assert_eq!(comp.layout().is_some(), comp_layout_ok);
    if (comp.struct_size as usize) < ComponentDesc::MIN_V0_2_SIZE {
        assert_eq!(comp_res, FfiResult::INVALID_VERSION);
    } else if (comp.flags & !BAM_COMPONENT_KNOWN_FLAGS) != 0 {
        assert_eq!(comp_res, FfiResult::UNSUPPORTED);
    } else if (comp.flags & BAM_COMPONENT_FLAG_SPARSE_SET) != 0
        && (comp.flags & BAM_COMPONENT_FLAG_SUMMARY_TICK) != 0
    {
        assert_eq!(comp_res, FfiResult::INVALID_ARGUMENT);
    } else if !name_ok
        || (comp.required_component_count > 0 && !req_ptr_ok)
        || (comp.field_count > 0 && !fields_ptr_ok)
        || (comp.required_init_desc_count > 0 && !req_init_ptr_ok)
    {
        assert_eq!(comp_res, FfiResult::NULL_POINTER);
    } else if !comp_layout_ok {
        assert_eq!(comp_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(comp_res, FfiResult::OK);
    }
    let _ = BAM_COMPONENT_FLAG_IMMUTABLE;

    // 2. BamRelationDesc::validate (exact bi-implication incl. Layout::from_size_align upper bound)
    let rel_name_ok: bool = kani::any();
    let mut rel = BamRelationDesc::new(
        if rel_name_ok {
            &raw const dummy_char
        } else {
            core::ptr::null()
        },
        ComponentUuid::ZERO,
    );
    rel.struct_size = kani::any();
    rel.flags = kani::any();
    rel.data_size = kani::any();
    rel.data_align = kani::any();
    rel.reserved0 = kani::any();
    let rel_res = rel.validate();
    let rel_layout_ok = rel.data_align.is_power_of_two()
        && rel.data_size.is_multiple_of(rel.data_align)
        && core::alloc::Layout::from_size_align(rel.data_size, rel.data_align).is_ok();
    if (rel.struct_size as usize) < BamRelationDesc::MIN_V0_3_SIZE {
        assert_eq!(rel_res, FfiResult::INVALID_VERSION);
    } else if (rel.flags & !BAM_RELATION_KNOWN_FLAGS) != 0 {
        assert_eq!(rel_res, FfiResult::UNSUPPORTED);
    } else if rel.reserved0 != 0 {
        assert_eq!(rel_res, FfiResult::INVALID_ARGUMENT);
    } else if !rel_name_ok {
        assert_eq!(rel_res, FfiResult::NULL_POINTER);
    } else if !rel_layout_ok {
        assert_eq!(rel_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(rel_res, FfiResult::OK);
    }

    // 3. BamScheduleSetDesc::validate (exact bi-implication + mutually exclusive execution-mode flags)
    let set_name_ok: bool = kani::any();
    let before_ptr_ok: bool = kani::any();
    let after_ptr_ok: bool = kani::any();
    let mut sched = BamScheduleSetDesc::new(if set_name_ok {
        &raw const dummy_char
    } else {
        core::ptr::null()
    });
    sched.struct_size = kani::any();
    sched.flags = kani::any();
    sched.schedule = ScheduleId(kani::any());
    sched.before_count = kani::any();
    sched.after_count = kani::any();
    sched.reserved0 = kani::any();
    sched.before_sets = if before_ptr_ok {
        &raw const dummy_set
    } else {
        core::ptr::null()
    };
    sched.after_sets = if after_ptr_ok {
        &raw const dummy_set
    } else {
        core::ptr::null()
    };
    let sched_res = sched.validate();
    let exec_bits = sched.flags
        & (BAM_SCHEDULE_SET_FLAG_SEQUENCE
            | BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK
            | BAM_SCHEDULE_SET_FLAG_PARALLEL);
    if (sched.struct_size as usize) < BamScheduleSetDesc::MIN_V0_3_SIZE {
        assert_eq!(sched_res, FfiResult::INVALID_VERSION);
    } else if (sched.flags & !BAM_SCHEDULE_SET_KNOWN_FLAGS) != 0 {
        assert_eq!(sched_res, FfiResult::UNSUPPORTED);
    } else if exec_bits.count_ones() > 1 || sched.reserved0 != 0 || !sched.schedule.is_valid() {
        assert_eq!(sched_res, FfiResult::INVALID_ARGUMENT);
    } else if !set_name_ok
        || (sched.before_count > 0 && !before_ptr_ok)
        || (sched.after_count > 0 && !after_ptr_ok)
    {
        assert_eq!(sched_res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(sched_res, FfiResult::OK);
    }

    // 4. VTableHeader::validate
    let expected_uuid = ComponentUuid::from_bytes(kani::any());
    let actual_uuid = ComponentUuid::from_bytes(kani::any());
    let hdr = VTableHeader {
        struct_size: kani::any(),
        abi_version: kani::any(),
        uuid: actual_uuid,
        size: kani::any(),
        align: kani::any(),
    };
    let min_vtable_sz: usize = kani::any();
    let vt_res = hdr.validate(min_vtable_sz, expected_uuid);
    let required = if min_vtable_sz > VTableHeader::MIN_V0_2_SIZE {
        min_vtable_sz
    } else {
        VTableHeader::MIN_V0_2_SIZE
    };
    if (hdr.struct_size as usize) < required {
        assert_eq!(vt_res, FfiResult::INVALID_VERSION);
    } else if !abi_version_is_compatible(hdr.abi_version, ABI_VERSION) {
        assert_eq!(vt_res, FfiResult::INVALID_VERSION);
    } else if actual_uuid != expected_uuid {
        assert_eq!(vt_res, FfiResult::TYPE_MISMATCH);
    } else {
        assert_eq!(vt_res, FfiResult::OK);
    }
}

/// PO-W5 (BamFieldDesc & BamRequiredComponent): Exact error-code bi-implication covering
/// `StrView` `isize::MAX` bounds, field `offset + size <= isize::MAX` overflow checks, mutually
/// exclusive required-component initializers, and `init_size <= isize::MAX`.
#[kani::proof]
fn po_w5_field_desc_and_required_component_validation() {
    let dummy_byte: u8 = 0;

    unsafe extern "C" fn stub_req_init(_: *mut c_void, _: *mut c_void) -> FfiResult {
        FfiResult::OK
    }

    // 1. BamFieldDesc::validate
    let name_ptr_ok: bool = kani::any();
    let tag_ptr_ok: bool = kani::any();
    let field = BamFieldDesc {
        struct_size: kani::any(),
        kind: BamFieldKind(kani::any()),
        name: StrView {
            ptr: if name_ptr_ok {
                &raw const dummy_byte
            } else {
                core::ptr::null()
            },
            len: kani::any(),
        },
        type_tag: StrView {
            ptr: if tag_ptr_ok {
                &raw const dummy_byte
            } else {
                core::ptr::null()
            },
            len: kani::any(),
        },
        offset: kani::any(),
        size: kani::any(),
        align: kani::any(),
    };
    let f_res = field.validate();
    let layout_ok = field.align.is_power_of_two()
        && field.size.is_multiple_of(field.align)
        && core::alloc::Layout::from_size_align(field.size, field.align).is_ok();
    let span_ok = field
        .offset
        .checked_add(field.size)
        .is_some_and(|end| end <= isize::MAX as usize);
    if (field.struct_size as usize) < BamFieldDesc::MIN_V0_3_SIZE {
        assert_eq!(f_res, FfiResult::INVALID_VERSION);
    } else if !field.kind.is_valid() {
        assert_eq!(f_res, FfiResult::INVALID_ARGUMENT);
    } else if field.name.len > 0 && !name_ptr_ok {
        assert_eq!(f_res, FfiResult::NULL_POINTER);
    } else if field.name.len == 0 || field.name.len > isize::MAX as usize {
        assert_eq!(f_res, FfiResult::INVALID_ARGUMENT);
    } else if field.type_tag.len > 0 && !tag_ptr_ok {
        assert_eq!(f_res, FfiResult::NULL_POINTER);
    } else if field.type_tag.len > isize::MAX as usize {
        assert_eq!(f_res, FfiResult::INVALID_ARGUMENT);
    } else if !layout_ok || !span_ok {
        assert_eq!(f_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(f_res, FfiResult::OK);
    }

    // 2. BamRequiredComponent::validate
    let init_bytes_ok: bool = kani::any();
    let init_fn_ok: bool = kani::any();
    let req = BamRequiredComponent {
        struct_size: kani::any(),
        flags: kani::any(),
        component: BamComponent(kani::any()),
        reserved0: kani::any(),
        uuid: ComponentUuid::from_bytes(kani::any()),
        init_bytes: if init_bytes_ok {
            &raw const dummy_byte
        } else {
            core::ptr::null()
        },
        init_size: kani::any(),
        init_fn: if init_fn_ok {
            Some(stub_req_init)
        } else {
            None
        },
        user_data: core::ptr::null_mut(),
    };
    let r_res = req.validate();
    let has_bytes = (req.flags & BAM_REQUIRED_FLAG_CUSTOM_BYTES) != 0;
    let has_fn = (req.flags & BAM_REQUIRED_FLAG_INIT_FN) != 0;
    let by_uuid = (req.flags & BAM_REQUIRED_FLAG_RESOLVE_BY_UUID) != 0;
    if (req.struct_size as usize) < BamRequiredComponent::MIN_V0_3_SIZE {
        assert_eq!(r_res, FfiResult::INVALID_VERSION);
    } else if (req.flags & !BAM_REQUIRED_KNOWN_FLAGS) != 0 {
        assert_eq!(r_res, FfiResult::UNSUPPORTED);
    } else if req.reserved0 != 0 || (has_bytes && has_fn) {
        assert_eq!(r_res, FfiResult::INVALID_ARGUMENT);
    } else if by_uuid && req.uuid.is_zero() {
        assert_eq!(r_res, FfiResult::INVALID_ARGUMENT);
    } else if !by_uuid && !req.component.is_valid() {
        assert_eq!(r_res, FfiResult::INVALID_COMPONENT);
    } else if (has_bytes && req.init_size > 0 && !init_bytes_ok) || (has_fn && !init_fn_ok) {
        assert_eq!(r_res, FfiResult::NULL_POINTER);
    } else if req.init_size > isize::MAX as usize {
        assert_eq!(r_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(r_res, FfiResult::OK);
    }
}

/// PO-W5 (System, Query, Term, Observer, Trigger, AssetHandle): Complete validation contracts.
#[kani::proof]
fn po_w5_system_query_term_observer_trigger_and_asset_validation() {
    let dummy_char: c_char = 0;
    let dummy_cstr_ptr: *const c_char = &raw const dummy_char;
    let dummy_term = BamTermDesc::read(BamComponent::from_raw(1));
    let dummy_query = BamQueryDesc::empty();
    let dummy_comp = BamComponent::from_raw(1);
    let mut dummy_byte: u8 = 0;

    unsafe extern "C" fn stub_sys(_: *mut BamSystemCtx, _: *mut c_void) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn stub_obs(
        _: FfiEntity,
        _: *const c_void,
        _: *mut BamCommands,
        _: *mut c_void,
    ) -> FfiResult {
        FfiResult::OK
    }
    unsafe extern "C" fn stub_obs_mut(
        _: FfiEntity,
        _: *mut u8,
        _: *mut FfiBool,
        _: *mut BamCommands,
        _: *mut c_void,
    ) -> FfiResult {
        FfiResult::OK
    }

    // 1. BamTermDesc::validate
    let term = BamTermDesc {
        component: BamComponent(kani::any()),
        mode: AccessMode(kani::any()),
        flags: kani::any(),
        or_group: kani::any(),
        reserved0: kani::any(),
    };
    let term_res = term.validate();
    if (term.flags & !BAM_TERM_KNOWN_FLAGS) != 0 {
        assert_eq!(term_res, FfiResult::UNSUPPORTED);
    } else if term.reserved0 != 0 {
        assert_eq!(term_res, FfiResult::INVALID_ARGUMENT);
    } else if !term.component.is_valid() {
        assert_eq!(term_res, FfiResult::INVALID_COMPONENT);
    } else if !term.mode.is_valid() {
        assert_eq!(term_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(term_res, FfiResult::OK);
    }

    // 2. BamQueryDesc::validate
    let terms_non_null: bool = kani::any();
    let query = BamQueryDesc {
        struct_size: kani::any(),
        flags: kani::any(),
        terms: if terms_non_null {
            &raw const dummy_term
        } else {
            core::ptr::null()
        },
        term_count: kani::any(),
    };
    let query_res = query.validate();
    if (query.struct_size as usize) < BamQueryDesc::MIN_V0_2_SIZE {
        assert_eq!(query_res, FfiResult::INVALID_VERSION);
    } else if (query.flags & !BAM_QUERY_KNOWN_FLAGS) != 0 {
        assert_eq!(query_res, FfiResult::UNSUPPORTED);
    } else if query.term_count > MAX_TERMS_PER_QUERY {
        assert_eq!(query_res, FfiResult::CAPACITY_EXCEEDED);
    } else if query.term_count > 0 && !terms_non_null {
        assert_eq!(query_res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(query_res, FfiResult::OK);
    }

    // 3. BamSystemDesc::validate
    let sys_name_ok: bool = kani::any();
    let sys_cb_ok: bool = kani::any();
    let q_ptr_ok: bool = kani::any();
    let after_ok: bool = kani::any();
    let before_ok: bool = kani::any();
    let sets_ok: bool = kani::any();
    let aw_ok: bool = kani::any();
    let bw_ok: bool = kani::any();
    let amb_ok: bool = kani::any();
    let sys = BamSystemDesc {
        struct_size: kani::any(),
        flags: kani::any(),
        name: if sys_name_ok {
            dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        queries: if q_ptr_ok {
            &raw const dummy_query
        } else {
            core::ptr::null()
        },
        query_count: kani::any(),
        callback: if sys_cb_ok { Some(stub_sys) } else { None },
        user_data: core::ptr::null_mut(),
        schedule: ScheduleId(kani::any()),
        reserved0: kani::any(),
        after: if after_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        after_count: kani::any(),
        before: if before_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        before_count: kani::any(),
        run_if: None,
        run_if_user_data: core::ptr::null_mut(),
        in_sets: if sets_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        in_set_count: kani::any(),
        after_weak: if aw_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        after_weak_count: kani::any(),
        before_weak: if bw_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        before_weak_count: kani::any(),
        ambiguous_with: if amb_ok {
            &raw const dummy_cstr_ptr
        } else {
            core::ptr::null()
        },
        ambiguous_with_count: kani::any(),
    };
    let sys_res = sys.validate();
    if (sys.struct_size as usize) < BamSystemDesc::MIN_V0_2_SIZE {
        assert_eq!(sys_res, FfiResult::INVALID_VERSION);
    } else if (sys.flags & !BAM_SYSTEM_KNOWN_FLAGS) != 0 {
        assert_eq!(sys_res, FfiResult::UNSUPPORTED);
    } else if sys.reserved0 != 0 {
        assert_eq!(sys_res, FfiResult::INVALID_ARGUMENT);
    } else if !sys_name_ok || !sys_cb_ok {
        assert_eq!(sys_res, FfiResult::NULL_POINTER);
    } else if sys.query_count > MAX_QUERIES_PER_SYSTEM {
        assert_eq!(sys_res, FfiResult::CAPACITY_EXCEEDED);
    } else if (sys.query_count > 0 && !q_ptr_ok)
        || (sys.after_count > 0 && !after_ok)
        || (sys.before_count > 0 && !before_ok)
        || (sys.in_set_count > 0 && !sets_ok)
        || (sys.after_weak_count > 0 && !aw_ok)
        || (sys.before_weak_count > 0 && !bw_ok)
        || (sys.ambiguous_with_count > 0 && !amb_ok)
    {
        assert_eq!(sys_res, FfiResult::NULL_POINTER);
    } else if !sys.schedule.is_valid() {
        assert_eq!(sys_res, FfiResult::INVALID_ARGUMENT);
    } else {
        assert_eq!(sys_res, FfiResult::OK);
        assert!(sys.schedule.0 <= 5 || sys.schedule.0 >= ScheduleId::CUSTOM_BASE);
    }

    // 4. BamObserverDesc::validate (exact error-code bi-implication)
    let obs_cb_ok: bool = kani::any();
    let obs_mut_ok: bool = kani::any();
    let obs_watched_ok: bool = kani::any();
    let obs = BamObserverDesc {
        struct_size: kani::any(),
        flags: kani::any(),
        name: core::ptr::null(),
        event: BamComponent(kani::any()),
        entity_lo: kani::any(),
        entity_hi: kani::any(),
        reserved0: kani::any(),
        callback: if obs_cb_ok { Some(stub_obs) } else { None },
        user_data: core::ptr::null_mut(),
        event_kind: BamEventKind(kani::any()),
        reserved1: kani::any(),
        watched_components: if obs_watched_ok {
            &raw const dummy_comp
        } else {
            core::ptr::null()
        },
        watched_component_count: kani::any(),
        mut_callback: if obs_mut_ok { Some(stub_obs_mut) } else { None },
        propagate_relationship: BamComponent(kani::any()),
        reserved2: kani::any(),
    };
    let obs_res = obs.validate();
    if (obs.struct_size as usize) < BamObserverDesc::MIN_V0_2_SIZE {
        assert_eq!(obs_res, FfiResult::INVALID_VERSION);
    } else if (obs.flags & !BAM_OBSERVER_KNOWN_FLAGS) != 0 {
        assert_eq!(obs_res, FfiResult::UNSUPPORTED);
    } else if (obs.reserved0 | obs.reserved1 | obs.reserved2) != 0 || !obs.event_kind.is_valid() {
        assert_eq!(obs_res, FfiResult::INVALID_ARGUMENT);
    } else if obs.watched_component_count > 0 && !obs_watched_ok {
        assert_eq!(obs_res, FfiResult::NULL_POINTER);
    } else if (obs.event_kind.is_custom() && !obs.event.is_valid())
        || (obs.event_kind.is_component_lifecycle()
            && !obs.event.is_valid()
            && obs.watched_component_count == 0)
    {
        assert_eq!(obs_res, FfiResult::INVALID_COMPONENT);
    } else if !obs_cb_ok && !obs_mut_ok {
        assert_eq!(obs_res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(obs_res, FfiResult::OK);
    }

    // 5. BamTriggerDesc::validate (exact error-code bi-implication)
    let trig_data_ok: bool = kani::any();
    let trig_watched_ok: bool = kani::any();
    let trig = BamTriggerDesc {
        struct_size: kani::any(),
        flags: kani::any(),
        event: BamComponent(kani::any()),
        event_kind: BamEventKind(kani::any()),
        entity_lo: kani::any(),
        entity_hi: kani::any(),
        data_ptr: if trig_data_ok {
            &raw mut dummy_byte
        } else {
            core::ptr::null_mut()
        },
        data_len: kani::any(),
        watched_components: if trig_watched_ok {
            &raw const dummy_comp
        } else {
            core::ptr::null()
        },
        watched_component_count: kani::any(),
        propagate_relationship: BamComponent(kani::any()),
        reserved0: kani::any(),
    };
    let trig_res = trig.validate();
    if (trig.struct_size as usize) < BamTriggerDesc::MIN_V0_2_SIZE {
        assert_eq!(trig_res, FfiResult::INVALID_VERSION);
    } else if (trig.flags & !BAM_TRIGGER_KNOWN_FLAGS) != 0 {
        assert_eq!(trig_res, FfiResult::UNSUPPORTED);
    } else if trig.reserved0 != 0 || !trig.event_kind.is_valid() {
        assert_eq!(trig_res, FfiResult::INVALID_ARGUMENT);
    } else if (trig.data_len > 0 && !trig_data_ok)
        || (trig.watched_component_count > 0 && !trig_watched_ok)
    {
        assert_eq!(trig_res, FfiResult::NULL_POINTER);
    } else if (trig.event_kind.is_custom() && !trig.event.is_valid())
        || (trig.event_kind.is_component_lifecycle()
            && !trig.event.is_valid()
            && trig.watched_component_count == 0)
    {
        assert_eq!(trig_res, FfiResult::INVALID_COMPONENT);
    } else {
        assert_eq!(trig_res, FfiResult::OK);
    }

    // 6. FfiAssetHandle::validate & index-bit roundtrip
    let handle = FfiAssetHandle {
        kind: kani::any(),
        reserved0: kani::any(),
        asset_type: BamComponent(kani::any()),
        index: kani::any(),
        generation: kani::any(),
        uuid: ComponentUuid::from_bytes(kani::any()),
    };
    let h_res = handle.validate();
    if handle.reserved0 != [0, 0, 0] {
        assert_eq!(h_res, FfiResult::INVALID_ARGUMENT);
    } else if handle.kind == FfiAssetHandle::KIND_NONE {
        assert_eq!(h_res, FfiResult::OK);
        assert!(handle.is_none() && !handle.is_some());
    } else if handle.kind == FfiAssetHandle::KIND_INDEX || handle.kind == FfiAssetHandle::KIND_UUID
    {
        assert!(handle.is_some() && !handle.is_none());
        if handle.asset_type.is_valid() {
            assert_eq!(h_res, FfiResult::OK);
        } else {
            assert_eq!(h_res, FfiResult::INVALID_COMPONENT);
        }
    } else {
        assert_eq!(h_res, FfiResult::INVALID_ARGUMENT);
    }

    let bits: u64 = kani::any();
    let indexed = FfiAssetHandle::from_index_bits(BamComponent::from_raw(1), bits);
    assert_eq!(indexed.to_index_bits(), Some(bits));
}

// ============================================================================
// PO-W6: `BamHostApi` 68-Slot Offset Table & `FfiResult` Totality
// ============================================================================

/// PO-W6: All 68 `BamHostApi` slot offsets satisfy `OFF64_k == 8 + 8*k`, `OFF32_k == 8 + 4*k`,
/// `SIZE_64 == 552`, `SIZE_32 == 280`, matching native `offset_of!`, and `FfiResult(x).name()`
/// is total and panic-free over all `x in 0..=u32::MAX`.
#[kani::proof]
fn po_w6_offsets_and_ffi_result_totality() {
    assert_eq!(BAM_HOST_API_SLOT_COUNT, 68);
    assert_eq!(offsets::SLOT_COUNT, 68);
    assert_eq!(offsets::SIZE_64, 552);
    assert_eq!(offsets::SIZE_32, 280);
    assert_eq!(offsets::STRUCT_SIZE, 0);
    assert_eq!(offsets::ABI_VERSION, 4);

    let slots: [(usize, usize, usize); BAM_HOST_API_SLOT_COUNT] = [
        (
            offsets::GET_PROC_ADDRESS,
            offsets::OFF64_GET_PROC_ADDRESS,
            offsets::OFF32_GET_PROC_ADDRESS,
        ),
        (
            offsets::LAST_ERROR,
            offsets::OFF64_LAST_ERROR,
            offsets::OFF32_LAST_ERROR,
        ),
        (
            offsets::LOAD_HOST_CTX,
            offsets::OFF64_LOAD_HOST_CTX,
            offsets::OFF32_LOAD_HOST_CTX,
        ),
        (
            offsets::REGISTER_COMPONENT,
            offsets::OFF64_REGISTER_COMPONENT,
            offsets::OFF32_REGISTER_COMPONENT,
        ),
        (
            offsets::RESOLVE_COMPONENT,
            offsets::OFF64_RESOLVE_COMPONENT,
            offsets::OFF32_RESOLVE_COMPONENT,
        ),
        (
            offsets::REGISTER_SYSTEM,
            offsets::OFF64_REGISTER_SYSTEM,
            offsets::OFF32_REGISTER_SYSTEM,
        ),
        (
            offsets::SYS_HOST_CTX,
            offsets::OFF64_SYS_HOST_CTX,
            offsets::OFF32_SYS_HOST_CTX,
        ),
        (
            offsets::SYS_QUERY,
            offsets::OFF64_SYS_QUERY,
            offsets::OFF32_SYS_QUERY,
        ),
        (
            offsets::SYS_COMMANDS,
            offsets::OFF64_SYS_COMMANDS,
            offsets::OFF32_SYS_COMMANDS,
        ),
        (
            offsets::ITER_NEXT_CHUNK,
            offsets::OFF64_ITER_NEXT_CHUNK,
            offsets::OFF32_ITER_NEXT_CHUNK,
        ),
        (
            offsets::ITER_RESET,
            offsets::OFF64_ITER_RESET,
            offsets::OFF32_ITER_RESET,
        ),
        (
            offsets::ITER_ENTITIES,
            offsets::OFF64_ITER_ENTITIES,
            offsets::OFF32_ITER_ENTITIES,
        ),
        (
            offsets::ITER_COLUMN_READONLY,
            offsets::OFF64_ITER_COLUMN_READONLY,
            offsets::OFF32_ITER_COLUMN_READONLY,
        ),
        (
            offsets::ITER_COLUMN_MUTABLE,
            offsets::OFF64_ITER_COLUMN_MUTABLE,
            offsets::OFF32_ITER_COLUMN_MUTABLE,
        ),
        (
            offsets::ITER_COLUMN_MUTABLE_UNTRACKED,
            offsets::OFF64_ITER_COLUMN_MUTABLE_UNTRACKED,
            offsets::OFF32_ITER_COLUMN_MUTABLE_UNTRACKED,
        ),
        (
            offsets::ITER_MARK_CHANGED_RANGE,
            offsets::OFF64_ITER_MARK_CHANGED_RANGE,
            offsets::OFF32_ITER_MARK_CHANGED_RANGE,
        ),
        (
            offsets::ITER_COLUMN_ROWS_READONLY,
            offsets::OFF64_ITER_COLUMN_ROWS_READONLY,
            offsets::OFF32_ITER_COLUMN_ROWS_READONLY,
        ),
        (
            offsets::ITER_COLUMN_ROWS_MUTABLE,
            offsets::OFF64_ITER_COLUMN_ROWS_MUTABLE,
            offsets::OFF32_ITER_COLUMN_ROWS_MUTABLE,
        ),
        (
            offsets::ITER_COLUMN_INFO,
            offsets::OFF64_ITER_COLUMN_INFO,
            offsets::OFF32_ITER_COLUMN_INFO,
        ),
        (
            offsets::RES_PTR,
            offsets::OFF64_RES_PTR,
            offsets::OFF32_RES_PTR,
        ),
        (
            offsets::RES_MUT_PTR,
            offsets::OFF64_RES_MUT_PTR,
            offsets::OFF32_RES_MUT_PTR,
        ),
        (
            offsets::CMD_SPAWN_EMPTY,
            offsets::OFF64_CMD_SPAWN_EMPTY,
            offsets::OFF32_CMD_SPAWN_EMPTY,
        ),
        (
            offsets::CMD_SPAWN,
            offsets::OFF64_CMD_SPAWN,
            offsets::OFF32_CMD_SPAWN,
        ),
        (
            offsets::CMD_DESPAWN,
            offsets::OFF64_CMD_DESPAWN,
            offsets::OFF32_CMD_DESPAWN,
        ),
        (
            offsets::CMD_INSERT,
            offsets::OFF64_CMD_INSERT,
            offsets::OFF32_CMD_INSERT,
        ),
        (
            offsets::CMD_INSERT_DEFAULT,
            offsets::OFF64_CMD_INSERT_DEFAULT,
            offsets::OFF32_CMD_INSERT_DEFAULT,
        ),
        (
            offsets::CMD_REMOVE,
            offsets::OFF64_CMD_REMOVE,
            offsets::OFF32_CMD_REMOVE,
        ),
        (
            offsets::PAR_FOR_EACH_CHUNK,
            offsets::OFF64_PAR_FOR_EACH_CHUNK,
            offsets::OFF32_PAR_FOR_EACH_CHUNK,
        ),
        (
            offsets::SYS_ENTITY_GET,
            offsets::OFF64_SYS_ENTITY_GET,
            offsets::OFF32_SYS_ENTITY_GET,
        ),
        (
            offsets::SYS_ENTITY_GET_MUT,
            offsets::OFF64_SYS_ENTITY_GET_MUT,
            offsets::OFF32_SYS_ENTITY_GET_MUT,
        ),
        (
            offsets::MSG_READ_BATCH,
            offsets::OFF64_MSG_READ_BATCH,
            offsets::OFF32_MSG_READ_BATCH,
        ),
        (
            offsets::MSG_SEND,
            offsets::OFF64_MSG_SEND,
            offsets::OFF32_MSG_SEND,
        ),
        (
            offsets::MSG_WRITE_BATCH,
            offsets::OFF64_MSG_WRITE_BATCH,
            offsets::OFF32_MSG_WRITE_BATCH,
        ),
        (
            offsets::RES_TICKS,
            offsets::OFF64_RES_TICKS,
            offsets::OFF32_RES_TICKS,
        ),
        (
            offsets::SYS_RES_PTR,
            offsets::OFF64_SYS_RES_PTR,
            offsets::OFF32_SYS_RES_PTR,
        ),
        (
            offsets::SYS_RES_MUT_PTR,
            offsets::OFF64_SYS_RES_MUT_PTR,
            offsets::OFF32_SYS_RES_MUT_PTR,
        ),
        (
            offsets::REGISTER_OBSERVER,
            offsets::OFF64_REGISTER_OBSERVER,
            offsets::OFF32_REGISTER_OBSERVER,
        ),
        (
            offsets::CMD_TRIGGER,
            offsets::OFF64_CMD_TRIGGER,
            offsets::OFF32_CMD_TRIGGER,
        ),
        (
            offsets::CMD_SPAWN_PERSISTENT,
            offsets::OFF64_CMD_SPAWN_PERSISTENT,
            offsets::OFF32_CMD_SPAWN_PERSISTENT,
        ),
        (
            offsets::CMD_SPAWN_BATCH,
            offsets::OFF64_CMD_SPAWN_BATCH,
            offsets::OFF32_CMD_SPAWN_BATCH,
        ),
        (
            offsets::CMD_DESPAWN_ALL,
            offsets::OFF64_CMD_DESPAWN_ALL,
            offsets::OFF32_CMD_DESPAWN_ALL,
        ),
        (
            offsets::CMD_INSERT_IF_NEW,
            offsets::OFF64_CMD_INSERT_IF_NEW,
            offsets::OFF32_CMD_INSERT_IF_NEW,
        ),
        (
            offsets::CMD_TRIGGER_EX,
            offsets::OFF64_CMD_TRIGGER_EX,
            offsets::OFF32_CMD_TRIGGER_EX,
        ),
        (
            offsets::CMD_OBSERVE,
            offsets::OFF64_CMD_OBSERVE,
            offsets::OFF32_CMD_OBSERVE,
        ),
        (
            offsets::ITER_READ_REMOVED,
            offsets::OFF64_ITER_READ_REMOVED,
            offsets::OFF32_ITER_READ_REMOVED,
        ),
        (
            offsets::SYS_ENTITY_CHILDREN,
            offsets::OFF64_SYS_ENTITY_CHILDREN,
            offsets::OFF32_SYS_ENTITY_CHILDREN,
        ),
        (
            offsets::ASSET_LOAD,
            offsets::OFF64_ASSET_LOAD,
            offsets::OFF32_ASSET_LOAD,
        ),
        (
            offsets::ASSET_GET_LOAD_STATE,
            offsets::OFF64_ASSET_GET_LOAD_STATE,
            offsets::OFF32_ASSET_GET_LOAD_STATE,
        ),
        (
            offsets::ASSET_RELEASE,
            offsets::OFF64_ASSET_RELEASE,
            offsets::OFF32_ASSET_RELEASE,
        ),
        (
            offsets::CMD_SPAWN_SCENE,
            offsets::OFF64_CMD_SPAWN_SCENE,
            offsets::OFF32_CMD_SPAWN_SCENE,
        ),
        (
            offsets::LOG_DIAGNOSTIC,
            offsets::OFF64_LOG_DIAGNOSTIC,
            offsets::OFF32_LOG_DIAGNOSTIC,
        ),
        (
            offsets::REPORT_GUEST_FAULT,
            offsets::OFF64_REPORT_GUEST_FAULT,
            offsets::OFF32_REPORT_GUEST_FAULT,
        ),
        (
            offsets::RESOLVE_COMPONENT_BY_UUID,
            offsets::OFF64_RESOLVE_COMPONENT_BY_UUID,
            offsets::OFF32_RESOLVE_COMPONENT_BY_UUID,
        ),
        (
            offsets::COMPONENT_LAYOUT_INFO,
            offsets::OFF64_COMPONENT_LAYOUT_INFO,
            offsets::OFF32_COMPONENT_LAYOUT_INFO,
        ),
        (
            offsets::REGISTER_RELATION,
            offsets::OFF64_REGISTER_RELATION,
            offsets::OFF32_REGISTER_RELATION,
        ),
        (
            offsets::INTERN_SYMBOL,
            offsets::OFF64_INTERN_SYMBOL,
            offsets::OFF32_INTERN_SYMBOL,
        ),
        (
            offsets::BIND_NAMED_OBJECT,
            offsets::OFF64_BIND_NAMED_OBJECT,
            offsets::OFF32_BIND_NAMED_OBJECT,
        ),
        (
            offsets::ITER_COUNT,
            offsets::OFF64_ITER_COUNT,
            offsets::OFF32_ITER_COUNT,
        ),
        (
            offsets::ITER_UP_CASCADE,
            offsets::OFF64_ITER_UP_CASCADE,
            offsets::OFF32_ITER_UP_CASCADE,
        ),
        (
            offsets::PAR_FOR_EACH_CHUNK_READONLY,
            offsets::OFF64_PAR_FOR_EACH_CHUNK_READONLY,
            offsets::OFF32_PAR_FOR_EACH_CHUNK_READONLY,
        ),
        (
            offsets::REGISTER_SYSTEM_HANDLE,
            offsets::OFF64_REGISTER_SYSTEM_HANDLE,
            offsets::OFF32_REGISTER_SYSTEM_HANDLE,
        ),
        (
            offsets::CONFIGURE_SCHEDULE_SET,
            offsets::OFF64_CONFIGURE_SCHEDULE_SET,
            offsets::OFF32_CONFIGURE_SCHEDULE_SET,
        ),
        (
            offsets::MOUNT_SYSTEM_IN_SCHEDULE,
            offsets::OFF64_MOUNT_SYSTEM_IN_SCHEDULE,
            offsets::OFF32_MOUNT_SYSTEM_IN_SCHEDULE,
        ),
        (
            offsets::CMD_CHECKPOINT,
            offsets::OFF64_CMD_CHECKPOINT,
            offsets::OFF32_CMD_CHECKPOINT,
        ),
        (
            offsets::CMD_ROLLBACK,
            offsets::OFF64_CMD_ROLLBACK,
            offsets::OFF32_CMD_ROLLBACK,
        ),
        (
            offsets::CMD_INSERT_RELATION,
            offsets::OFF64_CMD_INSERT_RELATION,
            offsets::OFF32_CMD_INSERT_RELATION,
        ),
        (
            offsets::CMD_REMOVE_RELATION,
            offsets::OFF64_CMD_REMOVE_RELATION,
            offsets::OFF32_CMD_REMOVE_RELATION,
        ),
        (
            offsets::MIGRATE_COMPONENT_COLUMN,
            offsets::OFF64_MIGRATE_COMPONENT_COLUMN,
            offsets::OFF32_MIGRATE_COMPONENT_COLUMN,
        ),
    ];

    let k: usize = kani::any();
    kani::assume(k < BAM_HOST_API_SLOT_COUNT);
    let (native, off64, off32) = slots[k];
    assert_eq!(off64, 8 + 8 * k);
    assert_eq!(off32, 8 + 4 * k);
    assert_eq!(native, 8 + size_of::<usize>() * k);
    assert_eq!(
        BamHostApi::MIN_V0_2_SIZE,
        offset_of!(BamHostApi, cmd_remove) + size_of::<usize>()
    );
    assert_eq!(BamHostApi::MIN_V0_3_SIZE, size_of::<BamHostApi>());

    // FfiResult totality over all 2^32 u32 values.
    let raw: u32 = kani::any();
    let res = FfiResult(raw);
    assert_eq!(res.is_ok(), raw == 0);
    assert_eq!(res.is_err(), raw != 0);
    assert_ne!(res.is_ok(), res.is_err());
    let label = res.name();
    let bytes = label.as_bytes();
    assert!(!bytes.is_empty());
    let is_unknown_label = bytes.len() == 7
        && bytes[0] == b'U'
        && bytes[1] == b'N'
        && bytes[2] == b'K'
        && bytes[3] == b'N'
        && bytes[4] == b'O'
        && bytes[5] == b'W'
        && bytes[6] == b'N';
    let is_known = raw <= 16 || raw == 99;
    assert_eq!(!is_unknown_label, is_known);

    // Totality and panic-freedom of all remaining `u32` wire enum-like newtypes over `0..=u32::MAX`.
    assert!(!AccessMode(raw).name().is_empty());
    assert_eq!(AccessMode(raw).is_valid(), raw <= 14);
    assert!(!ScheduleId(raw).name().is_empty());
    assert_eq!(
        ScheduleId(raw).is_valid(),
        raw <= 5 || raw >= ScheduleId::CUSTOM_BASE
    );
    assert!(!BamEventKind(raw).name().is_empty());
    assert_eq!(BamEventKind(raw).is_valid(), raw <= 6);
    assert!(!BamFieldKind(raw).name().is_empty());
    assert_eq!(BamFieldKind(raw).is_valid(), raw <= 18);
    assert!(!FfiAssetLoadState(raw).name().is_empty());
    assert_eq!(FfiAssetLoadState(raw).is_valid(), raw <= 3);
}

// ============================================================================
// PO-W7: `FfiSlice<T>` & `StrView` Unsafe Conversion, Null/Alignment/Overflow Safety
// ============================================================================

/// PO-W7: `FfiSlice<T>` handles empty C slices `(NULL, 0)` safely via `NonNull::dangling()`,
/// rejects non-empty null pointers, rejects misaligned pointers, rejects byte-length overflow /
/// `> isize::MAX`, and safely yields valid Rust slices for aligned non-empty and ZST inputs.
#[kani::proof]
#[kani::unwind(8)]
fn po_w7_ffi_slice_and_str_view_soundness() {
    // 1. Empty C slice (NULL, 0) is always valid and returns &[] without UB.
    let empty_u32 = FfiSlice::<u32> {
        ptr: core::ptr::null(),
        len: 0,
    };
    assert_eq!(empty_u32.validate(), FfiResult::OK);
    // SAFETY: `len == 0` substitutes `NonNull::dangling()`.
    assert_eq!(unsafe { empty_u32.try_as_slice() }, Ok(&[][..]));

    // 2. Arbitrary non-empty null slice always fails with NULL_POINTER without dereferencing.
    let nonzero_len: usize = kani::any();
    kani::assume(nonzero_len > 0);
    let null_u32 = FfiSlice::<u32> {
        ptr: core::ptr::null(),
        len: nonzero_len,
    };
    assert_eq!(null_u32.validate(), FfiResult::NULL_POINTER);
    // SAFETY: `validate()` rejects null before forming any slice.
    assert_eq!(
        unsafe { null_u32.try_as_slice() },
        Err(FfiResult::NULL_POINTER)
    );

    // 3. Misaligned non-null pointer always fails with INVALID_ARGUMENT without dereferencing.
    let storage = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<u64>());
    let bad_ptr = core::ptr::from_ref(&storage)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<u64>();
    let bad_len: usize = kani::any();
    let unaligned = FfiSlice::<u64> {
        ptr: bad_ptr,
        len: bad_len,
    };
    assert_eq!(unaligned.validate(), FfiResult::INVALID_ARGUMENT);
    // SAFETY: `validate()` rejects misaligned pointer before forming any slice.
    assert_eq!(
        unsafe { unaligned.try_as_slice() },
        Err(FfiResult::INVALID_ARGUMENT)
    );

    // 4. Overflowing / > isize::MAX byte length always fails with INVALID_ARGUMENT.
    let huge_len: usize = kani::any();
    kani::assume(
        huge_len
            .checked_mul(size_of::<u64>())
            .is_none_or(|b| b > isize::MAX as usize),
    );
    let huge = FfiSlice::<u64> {
        ptr: storage.as_ptr(),
        len: huge_len,
    };
    assert_eq!(huge.validate(), FfiResult::INVALID_ARGUMENT);
    // SAFETY: `validate()` rejects excessive byte length before forming any slice.
    assert_eq!(
        unsafe { huge.try_as_slice() },
        Err(FfiResult::INVALID_ARGUMENT)
    );

    // 5. Valid aligned slice roundtrip for arbitrary bounded length 0..=4.
    let data: [u32; 4] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= data.len());
    let valid_slice = FfiSlice::from_slice(&data[..len]);
    assert_eq!(valid_slice.validate(), FfiResult::OK);
    // SAFETY: `valid_slice` borrows `data[..len]` on the local stack.
    let out = unsafe { valid_slice.try_as_slice() }.expect("valid slice must succeed");
    assert_eq!(out.len(), len);
    if len > 0 {
        let i: usize = kani::any();
        kani::assume(i < len);
        assert_eq!(out[i], data[i]);
    }

    // 6. ZST FfiSlice<()>: null with len > 0 rejected, non-null dangling with arbitrary len valid.
    let zst_null_nz = FfiSlice::<()> {
        ptr: core::ptr::null(),
        len: nonzero_len,
    };
    assert_eq!(zst_null_nz.validate(), FfiResult::NULL_POINTER);
    let zst_len: usize = kani::any();
    let zst_valid = FfiSlice::<()> {
        ptr: core::ptr::NonNull::<()>::dangling().as_ptr(),
        len: zst_len,
    };
    assert_eq!(zst_valid.validate(), FfiResult::OK);
    // SAFETY: `()` is zero-sized and `ptr` is `NonNull::dangling()`.
    let zst_out =
        unsafe { zst_valid.try_as_slice() }.expect("ZST slice with dangling ptr is valid");
    assert_eq!(zst_out.len(), zst_len);

    // 7. StrView UTF-8 validation: panic-free on arbitrary 3-byte buffers, accepts ASCII,
    //    and rejects invalid continuation/overlong lead bytes with INVALID_UTF8.
    let raw_bytes: [u8; 3] = kani::any();
    let str_len: usize = kani::any();
    kani::assume(str_len <= 3);
    let view = StrView {
        ptr: raw_bytes.as_ptr(),
        len: str_len,
    };
    // SAFETY: `raw_bytes[..str_len]` is initialized stack storage.
    match unsafe { view.try_as_str() } {
        Ok(s) => {
            assert_eq!(s.len(), str_len);
            if str_len > 0 {
                let idx: usize = kani::any();
                kani::assume(idx < str_len);
                assert_eq!(s.as_bytes()[idx], raw_bytes[idx]);
                // First byte can never be an invalid UTF-8 lead byte.
                assert!(!(0x80..=0xBF).contains(&raw_bytes[0]));
                assert!(raw_bytes[0] < 0xF5);
            }
        }
        Err(err) => {
            assert_eq!(err, FfiResult::INVALID_UTF8);
            // Must contain at least one non-ASCII byte.
            assert!(
                (str_len >= 1 && raw_bytes[0] >= 0x80)
                    || (str_len >= 2 && raw_bytes[1] >= 0x80)
                    || (str_len >= 3 && raw_bytes[2] >= 0x80)
            );
        }
    }
}

// ============================================================================
// PO-W8: `FfiOption<T>::none`, `BamChangeTicks`, `ComponentUuid::try_parse_hex`
// ============================================================================

/// PO-W8 (`FfiOption<T>`): `FfiOption::none()` (`MaybeUninit::zeroed().assume_init()`) is sound
/// across fixed-width and composite `AbiPod` types and roundtrips `Option<T>` losslessly.
#[kani::proof]
fn po_w8_ffi_option_zeroed_none_and_roundtrip() {
    let opt_u32: Option<u32> = kani::any();
    let ffi_u32 = FfiOption::from_option(opt_u32);
    assert_eq!(ffi_u32.is_some(), opt_u32.is_some());
    assert_eq!(ffi_u32.is_none(), opt_u32.is_none());
    assert_eq!(ffi_u32.into_option(), opt_u32);

    let none_entity = FfiOption::<FfiEntity>::none();
    assert!(none_entity.is_none());
    assert_eq!(none_entity.value, FfiEntity::NULL);

    let none_uuid = FfiOption::<ComponentUuid>::none();
    assert!(none_uuid.is_none());
    assert!(none_uuid.value.is_zero());

    let none_asset = FfiOption::<FfiAssetHandle>::none();
    assert!(none_asset.is_none());
    assert_eq!(none_asset.value, FfiAssetHandle::NONE);
    assert_eq!(none_asset.value.validate(), FfiResult::OK);

    let none_nested = FfiOption::<FfiOption<u64>>::none();
    assert!(none_nested.is_none());
    assert!(none_nested.value.is_none());

    // Arbitrary non-zero tag byte in `FfiBool` always decodes to `Some` without UB.
    let raw_tag: u8 = kani::any();
    let payload: u32 = kani::any();
    let raw_opt = FfiOption {
        is_some: FfiBool(raw_tag),
        value: payload,
    };
    assert_eq!(raw_opt.into_option(), (raw_tag != 0).then_some(payload));
}

/// PO-W8 (`BamChangeTicks`): Wraparound-safe change detection is panic-free over all
/// `(tick, last_run, this_run) in u32^3`, irreflexive (`tick == last_run => false`), and respects
/// `BAM_MAX_CHANGE_AGE` clamping.
#[kani::proof]
fn po_w8_change_ticks_wraparound_safety() {
    let tick: u32 = kani::any();
    let last_run: u32 = kani::any();
    let this_run: u32 = kani::any();

    // Irreflexive: a tick equal to last_run is never strictly newer.
    assert!(!BamChangeTicks::is_tick_newer(last_run, last_run, this_run));

    // Exact specification matching clamped wrapping distance to this_run.
    let d_tick = this_run.wrapping_sub(tick).min(BAM_MAX_CHANGE_AGE);
    let d_last = this_run.wrapping_sub(last_run).min(BAM_MAX_CHANGE_AGE);
    assert_eq!(
        BamChangeTicks::is_tick_newer(tick, last_run, this_run),
        d_last > d_tick
    );

    // If tick == this_run and last_run != this_run (and within non-zero distance), tick is newer.
    if last_run != this_run {
        assert!(BamChangeTicks::is_tick_newer(this_run, last_run, this_run));
    }

    let ticks = BamChangeTicks {
        added: tick,
        changed: tick,
        last_run,
        this_run,
    };
    assert_eq!(ticks.is_added(), ticks.is_changed());

    // Handle sentinels and FfiAssetLoadState totality.
    let s: u32 = kani::any();
    assert_eq!(BamSystem::from_raw(s).is_valid(), s != u32::MAX);
    assert_eq!(BamSystemSet::from_raw(s).is_valid(), s != u32::MAX);
    assert_eq!(BamSymbol::from_raw(s).is_valid(), s != u32::MAX);
    assert_eq!(FfiAssetLoadState(s).is_valid(), s <= 3);
}

/// PO-W8 (`ComponentUuid`): `from_u128`/`to_u128` bijection, exact hex round-trip parsing for all
/// 16-byte UUIDs in both 32-char and 36-char (upper/lower) hex forms, and panic-freedom over
/// arbitrary 32-byte/36-byte ASCII and multi-byte non-ASCII UTF-8 strings.
#[kani::proof]
#[kani::unwind(38)]
fn po_w8_component_uuid_bijection_and_hex_parser() {
    let raw: u128 = kani::any();
    let uuid = ComponentUuid::from_u128(raw);
    assert_eq!(uuid.to_u128(), raw);
    assert_eq!(uuid.is_zero(), raw == 0);

    // Exact round-trip bijection for arbitrary symbolic 16 bytes in 32-hex and 36-hex forms.
    let sym_bytes: [u8; 16] = kani::any();
    let uppercase: bool = kani::any();
    let nibble_to_hex = |n: u8| -> u8 {
        if n < 10 {
            b'0' + n
        } else if uppercase {
            b'A' + (n - 10)
        } else {
            b'a' + (n - 10)
        }
    };
    let mut enc32 = [0u8; 32];
    let mut enc36 = [0u8; 36];
    let mut b_idx = 0usize;
    let mut p36 = 0usize;
    while b_idx < 16 {
        if p36 == 8 || p36 == 13 || p36 == 18 || p36 == 23 {
            enc36[p36] = b'-';
            p36 += 1;
        }
        let hi = nibble_to_hex(sym_bytes[b_idx] >> 4);
        let lo = nibble_to_hex(sym_bytes[b_idx] & 0x0F);
        enc32[b_idx * 2] = hi;
        enc32[b_idx * 2 + 1] = lo;
        enc36[p36] = hi;
        enc36[p36 + 1] = lo;
        p36 += 2;
        b_idx += 1;
    }
    // SAFETY: Every byte in `enc32`/`enc36` is ASCII hex or `'-'`, hence valid UTF-8.
    let hex32_str = unsafe { core::str::from_utf8_unchecked(&enc32) };
    // SAFETY: Same as above for `enc36`.
    let hex36_str = unsafe { core::str::from_utf8_unchecked(&enc36) };
    let expected_uuid = ComponentUuid::from_bytes(sym_bytes);
    assert_eq!(ComponentUuid::try_parse_hex(hex32_str), Some(expected_uuid));
    assert_eq!(ComponentUuid::try_parse_hex(hex36_str), Some(expected_uuid));

    // Panic-freedom and strict hyphen enforcement on arbitrary 36-byte ASCII strings.
    let mut buf36: [u8; 36] = kani::any();
    let mut i = 0;
    while i < 36 {
        buf36[i] &= 0x7F;
        i += 1;
    }
    // SAFETY: Every byte in `buf36` is masked to `< 0x80` (ASCII), hence valid UTF-8.
    let s36 = unsafe { core::str::from_utf8_unchecked(&buf36) };
    let parsed36 = ComponentUuid::try_parse_hex(s36);
    if buf36[8] != b'-' || buf36[13] != b'-' || buf36[18] != b'-' || buf36[23] != b'-' {
        assert!(parsed36.is_none());
    }

    // Panic-freedom on arbitrary 32-byte ASCII strings.
    let mut buf32: [u8; 32] = kani::any();
    let mut j = 0;
    while j < 32 {
        buf32[j] &= 0x7F;
        j += 1;
    }
    // SAFETY: Every byte in `buf32` is masked to `< 0x80` (ASCII), hence valid UTF-8.
    let s32 = unsafe { core::str::from_utf8_unchecked(&buf32) };
    let _ = ComponentUuid::try_parse_hex(s32);

    // Non-ASCII multi-byte UTF-8 strings of byte length 32 and 36 never panic and return None.
    assert!(ComponentUuid::try_parse_hex("éééééééééééééééé").is_none());
    assert!(ComponentUuid::try_parse_hex("éééé-éé-éé-éé-éééééé").is_none());
    assert!(ComponentUuid::try_parse_hex("€€€€€€€€€€€€").is_none());

    // Wrong lengths always return None.
    assert!(ComponentUuid::try_parse_hex("").is_none());
    assert!(ComponentUuid::try_parse_hex("0123456789abcdef").is_none());
}
