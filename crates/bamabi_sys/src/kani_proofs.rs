//! Kani formal verification proof harnesses for `bamabi_sys` (`PO-W1`..`PO-W4`, `PO-W6`..`PO-W8`).
//!
//! Discharges Layer 0 wire proof obligations codified in the Bamabi Formal Verification
//! Specification over full symbolic integer spaces (`u32`, `usize`) and all 13 `ExtensibleStruct`
//! wire types, plus every `unsafe` boundary primitive (`FfiSlice`, `StrView`, `FfiOption`,
//! `raw_struct_size`, `raw_abi_version`) in `bamabi_sys`.

use core::{
    ffi::c_void,
    mem::{align_of, size_of},
};

use crate::{
    ABI_VERSION, ABI_VERSION_MAJOR, ABI_VERSION_MINOR, ABI_VERSION_PATCH, AccessMode,
    BAM_MAX_CHANGE_AGE, BamChangeTicks, BamComponent, BamEventKind, BamFieldDesc, BamFieldKind,
    BamHostApi, BamLayoutInfo, BamManifest, BamObserverDesc, BamQueryDesc, BamRelationDesc,
    BamRequiredComponent, BamScheduleSetDesc, BamSymbol, BamSystem, BamSystemDesc, BamSystemSet,
    BamTriggerDesc, ComponentDesc, ComponentUuid, ExtensibleStruct, FfiAssetHandle,
    FfiAssetLoadState, FfiBool, FfiEntity, FfiOption, FfiResult, FfiSlice, ScheduleId, StrView,
    VTableHeader, abi_version_is_compatible, abi_version_major, abi_version_minor,
    abi_version_patch, pack_abi_version, read_descriptor, read_extensible_struct,
    unpack_abi_version,
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
    kani::cover!(compat);
    kani::cover!(!compat);
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
    kani::cover!(null_res.is_err());
    assert!(matches!(null_res, Err(FfiResult::NULL_POINTER)));
    // SAFETY: Null pointer is explicitly handled without dereferencing.
    let null_desc_res = unsafe { read_descriptor::<T>(core::ptr::null()) };
    kani::cover!(null_desc_res.is_err());
    assert!(matches!(null_desc_res, Err(FfiResult::NULL_POINTER)));

    // (b) struct_size < MIN always returns Err(INVALID_VERSION) without reading past 0 / 4 bytes.
    let small_size: usize = kani::any();
    kani::assume(small_size < MIN);
    let tiny = [0xAAu8; 1];
    // SAFETY: `small_size < T::MIN_V0_2_SIZE`, so `read_extensible_struct` rejects before copying.
    let under_res = unsafe { read_extensible_struct::<T>(tiny.as_ptr(), small_size) };
    kani::cover!(under_res.is_err());
    assert!(matches!(under_res, Err(FfiResult::INVALID_VERSION)));

    let small_u32: u32 = kani::any();
    kani::assume((small_u32 as usize) < MIN);
    let mut unaligned_hdr = [0xEEu8; 5];
    unaligned_hdr[1..5].copy_from_slice(&small_u32.to_ne_bytes());
    // SAFETY: Offset +1 provides 4 readable unaligned bytes for the leading `struct_size: u32`,
    // and `small_u32 < MIN` guarantees rejection before copying any further bytes.
    let under_desc =
        unsafe { read_descriptor::<T>(unaligned_hdr.as_ptr().add(1).cast::<c_void>()) };
    kani::cover!(under_desc.is_err());
    assert!(matches!(under_desc, Err(FfiResult::INVALID_VERSION)));

    // (c) & (d) Arbitrary valid prefix copy_len in MIN..=FULL with strict tail zero-extension.
    let src: [u8; FULL] = kani::any();
    let req_size: usize = kani::any();
    kani::assume(req_size >= MIN);
    let copy_len = if req_size < FULL { req_size } else { FULL };

    // SAFETY: `src` provides `FULL >= copy_len` initialized readable bytes.
    let req_res = unsafe { read_extensible_struct::<T>(src.as_ptr(), req_size) };
    kani::cover!(req_res.is_ok());
    kani::cover!(MIN == FULL || req_size < FULL);
    kani::cover!(req_size >= FULL);
    let val = req_res.expect("valid struct_size >= MIN_V0_2_SIZE must succeed");
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
    let old_res = unsafe { read_descriptor::<T>(old_buf.as_ptr().add(1).cast::<c_void>()) };
    kani::cover!(old_res.is_ok());
    let old_val = old_res.expect("MIN_V0_2_SIZE prefix must succeed");
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
    let future_res = unsafe { read_descriptor::<T>(future_buf.as_ptr().add(1).cast::<c_void>()) };
    kani::cover!(future_res.is_ok());
    kani::cover!((future_size as usize) > FULL);
    let future_val = future_res.expect("future struct_size >= FULL must truncate cleanly");
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
    let null_sz = unsafe { BamHostApi::raw_struct_size(core::ptr::null()) };
    kani::cover!(null_sz.is_none());
    assert_eq!(null_sz, None);
    // SAFETY: Null pointers are explicitly rejected before dereferencing.
    let null_abi = unsafe { BamHostApi::raw_abi_version(core::ptr::null()) };
    kani::cover!(null_abi.is_none());
    assert_eq!(null_abi, None);

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
    let sz4 = unsafe { BamHostApi::raw_struct_size(ptr4) };
    kani::cover!(sz4.is_some());
    assert_eq!(sz4, Some(small_sz));
    // SAFETY: `raw_abi_version` checks `struct_size < 8` before touching offset 4.
    let abi4 = unsafe { BamHostApi::raw_abi_version(ptr4) };
    kani::cover!(abi4.is_none());
    assert_eq!(abi4, None);

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
    let sz8 = unsafe { BamHostApi::raw_struct_size(ptr8) };
    kani::cover!(sz8.is_some());
    assert_eq!(sz8, Some(full_sz));
    // SAFETY: `ptr8` is 8-byte aligned and readable for 8 bytes (`struct_size >= 8`).
    let abi8 = unsafe { BamHostApi::raw_abi_version(ptr8) };
    kani::cover!(abi8.is_some());
    assert_eq!(abi8, Some(abi_ver));
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
    kani::cover!(expected);
    kani::cover!(!expected);
    kani::cover!(slot_offset.checked_add(slot_size).is_none());

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
// PO-W6: `FfiResult` & Wire Enum Newtype Totality
// ============================================================================

/// PO-W6: `FfiResult(x).name()` and all `u32` wire enum-like newtypes (`AccessMode`, `ScheduleId`,
/// `BamEventKind`, `BamFieldKind`, `FfiAssetLoadState`) are total and panic-free over all
/// `x in 0..=u32::MAX`.
#[kani::proof]
fn po_w6_offsets_and_ffi_result_totality() {
    // FfiResult totality over all 2^32 u32 values.
    let raw: u32 = kani::any();
    let res = FfiResult(raw);
    kani::cover!(res.is_ok());
    kani::cover!(res.is_err());
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
    kani::cover!(is_known);
    kani::cover!(!is_known);
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
    let empty_res = unsafe { empty_u32.try_as_slice() };
    kani::cover!(empty_res.is_ok());
    assert_eq!(empty_res, Ok(&[][..]));

    // 2. Arbitrary non-empty null slice always fails with NULL_POINTER without dereferencing.
    let nonzero_len: usize = kani::any();
    kani::assume(nonzero_len > 0);
    let null_u32 = FfiSlice::<u32> {
        ptr: core::ptr::null(),
        len: nonzero_len,
    };
    assert_eq!(null_u32.validate(), FfiResult::NULL_POINTER);
    // SAFETY: `validate()` rejects null before forming any slice.
    let null_res = unsafe { null_u32.try_as_slice() };
    kani::cover!(null_res.is_err());
    assert_eq!(null_res, Err(FfiResult::NULL_POINTER));

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
    let unaligned_res = unsafe { unaligned.try_as_slice() };
    kani::cover!(unaligned_res.is_err());
    assert_eq!(unaligned_res, Err(FfiResult::INVALID_ARGUMENT));

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
    let huge_res = unsafe { huge.try_as_slice() };
    kani::cover!(huge_res.is_err());
    assert_eq!(huge_res, Err(FfiResult::INVALID_ARGUMENT));

    // 5. Valid aligned slice roundtrip for arbitrary bounded length 0..=4.
    let data: [u32; 4] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= data.len());
    let valid_slice = FfiSlice::from_slice(&data[..len]);
    assert_eq!(valid_slice.validate(), FfiResult::OK);
    // SAFETY: `valid_slice` borrows `data[..len]` on the local stack.
    let valid_res = unsafe { valid_slice.try_as_slice() };
    kani::cover!(valid_res.is_ok());
    let out = valid_res.expect("valid slice must succeed");
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
    let zst_res = unsafe { zst_valid.try_as_slice() };
    kani::cover!(zst_res.is_ok());
    let zst_out = zst_res.expect("ZST slice with dangling ptr is valid");
    assert_eq!(zst_out.len(), zst_len);

    // 7. StrView null-pointer rejection and UTF-8 validation: panic-free on arbitrary 4-byte
    //    buffers (covering 4-byte UTF-8 sequences, surrogate range 0xED 0xA0..=0xBF, and
    //    > U+10FFFF 0xF4 0x90..), rejecting invalid UTF-8 with INVALID_UTF8.
    let null_str = StrView {
        ptr: core::ptr::null(),
        len: nonzero_len,
    };
    // SAFETY: `try_as_str` rejects non-empty null pointers before forming a slice.
    let null_str_res = unsafe { null_str.try_as_str() };
    kani::cover!(null_str_res.is_err());
    assert_eq!(null_str_res, Err(FfiResult::NULL_POINTER));

    let raw_bytes: [u8; 4] = kani::any();
    let str_len: usize = kani::any();
    kani::assume(str_len <= 4);
    let view = StrView {
        ptr: raw_bytes.as_ptr(),
        len: str_len,
    };
    // SAFETY: `raw_bytes[..str_len]` is initialized stack storage.
    let str_res = unsafe { view.try_as_str() };
    kani::cover!(str_res.is_ok());
    kani::cover!(str_res.is_err());
    match str_res {
        Ok(s) => {
            assert_eq!(s.len(), str_len);
            kani::cover!(str_len == 4 && raw_bytes[0] >= 0xF0);
            if str_len > 0 {
                let idx: usize = kani::any();
                kani::assume(idx < str_len);
                assert_eq!(s.as_bytes()[idx], raw_bytes[idx]);
                // First byte can never be an invalid UTF-8 lead byte or overlong C0/C1.
                assert!(!(0x80..=0xBF).contains(&raw_bytes[0]));
                assert!(!(0xC0..=0xC1).contains(&raw_bytes[0]));
                assert!(raw_bytes[0] < 0xF5);
            }
            // UTF-16 surrogate range (0xED 0xA0..=0xBF) is never accepted.
            if str_len >= 2 && raw_bytes[0] == 0xED {
                assert!(raw_bytes[1] < 0xA0);
            }
            // Code points > U+10FFFF (0xF4 0x90..) are never accepted.
            if str_len >= 2 && raw_bytes[0] == 0xF4 {
                assert!(raw_bytes[1] <= 0x8F);
            }
        }
        Err(err) => {
            assert_eq!(err, FfiResult::INVALID_UTF8);
            kani::cover!(
                str_len >= 3 && raw_bytes[0] == 0xED && (0xA0..=0xBF).contains(&raw_bytes[1])
            );
            kani::cover!(str_len == 4 && raw_bytes[0] == 0xF4 && raw_bytes[1] >= 0x90);
            // Must contain at least one non-ASCII byte.
            assert!(
                (str_len >= 1 && raw_bytes[0] >= 0x80)
                    || (str_len >= 2 && raw_bytes[1] >= 0x80)
                    || (str_len >= 3 && raw_bytes[2] >= 0x80)
                    || (str_len >= 4 && raw_bytes[3] >= 0x80)
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
    kani::cover!(ffi_u32.is_some());
    kani::cover!(ffi_u32.is_none());
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
    let decoded = raw_opt.into_option();
    kani::cover!(decoded.is_some());
    kani::cover!(decoded.is_none());
    kani::cover!(raw_tag > 1);
    assert_eq!(decoded, (raw_tag != 0).then_some(payload));
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
    let newer = BamChangeTicks::is_tick_newer(tick, last_run, this_run);
    kani::cover!(newer);
    kani::cover!(!newer);
    kani::cover!(newer && this_run < last_run);
    assert_eq!(newer, d_last > d_tick);

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
    kani::cover!(BamSystem::from_raw(s).is_valid());
    kani::cover!(!BamSystem::from_raw(s).is_valid());
    kani::cover!(FfiAssetLoadState(s).is_valid());
    kani::cover!(!FfiAssetLoadState(s).is_valid());
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
    kani::cover!(uuid.is_zero());
    kani::cover!(!uuid.is_zero());
    assert_eq!(uuid.to_u128(), raw);
    assert_eq!(uuid.is_zero(), raw == 0);

    // Exact round-trip bijection for arbitrary symbolic 16 bytes in 32-hex and 36-hex forms.
    let sym_bytes: [u8; 16] = kani::any();
    let uppercase: bool = kani::any();
    kani::cover!(uppercase);
    kani::cover!(!uppercase);
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
    kani::cover!(parsed36.is_some());
    kani::cover!(parsed36.is_none());
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
    let parsed32 = ComponentUuid::try_parse_hex(s32);
    kani::cover!(parsed32.is_some());
    kani::cover!(parsed32.is_none());

    // Non-ASCII multi-byte UTF-8 strings of byte length 32 and 36 never panic and return None.
    assert!(ComponentUuid::try_parse_hex("éééééééééééééééé").is_none());
    assert!(ComponentUuid::try_parse_hex("éééé-éé-éé-éé-éééééé").is_none());
    assert!(ComponentUuid::try_parse_hex("€€€€€€€€€€€€").is_none());

    // Wrong lengths always return None.
    assert!(ComponentUuid::try_parse_hex("").is_none());
    assert!(ComponentUuid::try_parse_hex("0123456789abcdef").is_none());
}
