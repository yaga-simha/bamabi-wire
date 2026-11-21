# `bamabi_sys` Test & Formal Verification Guide

This document inventories **every** unit test, property-based test, doctest, and Kani formal proof harness in `bamabi_sys`, explaining **why** each check exists and **what** invariant or bug class it proves.

---

## 1. Suite Overview & Commands

| Layer | File(s) | Count | Command |
|---|---|---|---|
| **Compile-time `const` assertions** | `src/access.rs`, `src/commands.rs`, `src/component.rs`, `src/host_api.rs`, `src/iter.rs`, `src/primitives/*.rs`, `src/system.rs` | Per-type blocks | `cargo check -p bamabi_sys` (`--target i686-unknown-linux-gnu`, `--target wasm32-unknown-unknown`) |
| **Unit tests** | [`src/host_api.rs`](src/host_api.rs), [`src/tests/mod.rs`](src/tests/mod.rs), [`src/tests/layout.rs`](src/tests/layout.rs) | 16 tests | `cargo test -p bamabi_sys --lib --all-features` |
| **Property-based tests** | [`tests/proptest_wire.rs`](tests/proptest_wire.rs) | 6 properties | `cargo test -p bamabi_sys --test proptest_wire` |
| **Doctests** | `src/lib.rs`, `src/access.rs`, `src/component.rs`, `src/primitives/*.rs`, `src/traits.rs`, `src/version.rs` | 13 passing + 5 `compile_fail` | `cargo test --doc -p bamabi_sys --all-features` |
| **Kani formal proofs** | [`src/kani_proofs.rs`](src/kani_proofs.rs) | 25 harnesses (`PO-W1`..`PO-W4`, `PO-W6`..`PO-W8`) | `cargo kani -p bamabi_sys` |
| **Miri UB checks** | All unit + property + doc tests (`proptest` capped at 4 cases under `cfg(miri)`) | — | `MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_sys` |

---

## 2. Unit Tests (`src/host_api.rs` & `src/tests/layout.rs`)

### `src/host_api.rs` (`offset_tests`)

1. **`host_api::offset_tests::host_api_offset_constants_cover_all_68_slots_monotonically`**
   - **Why:** JIT/AOT backends (such as `bql_jit`) and `bamabi_sdk_gen` rely on `bamabi_sys::offsets` (`OFF64_*`, `OFF32_*`, `<FIELD>`) instead of hardcoding magic numbers, and `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` must match exact slot boundaries.
   - **Proves (`PO-W6`):** `BAM_HOST_API_SLOT_COUNT == offsets::SLOT_COUNT == 68`, `SIZE_64 == 552`, `SIZE_32 == 280`, header fields sit at offsets `0` and `4`, every slot `k` in `0..68` satisfies `OFF64_k == 8 + 8*k` and `OFF32_k == 8 + 4*k` matching the native target offset, and `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` terminate immediately after `cmd_remove` (slot 26) and `migrate_component_column` (slot 67) respectively.

### `src/tests/layout.rs`

2. **`tests::layout::abi_v0_2_exact_sizes_alignments_and_offsets`**
   - **Why:** Rust compiler changes or accidental field reordering must never silently shift C-ABI struct layouts or break Rust's null-pointer niche optimization on `Option<unsafe extern "C" fn(...)>`.
   - **Proves:** Exact `size_of`, `align_of`, `offset_of!`, and `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` on both 64-bit and 32-bit targets for all primitive newtypes, `FfiAssetHandle`, `BamTermDesc`, `StrView`, `BamManifest`, `BamQueryDesc`, `BamSystemDesc`, `BamObserverDesc`, `BamTriggerDesc`, `ComponentDesc`, `BamColumnInfo`, `VTableHeader`, and `BamHostApi`, plus pointer-width equality for every `Option<unsafe extern "C" fn(...)>` callback typedef.

3. **`tests::layout::zero_major_and_post_one_semver_handshake`**
   - **Why:** Prevents silent ABI mismatch between host and guest libraries compiled against different `0.x` or `1.x+` revisions.
   - **Proves (`PO-W1`, `PO-W2`):** `ABI_VERSION == 0x0003_0000` (`(0, 3, 0)`), `pack_abi_version` / `unpack_abi_version` / `abi_version_{major,minor,patch}` round-trip cleanly, pre-1.0 (`major == 0`) requires exact `minor` match while ignoring `patch`, and post-1.0 (`major >= 1`) accepts `guest_minor <= host_minor` within the same `major` while rejecting newer guests or major mismatches.

4. **`tests::layout::pack_abi_version_rejects_major_overflow`**
   - **Why:** Prevents `(major << 24)` from silently wrapping/truncating `major > 255`.
   - **Proves (`PO-W1`):** `pack_abi_version(256, 0, 0)` panics with `"ABI major version exceeds 8-bit range"`.

5. **`tests::layout::pack_abi_version_rejects_minor_overflow`**
   - **Why:** Prevents `(minor << 16)` from silently corrupting `major` bits when `minor > 255`.
   - **Proves (`PO-W1`):** `pack_abi_version(0, 256, 0)` panics with `"ABI minor version exceeds 8-bit range"`.

6. **`tests::layout::pack_abi_version_rejects_patch_overflow`**
   - **Why:** Prevents `patch > 65535` from spilling into `minor` bits.
   - **Proves (`PO-W1`):** `pack_abi_version(0, 2, 65_536)` panics with `"ABI patch version exceeds 16-bit range"`.

7. **`tests::layout::ffi_result_taxonomy_and_display`**
   - **Why:** Foreign C/C++/Zig callers can return arbitrary `u32` values; `FfiResult` must expose stable numeric discriminants and format unknown codes without panicking or UB.
   - **Proves (`PO-W6`):** All 18 canonical `FfiResult` codes (`0..=16` and `99`) match their exact integer constants, `name()`, `is_ok()`/`is_err()`, and `Display` output, and foreign values like `FfiResult(777)` report `is_err() == true`, `name() == "UNKNOWN"`, and `Display == "UNKNOWN(777)"`.

8. **`tests::layout::extensible_struct_zero_extension_and_truncation`**
   - **Why:** Forward and backward ABI evolution depends on `read_descriptor` / `read_extensible_struct` never reading out of bounds or leaving uninitialized trailing slots.
   - **Proves (`PO-W3`, `PO-W4`, `PO-W5`):** Newer consumers reading an older `BamHostApi` zero-fill appended `Option<fn>` slots to `None` (`covers_slot == false`); older consumers reading a larger future struct truncate trailing bytes cleanly (`covers_slot == true`); undersized headers (`struct_size < MIN_V0_2_SIZE`) return `FfiResult::INVALID_VERSION` without OOB reads; `raw_struct_size` / `raw_abi_version` read minimal headers safely; null pointers return `FfiResult::NULL_POINTER`; and missing mandatory `0.2` slots fail `validate_for_guest` with `FfiResult::NULL_POINTER`.

9. **`tests::layout::unknown_flags_and_nonzero_reserved_rejected`**
   - **Why:** Silently ignoring unknown flag bits or non-zero reserved padding bytes hides version skew and garbage stack memory bugs.
   - **Proves (`PO-W5`):** Strict validation across `BamManifest`, `BamTermDesc`, `BamQueryDesc`, `BamSystemDesc`, and `ComponentDesc` rejects unknown flag bits (`UNSUPPORTED`), non-zero `reserved0` (`INVALID_ARGUMENT`), reserved schedule IDs `6..1024` (`INVALID_ARGUMENT`), query/term overflows (`CAPACITY_EXCEEDED`), `SPARSE_SET | SUMMARY_TICK` (`INVALID_ARGUMENT`), and invalid `(size, align)` pairs (`INVALID_ARGUMENT`), while accepting ZST (`0, 1`) and SIMD (`32, 32`) layouts.

10. **`tests::layout::siphash24_128_vectors_and_component_uuid`**
    - **Why:** `ComponentUuid` must match standard SipHash-2-4-128 bit-for-bit across Rust and C99 (`bam_uuid_from_parts`) so components never collide or misidentify across mods.
    - **Proves:** Official Aumasson & Bernstein SipHash-2-4-128 vectors (`len = 0, 1, 8, 15, 63`), parity between streaming `ComponentUuid::from_parts` and one-shot `siphash24_128`, cross-mod namespace/size/schema isolation, `from_canonical_name`, `parse_hex` / `to_u128` round-trip, and rejection of malformed UUID strings by `try_parse_hex`.

11. **`tests::layout::abi_pod_and_vtable_header_and_column_info`**
    - **Why:** Verifies the distinction between cross-language `AbiPod` and same-process `RawInsertable`, plus runtime VTable/column/tick metadata checks.
    - **Proves:** `AbiPod` and `RawInsertable` trait coverage (including `u128`/`i128`/`usize`/`isize` in `RawInsertable`), mock `ModComponent` `VTableHeader::validate` (`OK`, `TYPE_MISMATCH`, `INVALID_VERSION`) and `resolve_symbol`, `BamColumnInfo` flag helpers and `matches_layout`, wraparound-safe `BamChangeTicks`, `BamObserverDesc::with_entity` 64-bit packing, and `FfiChildOf` POD invariants.

12. **`tests::layout::min_v0_2_backwards_compatibility_for_manifest_component_desc_and_host_api`**
    - **Why:** Ensures exact ABI `0.2` minimum-size prefix structs (`MinimalV02Manifest`, `MinimalV02ComponentDesc`) remain accepted by ABI `0.3` readers.
    - **Proves (`PO-W3`):** `read_descriptor` accepts exact `MIN_V0_2_SIZE` prefixes for `BamManifest` and `ComponentDesc`, validates them as `FfiResult::OK`, zero-extends optional callbacks/hooks to `None`, and reports uncovered slots via `covers_slot == false`.

13. **`tests::layout::ffi_slice_and_str_view_null_zero_and_alignment_safety`**
    - **Why:** Passing `(NULL, 0)` from C directly into `core::slice::from_raw_parts` is immediate UB in Rust and flagged by Miri.
    - **Proves:** `FfiSlice::try_as_slice` safely turns `(NULL, 0)` into `&[]` via `NonNull::dangling()`, rejects `(NULL, len > 0)` with `NULL_POINTER`, rejects misaligned non-null pointers with `INVALID_ARGUMENT`, and `StrView::try_as_str` rejects invalid UTF-8 with `INVALID_UTF8`.

14. **`tests::layout::ffi_option_asset_handles_triggers_lifecycle_observers_and_modes`**
    - **Why:** Covers all additive ABI `0.2.x` types and builder predicates before ABI `0.3.0`.
    - **Proves:** `FfiOption<T>` round-trips and `AbiPod`/`RawInsertable` bounds; `FfiAssetHandle` (`none`, `from_index`, `from_index_bits`, `from_uuid`, `reserved0 != 0`, invalid `kind` / `asset_type`) and `FfiAssetLoadState`; `AccessMode::{ALLOW, HAS, REMOVED_READ}` and `BamTermDesc::or_group`; lifecycle/mutable/propagating `BamObserverDesc` and `BamTriggerDesc`; and `BamSystemDesc` weak ordering / `AMBIGUOUS_WITH_ALL` plus `BamColumnInfo::is_summary_tick`.

15. **`tests::layout::abi_v0_3_extension_layouts_and_validation`**
    - **Why:** Locks down every ABI `0.3.0` wire addition across 64-bit and 32-bit targets.
    - **Proves:** `BamSystem` / `BamSystemSet` / `BamSymbol` / `BamFieldKind` POD and sentinel invariants; `BAM_LOG_*` levels `0..=4`; FNV-1a determinism; exact 64-bit/32-bit sizes and offsets for `BamFieldDesc`, `BamRequiredComponent`, `BamRelationDesc`, `BamLayoutInfo`, and `BamScheduleSetDesc`; `ComponentDesc::schema_fingerprint` and `BamLayoutInfo::schema_fingerprint` offsets; `BamRelationDesc` layout/flag validation; mutually exclusive `BamScheduleSetDesc` execution-mode flags (`SEQUENCE`, `SEQUENCE_WEAK`, `PARALLEL`); and `BamHostApi::has_all_v0_3_slots` / `has_all_slots`.

16. **`tests::layout::bevy_020_additive_sys_helpers_and_validators`**
    - **Why:** Exercises edge-case validation and formatting helpers added for Bevy `0.20` parity (`Disabled`, `Ord`, `Display`, `BamFieldDesc`, `BamRequiredComponent`).
    - **Proves:** `FfiDisabled` ZST/UUID invariants; `Ord` ordering on opaque handle primitives; `Display` formatting for valid and unknown `AccessMode`, `ScheduleId`, `FfiAssetLoadState`, `BamEventKind`, and `BamFieldKind`; all error branches of `BamFieldDesc::validate` and `BamRequiredComponent::validate` (including mutually exclusive `CUSTOM_BYTES` + `INIT_FN`, `isize::MAX` overflow, and null pointers); and builders/predicates on `BamRelationDesc`, `BamScheduleSetDesc`, and `BamLayoutInfo`.

---

## 3. Property-Based Tests (`tests/proptest_wire.rs`)

1. **`siphash24_128_matches_reference`**
   - **Why:** Fixed test vectors only probe a handful of lengths; tail-padding (`total_len & 0xff`) and 8-byte block chunking bugs can hide at arbitrary lengths.
   - **Proves:** Differential equivalence of `bamabi_sys::siphash24_128` against an independent `ref_siphash24_128` oracle across arbitrary 128-bit keys and `0..=300`-byte payloads.

2. **`const_sip_hasher_multi_chunk_from_parts_matches_flat_oneshot`**
   - **Why:** `ComponentUuid::from_parts` streams multiple slices (`canonical_name`, `0xFF`, `size`, `align`, `schema`) through `ConstSipHasher128`; partial-tail buffering bugs would break parity with C99's `bam_uuid_from_parts`.
   - **Proves:** Bit-exact equivalence between `ComponentUuid::from_parts` / `from_namespaced` / `from_canonical_name`, one-shot `siphash24_128`, and `ref_siphash24_128` over arbitrary UTF-8 names and layout parameters.

3. **`const_sip_hasher_multi_chunk_field_schema_matches_flat_oneshot`**
   - **Why:** `ComponentUuid::hash_field_schema` iteratively folds `1..=8` fields with `0xFE`/`0xFD` separators into a 64-bit schema digest.
   - **Proves:** Each chained `hash_field_schema` step matches the lower 64 bits of one-shot `siphash24_128` and `ref_siphash24_128` over the flat concatenated field tuple.

4. **`try_parse_hex_valid_roundtrip`**
   - **Why:** Ensures every 128-bit UUID survives mixed-case, hyphenated (`36`-char) and compact (`32`-char) formatting and parsing in `const` contexts.
   - **Proves:** Lossless round-trip of `try_parse_hex`, `parse_hex`, and `from_u128`/`to_u128` across arbitrary `[u8; 16]` values and per-nibble casing.

5. **`try_parse_hex_arbitrary_utf8_matches_reference`**
   - **Why:** `try_parse_hex` indexes `&str` bytes directly; multi-byte UTF-8 strings of byte length 32 or 36 must never panic or be accepted.
   - **Proves:** Panic-free differential equivalence against `ref_parse_uuid_hex` across arbitrary UTF-8 strings and adversarial `28..=40`-char mixed ASCII/emoji strings.

6. **`try_parse_hex_mutated_valid_strings`**
   - **Why:** Near-valid UUID strings (single swapped char, shifted hyphen, replaced nibble, inserted multi-byte char) stress every validation branch.
   - **Proves:** Exact differential equivalence against `ref_parse_uuid_hex` after `1..=6` random structural mutations.

---

## 4. Kani Formal Verification Harnesses (`src/kani_proofs.rs`)

> **Proof Obligation `PO-W5`:** Strict unknown-flag (`UNSUPPORTED`) and non-zero reserved-word (`INVALID_ARGUMENT`) rejection across descriptors is discharged deterministically by unit tests in `src/tests/layout.rs` (`unknown_flags_and_nonzero_reserved_rejected`, `abi_v0_3_extension_layouts_and_validation`, `bevy_020_additive_sys_helpers_and_validators`).

### `PO-W1`: ABI Version Packing & Unpacking Bijectivity (4 Harnesses)

- **`po_w1_pack_unpack_abi_version_bijection`**: Proves over all valid `(maj <= 0xFF, min <= 0xFF, pat <= 0xFFFF)` and all `u32` words that `pack_abi_version` and `unpack_abi_version` form a lossless bijection matching `abi_version_{major,minor,patch}` and `ABI_VERSION`.
- **`po_w1_pack_abi_version_rejects_major_overflow`**: Proves `pack_abi_version` panics for all `maj > 0xFF` instead of silently truncating bits.
- **`po_w1_pack_abi_version_rejects_minor_overflow`**: Proves `pack_abi_version` panics for all `min > 0xFF`.
- **`po_w1_pack_abi_version_rejects_patch_overflow`**: Proves `pack_abi_version` panics for all `pat > 0xFFFF`.

### `PO-W2`: SemVer ABI Compatibility Matrix & Order-Theoretic Laws (1 Harness)

- **`po_w2_abi_version_compatibility_laws`**: Exhaustively proves over all `(u32, u32)` pairs that `abi_version_is_compatible` satisfies exact pre-1.0/post-1.0 SemVer semantics, reflexivity, patch-invariance, symmetry (`major == 0`), minor antisymmetry (`major >= 1`), and transitivity across arbitrary 3-way version chains.

### `PO-W3`: `ExtensibleStruct` Prefix Reading, Zero-Extension & Tail Truncation (14 Harnesses)

- **`po_w3_extensible_{bam_manifest, bam_host_api, bam_system_desc, bam_observer_desc, bam_trigger_desc, bam_query_desc, component_desc, vtable_header, bam_field_desc, bam_required_component, bam_relation_desc, bam_layout_info, bam_schedule_set_desc}`** (13 harnesses): Each invokes `verify_extensible_struct_contract` symbolically over all `struct_size: usize` and payload bytes to prove:
  1. Null pointers always return `Err(FfiResult::NULL_POINTER)` without dereferencing.
  2. `struct_size < MIN_V0_2_SIZE` always returns `Err(FfiResult::INVALID_VERSION)` without reading past offset 0 / 4.
  3. Valid prefixes in `MIN_V0_2_SIZE..=size_of::<T>()` copy `copy_len` bytes faithfully and zero-fill every trailing byte in `copy_len..size_of::<T>()`.
  4. Unaligned `+1`-offset minimal `MIN_V0_2_SIZE` buffers and future buffers with `declared_size > size_of::<T>()` up to `u32::MAX` never read out of bounds.
- **`po_w3_host_api_raw_header_reads`**: Proves `BamHostApi::raw_struct_size` and `raw_abi_version` handle null pointers without dereferencing, never read offset 4 when `struct_size < 8`, and accurately read 4-byte / 8-byte headers without forming `&BamHostApi`.

### `PO-W4`: Overflow-Free `covers_slot` Arithmetic (1 Harness)

- **`po_w4_covers_slot_exact_and_overflow_free`**: Proves over all `(struct_size: u32, slot_offset: usize, slot_size: usize)` that `covers_slot` never wraps or panics and equals mathematical `slot_offset + slot_size <= struct_size` across `BamHostApi`, `BamManifest`, `ComponentDesc`, `BamSystemDesc`, `BamObserverDesc`, and `BamTriggerDesc`.

### `PO-W6`: `FfiResult` & Wire Enum Newtype Totality (1 Harness)

- **`po_w6_offsets_and_ffi_result_totality`**: Exhaustively proves over all `raw: u32` (`0..=u32::MAX`) that `FfiResult(raw)` is panic-free, `is_ok() ^ is_err()`, `name()` is non-empty and returns `"UNKNOWN"` iff `raw` is not one of the 18 canonical codes, and `AccessMode`, `ScheduleId`, `BamEventKind`, `BamFieldKind`, and `FfiAssetLoadState` have total, panic-free `name()` and exact `is_valid()` ranges.

### `PO-W7`: `FfiSlice<T>` & `StrView` Soundness (1 Harness)

- **`po_w7_ffi_slice_and_str_view_soundness`**: Symbolically proves empty C slices `(NULL, 0)` safely yield `&[]`, non-empty null pointers return `NULL_POINTER`, misaligned pointers return `INVALID_ARGUMENT`, byte-length overflows (`> isize::MAX`) return `INVALID_ARGUMENT`, aligned bounded slices and ZST slices round-trip without UB, and `StrView::try_as_str` is panic-free on arbitrary 4-byte buffers (rejecting UTF-16 surrogates `0xED 0xA0..=0xBF`, overlong `0xC0..=0xC1`, and `> U+10FFFF` with `INVALID_UTF8`).

### `PO-W8`: `FfiOption<T>`, `BamChangeTicks`, and `ComponentUuid` Invariants (3 Harnesses)

- **`po_w8_ffi_option_zeroed_none_and_roundtrip`**: Proves `FfiOption::none()` (`MaybeUninit::zeroed().assume_init()`) is sound across `u32`, `FfiEntity`, `ComponentUuid`, `FfiAssetHandle`, and nested `FfiOption<u64>`, round-trips `Option<T>`, and decodes any non-zero `FfiBool` tag byte (`1..=255`) as `Some` without UB.
- **`po_w8_change_ticks_wraparound_safety`**: Proves over all `(tick, last_run, this_run) in u32^3` that `BamChangeTicks::is_tick_newer` is panic-free, irreflexive (`tick == last_run => false`), matches clamped wrapping distance (`BAM_MAX_CHANGE_AGE`), and `BamSystem` / `BamSystemSet` / `BamSymbol` / `FfiAssetLoadState` sentinels are exact.
- **`po_w8_component_uuid_bijection_and_hex_parser`**: Proves `from_u128`/`to_u128` bijection, exact 32-char and 36-char (upper/lower) hex round-trip over arbitrary 16-byte UUIDs, and panic-free rejection of malformed 32/36-byte ASCII and multi-byte non-ASCII UTF-8 strings in `try_parse_hex`.

---

## 5. Doctests (`cargo test --doc -p bamabi_sys --all-features`)

### Passing Usage Doctests (13)

- `src/lib.rs`: Crate-level Step 2 example exercising `ComponentUuid::from_parts`, `BamComponent`, `FfiOption`, `StrView`, and `FfiResult`.
- `src/access.rs`: `AccessMode` (`BamTermDesc::write`, `is_valid`, `is_mutable`).
- `src/component.rs`: `ComponentDesc` construction, `BAM_COMPONENT_FLAG_SPARSE_SET`, and `validate`.
- `src/primitives/mod.rs`: `FfiResult::OK` predicates and `name()`.
- `src/primitives/asset.rs`: `FfiAssetHandle::NONE` and `from_index` validation.
- `src/primitives/bool.rs`: `FfiBool` truthiness (`0` vs non-zero `42`) and `bool` conversions.
- `src/primitives/entity.rs`: `BamComponent` sentinel/validity and `FfiEntity` round-trip.
- `src/primitives/option.rs`: `FfiOption::some` / `none` / `into_option`.
- `src/primitives/slice.rs`: `FfiSlice::from_slice` and `StrView::from_str` safe conversion.
- `src/primitives/uuid.rs`: `ComponentUuid::from_parts` namespace/layout isolation and `parse_hex`.
- `src/traits.rs`: `SymbolEntry` construction.
- `src/version.rs`: `ABI_VERSION` unpacking and `abi_version_is_compatible` examples.

### `compile_fail` Safety Doctests (5, `src/traits.rs`)

- Asserts `assert_abi_pod::<u128>()`, `assert_abi_pod::<i128>()`, `assert_abi_pod::<usize>()`, `assert_abi_pod::<isize>()`, and `assert_abi_pod::<bool>()` **fail to compile**, proving excluded non-C99 / pointer-width-dependent / invalid-bit-pattern primitives can never accidentally satisfy `AbiPod`.
