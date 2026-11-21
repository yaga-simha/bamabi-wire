# `bamabi_macros` Test & Verification Guide

Complete catalog of every unit test, `trybuild` UI fixture, VTable/witness integration test, property-based witness test, and doctest in `bamabi_macros`, explaining **why** each exists and **what** soundness, layout, or ABI invariant it proves.

---

## 1. Suite Overview & Running Commands

| Suite | File(s) | Count | Miri Status | What It Proves |
|---|---|---|---|---|
| Unit tests | `src/tests.rs` | 18 tests | Host-only (tests AST/tokens) | Macro AST validation, error messages, crate resolution, token emission |
| UI compile-fail & pass | `tests/compile_fail_tests.rs` + `tests/ui/*.rs` | 2 tests + 34 `.rs` fixtures | `ui_compile_fail_and_pass_suite` `#[cfg_attr(miri, ignore)]` | Compiler acceptance of valid derives and exact error rejection of UB/layout hazards |
| VTable & witness runtime | `tests/mod_component_vtable.rs` | 16 tests | **Miri-verified** (`-Zmiri-tree-borrows`) | Runtime `extern "C"` getters/setters, aliasing guards, enums, metadata, and witness migrations |
| Property-based witness | `tests/proptest_witness.rs` | 3 tests (512 cases each; 4 under Miri) | **Miri-verified** (`-Zmiri-tree-borrows`) | Arbitrary `export_fields` → `import_field` round-trips & schema evolution |
| Rustdoc doctests | `src/lib.rs` | 2 doctests (1 pass, 1 `compile_fail`) | Host-only | Public usage example + padding `compile_fail` guard |

### Commands

```bash
# Run all 39 tests + 34 trybuild UI fixtures
cargo test -p bamabi_macros --all-features

# Run rustdoc doctests
cargo test --doc -p bamabi_macros --all-features

# Run Miri Tree Borrows verification over unsafe generated VTable & witness code
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test mod_component_vtable
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test proptest_witness
```

> **Maintainer Rule (`tests/ui/`):** Never edit line numbers or contents of existing `tests/ui/*.rs` or `.stderr` files unless regenerating compiler diagnostics intentionally with `TRYBUILD=overwrite`, because `trybuild` matches `.stderr` against exact line and column spans.

---

## 2. Unit Tests (`src/tests.rs` — 18 Tests)

These tests parse `syn::DeriveInput` directly and inspect the resulting `TokenStream` or `syn::Error` without invoking `rustc`.

| Test | Why It Exists & Invariant Proved |
|---|---|
| `rejects_generic_structs` | Proves `#[derive(ModComponent)]` rejects generic structs (`GenericComp<T>`) because monomorphization-dependent layouts cannot emit one static C-ABI VTable. |
| `rejects_invalid_enum_reprs_and_abi_pod_enums` | Proves `ModComponent` rejects Rust-repr enums, data-carrying enums missing `C` (`#[repr(u8)]`), and zero-variant enums; also proves `AbiPod` rejects enums completely. |
| `rejects_packed_structs` | Proves `#[derive(ModComponent)]` rejects `#[repr(C, packed)]` because taking aligned pointers/references (`&*typed_ptr`, `addr_of_mut!`) to packed fields is UB. |
| `rejects_nested_field_name_collision` | Proves `CodegenState::emit_accessor` catches VTable slot collisions (fields `x` and `mut_x` both colliding on `ptr_mut_x`) at macro-expansion time instead of emitting duplicate struct fields. |
| `rejects_heap_owning_types_without_skip` | Proves `Option<String>`, `Box<u32>`, `HashMap<u32, u32>`, and `&'static str` fail cleanly with actionable `#[mod_field(skip)]` diagnostics. |
| `rejects_unknown_attributes_and_invalid_uuids` | Proves typos in `#[mod_component(...)]`, malformed `uuid = "..."` strings, and conflicting `namespace` + namespaced `name = "other::Pos"` are rejected at expansion time. |
| `bool_generates_ffi_bool_and_ffi_result_and_omits_raw_insertable` | Proves a `bool` field maps to `FfiBool`/`FfiResult` slots and sets `IS_ABI_POD = false` and `IS_RAW_INSERTABLE = false` (no `unsafe impl AbiPod`/`RawInsertable`). |
| `string_and_vec_omit_raw_insertable_and_emit_push` | Proves `String`, `Vec<f32>`, and `#[mod_field(skip)]` suppress `AbiPod` and `RawInsertable`, and verifies all 11 `Vec` VTable slots (`get`, `set`, `len`, `clear`, `push`, `pop`, `remove`, `swap_remove`, `truncate`, `reserve`, `as_mut_slice`) are emitted. |
| `supports_enums_ffi_option_summary_tick_and_require` | Proves fieldless `#[repr(u8)]` and tagged `#[repr(C, u8)]` enums emit `tag`/`set_*`/`try_get_*` without `AbiPod`, verifies `FfiOption<u32>` emits `get`/`set`/`clear` while staying `IS_ABI_POD = true`, checks `SUMMARY_TICK` / `REQUIRED_COMPONENT_*`, and rejects `sparse_set + summary_tick`. |
| `emits_witness_symbols_for_non_opaque_and_omits_for_opaque` | Proves non-opaque components register `bam.v0.<name>.field_info`, `.export_fields`, and `.import_field`, while `#[mod_component(opaque)]` omits all three. |
| `repr_c_pod_emits_raw_insertable_abi_pod_and_schema_uuid` | Proves `#[repr(C)]` POD structs emit `IS_ABI_POD = true`, `IS_RAW_INSERTABLE = true`, `hash_field_schema` → `ComponentUuid::from_parts`, and both `unsafe impl`s. |
| `usize_in_mod_component_is_raw_insertable_but_not_abi_pod` | Proves `usize` in a `#[repr(C)]` `ModComponent` implements `RawInsertable` (same-process safe) while keeping `IS_ABI_POD = false` (pointer-width dependent). |
| `abi_pod_derive_uses_assert_abi_pod_and_raw_insertable` | Proves `expand_abi_pod` rejects missing `#[repr(C)]`/`transparent` and `#[repr(packed)]`, and emits `assert_abi_pod::<FieldTy>()`, `size_of` zero-padding check, `RawInsertable`, and `AbiPod`. |
| `crate_path_resolution_and_overrides` | Proves `resolve_sys_crate_with` and `resolve_guest_crate_with` handle `crate = ...` / `guest_crate = ...` overrides, renamed packages, self-crate compilation, and `bamabi`/`bamabi_host`/`bamabi_guest` facades. |
| `hygienic_const_scope_and_namespaced_bam_export_symbols` | Proves generated `extern "C"` functions stay private inside `const _: ()` without `export_name`/`no_mangle` by default, and emit `bam_<ns>_<Struct>_<slot>` only under `export_symbols`. |
| `guest_component_derive_expansion_and_errors` | Proves `#[derive(GuestComponent)]` emits `impl GuestComponent` (only, no `AbiPod`/`RawInsertable`), handles `name`/`namespace`/`uuid`/`no_schema_hash`/`sparse_set`/`immutable`/`summary_tick`/`guest_crate`, falls back to `#[mod_component(...)]`, and rejects `sparse_set + summary_tick`, enums, and missing `repr`. |
| `mod_component_guest_shorthand_and_non_pod_rejection` | Proves `#[mod_component(..., guest)]` delegates `GuestComponent` to `ModComponent` on `AbiPod` structs and rejects `String` fields, `no_pod`, and enums. |
| `abi_v03_handles_classify_as_primitives_and_emit_c_zig_types` | Proves `BamSystem`, `BamSystemSet`, `BamSymbol`, `FfiAssetLoadState`, `BamEventKind`, `BamFieldKind`, `FfiChildOf`, and `FfiAssetHandle` qualify as `IS_ABI_POD = true` and emit exact C99 (`Bam*`) and Zig (`bam.Bam*`) types. |

---

## 3. UI Compile-Fail & Positive-Control Suite (`tests/compile_fail_tests.rs` & `tests/ui/` — 2 Tests, 34 Fixtures)

`tests/compile_fail_tests.rs` defines two Rust tests:
- **`link_anchor_traits_are_live`**: Compiles and runs `LinkAnchorPod` (`AbiPod`) and `LinkAnchorComp` (`ModComponent`) so `bamabi_macros` and `bamabi_sys` are linked for `trybuild`, and verifies `CANONICAL_NAME == "compile_fail_tests::LinkAnchorComp"`.
- **`ui_compile_fail_and_pass_suite`**: Runs `trybuild::TestCases` over all 34 fixtures in `tests/ui/*.rs` (skipped under Miri via `#[cfg_attr(miri, ignore)]`).

### Positive Controls (`pass_*.rs` — 2 Fixtures)

| Fixture | Why It Exists & Invariant Proved |
|---|---|
| `pass_valid_abi_pod_and_mod_component.rs` | Proves valid `#[repr(C)]` (with explicit `_pad`), `#[repr(transparent)]`, nested `AbiPod`, `FfiOption<T>`, `#[repr(C, u8)]` enums with `require(...)`, and `Vec`/`String`/`bool` `ModComponent`s all compile cleanly with expected `IS_ABI_POD` / `IS_RAW_INSERTABLE` / `COMPONENT_FLAGS` values. |
| `pass_guest_component.rs` | Proves bit-for-bit `ComponentUuid::to_u128()` parity between host `#[derive(ModComponent)]` (`HostPosition`) and guest `#[derive(AbiPod, GuestComponent)]` (`GuestPosition`), plus `#[derive(ModComponent, GuestComponent)]` fallback (`DualDerived`) and `#[mod_component(..., guest)]` shorthand (`ShorthandGuest`) against a `GuestComponent` trait (`mock_guest`). |

### `#[derive(AbiPod)]` Compile-Fail Fixtures (`abi_pod_*.rs` — 14 Fixtures)

| Fixture | Rejected Construct | Soundness Rule Proved |
|---|---|---|
| `abi_pod_bool_field.rs` | `bool` field (even with `_pad: [u8; 3]`) | `bool` has invalid bit patterns `2..=255`; must use `FfiBool`. |
| `abi_pod_direct_drop.rs` | `impl Drop for DirectDropPod` | Types with drop glue (`needs_drop` / missing `Copy`) cannot be bitwise-copied across FFI. |
| `abi_pod_enum.rs` | `#[repr(u32)] enum StateEnum` | Enums have invalid discriminant bit patterns and are rejected by `AbiPod`. |
| `abi_pod_generic.rs` | `struct GenericPod<T>` | Generic structs have monomorphization-dependent layouts. |
| `abi_pod_implicit_padding.rs` | `tag: u8, value: u32` (`#[repr(C)]`) | 3 bytes of interior padding violates `size_of::<Self>() == sum(fields)`. |
| `abi_pod_nested_drop_field.rs` | Nested `CustomDroppable` with `Drop` | Nested drop glue prevents `Copy` and `AbiPod` satisfaction. |
| `abi_pod_no_repr_c.rs` | Default `#[repr(Rust)]` struct | Rust layout is unspecified across compiler runs/targets. |
| `abi_pod_repr_c_packed.rs` | `#[repr(C, packed)]` struct | Unaligned field references are undefined behavior in Rust. |
| `abi_pod_string_field.rs` | `String` field | Heap pointers/allocators cannot cross FFI by value. |
| `abi_pod_tagged_enum.rs` | `#[repr(C, u8)] enum TaggedPodRejected` | Tagged enums are non-POD (`AbiPod` rejects all enums). |
| `abi_pod_trailing_padding.rs` | `id: u64, flag: u8` (`#[repr(C)]`) | 7 bytes of trailing padding violates `size_of::<Self>() == sum(fields)`. |
| `abi_pod_u128_field.rs` | `u128` field | `u128` alignment diverges across C/LLVM ABIs (`8` vs `16` bytes). |
| `abi_pod_usize_field.rs` | `usize` field | Pointer-sized integers vary between 32-bit and 64-bit targets. |
| `abi_pod_vec_u8_field.rs` | `Vec<u8>` field | Heap-allocated buffer owned by host allocator cannot be `AbiPod`. |

### `#[derive(ModComponent)]` Compile-Fail Fixtures (`mod_component_*.rs` — 16 Fixtures)

| Fixture | Rejected Construct / Assertion | Soundness / Contract Rule Proved |
|---|---|---|
| `mod_component_bool_not_abi_pod.rs` | `assert_abi_pod::<BoolComp>()` | Struct with `bool` field must NOT implement `AbiPod`. |
| `mod_component_bool_not_raw_insertable.rs` | `assert_raw_insertable::<BoolComp>()` | Struct with `bool` field must NOT implement `RawInsertable` (invalid bit patterns). |
| `mod_component_enum_not_abi_pod.rs` | `assert_abi_pod::<EnumNotAbiPod>()` | `ModComponent` enum must NOT implement `AbiPod`. |
| `mod_component_ffi_option_bool_rejected.rs` | `FfiOption<bool>` field | `FfiOption<T>` requires `T: AbiPod`; `bool` is rejected. |
| `mod_component_guest_non_pod.rs` | `#[mod_component(name = "bad::NonPodGuest", guest)]` with `String` | `#[mod_component(..., guest)]` requires `IS_ABI_POD == true`. |
| `mod_component_implicit_padding.rs` | `#[repr(C)]` `ModComponent` with `a: u8, b: u64` | POD `ModComponent` enforces zero implicit padding unless `no_pod` or explicit `_pad` is used. |
| `mod_component_invalid_uuid_rejected.rs` | `#[mod_component(uuid = "not-a-valid-hex-uuid")]` | Malformed UUID literals fail at macro-expansion time. |
| `mod_component_no_pod_suppresses_abi_pod.rs` | `assert_abi_pod::<ExplicitNoPod>()` with `#[mod_component(no_pod)]` | `no_pod` suppresses `AbiPod` impl even when all fields are POD. |
| `mod_component_non_opaque_nested_string_rejected.rs` | Nested `InnerLabel` (containing `String`) without `opaque` | Non-`RawInsertable` nested structs require `#[mod_component(opaque)]` on the outer struct. |
| `mod_component_spoofed_string_rejected.rs` | Local `struct String { ... }` without `#[mod_field(nested)]` | Anti-spoofing trait `__BamStdString` catches shadowed `String` structs at compile time. |
| `mod_component_spoofed_vec_rejected.rs` | Local `struct Vec<T>(T)` without `#[mod_field(nested)]` | Anti-spoofing trait `__BamStdVec<T>` catches shadowed `Vec` structs at compile time. |
| `mod_component_string_not_abi_pod.rs` | `assert_abi_pod::<NamedComp>()` | Struct with `String` must NOT implement `AbiPod`. |
| `mod_component_string_not_raw_insertable.rs` | `assert_raw_insertable::<NamedComp>()` | Struct with `String` must NOT implement `RawInsertable` (heap ownership + drop glue). |
| `mod_component_vec_of_bool_comp_rejected.rs` | `Vec<BoolComp>` field | `Vec<T>` element type `T` must satisfy `AbiPod` (`bool` disqualifies `BoolComp`). |
| `mod_component_vec_of_repr_rust_rejected.rs` | `Vec<ReprRustPos>` field | `Vec<T>` element type `T` must satisfy `AbiPod` (`#[repr(Rust)]` is non-POD). |
| `mod_component_vec_of_string_comp_rejected.rs` | `Vec<NamedComp>` field | `Vec<T>` element type `T` must satisfy `AbiPod` (`String` is non-POD). |

### `#[derive(GuestComponent)]` Compile-Fail Fixtures (`guest_component_*.rs` — 2 Fixtures)

| Fixture | Rejected Construct | Contract Rule Proved |
|---|---|---|
| `guest_component_enum_rejected.rs` | `#[derive(GuestComponent)]` on `#[repr(u8)] enum` | `GuestComponent` requires `AbiPod`, which only supports structs. |
| `guest_component_sparse_summary_tick.rs` | `#[guest_component(..., sparse_set, summary_tick)]` | `summary_tick` requires `Table` storage and cannot be combined with `sparse_set`. |

---

## 4. VTable & Witness Integration Suite (`tests/mod_component_vtable.rs` — 16 Tests)

All 16 tests compile real derived components and invoke generated `extern "C"` functions under both native `cargo test` and `cargo +nightly miri test`.

| Test | Why It Exists & Invariant Proved |
|---|---|
| `vtable_header_and_error_returning_setters` | Proves `VTableHeader` sits at byte offset `0`, passes `ExtensibleStruct` `read_descriptor` & `validate`, checks `FfiBool` normalization (`2`/`255` → `true`), nested `ptr`/`ptr_mut`, `StrView` UTF-8 validation (`INVALID_UTF8` leaves field intact; `(NULL, 0)` clears), `Vec<u32>` `get`/`set`/`push`, `NULL_POINTER`/`INVALID_ARGUMENT` on null/unaligned pointers, and skipped-field isolation. |
| `self_aliasing_string_and_vec_setters` | Proves passing a `StrView` or `FfiSlice` borrowed from a component's own `String`/`Vec` back into `set_*` (full or interior sub-slice) triggers the overlap-copy guard and stays UB-free under Miri Tree Borrows. |
| `namespaced_canonical_names_symbols_and_uuid_fingerprints` | Proves default `module_path!()::Name`, explicit `name = "ns::Name"` and `namespace = "..."`, `"bam.v0.<name>.<slot>"` symbol lookup, `hash_field_schema` sensitivity to field type (`f32` vs `i32`) and field order (`(x, y)` vs `(y, x)`), explicit `uuid = "..."` parsing, and `SPARSE_SET \| IMMUTABLE` flags. |
| `byte_stride_array_iteration_and_tuple_struct` | Proves raw byte-stride iteration (`i * vt.header.size`) over a 32-element `Vec<MixedComponent>` via resolved `"bam.v0.*.get_health"` function pointers, plus tuple-struct (`NameTag`) `get_0`/`set_0` accessors. |
| `pod_vs_raw_insertable_vs_opaque_traits` | Proves exact `IS_ABI_POD` / `IS_RAW_INSERTABLE` trait matrix across `MixedComponent`, `Vec3` (`#[repr(Rust)]` POD → `RawInsertable` only), `HostPlatformIndex` (`usize` + `u128` → `RawInsertable` only), `PodPosition`, `PodEntityTransform` (`FfiEntity` + `[f32; 4]`), `StandalonePodPair` (`AbiPod`), `TransparentEntityId` (`#[repr(transparent)]`), and `OpaqueNamedHolder` (`opaque`). |
| `m06_qualified_std_types_aliases_and_explicit_mod_field_override` | Proves `::std::string::String`, `::std::vec::Vec<T>`, type aliases (`#[mod_field(string)]`, `#[mod_field(vec)]`, `#[mod_field(vec(u32))]`, `#[mod_field(primitive)]`, `#[mod_field(bool)]`), and shadowed local `struct String` with `#[mod_field(nested)]` all generate sound VTable slots. |
| `crate_override_and_facade_reexport` | Proves `#[mod_component(crate = self::facade::bamabi_sys)]` resolves both `AbiPod` and `ModComponent` through an arbitrary intermediate facade module. |
| `duplicate_struct_names_in_submodules` | Proves two `DuplicateItem` structs in sibling submodules under `#[deny(non_snake_case)]` produce distinct `module_path!()` UUIDs, zero symbol collisions, and clean isolation from a user `DuplicateItem_get_x()` function. |
| `emits_c_decl_and_zig_decl_for_pod_and_non_pod_components` | Proves `C_DECL` and `ZIG_DECL` emit full structs + `BAM_STATIC_ASSERT(sizeof(...VTable) == 88u)` for `AbiPod` (`PodPosition`) and opaque forward typedefs + `Slice` + C++17 `HostVecView` / `<Struct>View` + Zig `VecView` for non-POD (`MixedComponent`). |
| `full_host_vec_vtable_operations_and_bounds_safety` | Exercises all 11 `Vec<u32>` VTable slots (`len`, `reserve` + `usize::MAX` → `CAPACITY_EXCEEDED`, `remove` + OOB `OUT_OF_RANGE` + null out-ptr `NULL_POINTER`, `swap_remove`, `as_mut_slice`, `pop`, `push`, `truncate`, `clear`). |
| `ffi_option_summary_tick_and_require_metadata` | Proves `FfiOption<u32>`, `FfiOption<PodPosition>`, and aliased `#[mod_field(option(u64))]` emit `get`/`set`/`clear` slots while remaining `IS_ABI_POD = true`, checks `SUMMARY_TICK` & `REQUIRED_COMPONENT_*`, and validates generated `*_Option` C99/Zig typedefs. |
| `repr_c_fieldless_and_tagged_enums_vtable_and_schema_uuid` | Proves `#[repr(u8)]`, negative `#[repr(C)]`, and `#[repr(C, u8)]` tagged data enums support `tag`, `set_<variant>`, and `try_get_<variant>_<field>` (`INVALID_STATE` on wrong active variant), self-aliasing `set_cast`, discriminant-sensitive UUIDs, and `C_DECL`/`ZIG_DECL` enum tag emission. |
| `witness_field_info_export_and_import_migration_is_miri_clean` | Proves `opaque` components omit witness symbols; validates `field_info` names/tags/offsets; tests `RecordV1` → `RecordV2` schema migration (reordered, added, removed `NOT_FOUND`, widened `i32→i64`, `i8→i32`, `u16→u64`, `f32→f64`, `FfiOption::none()` padding safety, `INVALID_UTF8`); tests recursive `NestedHolder` (`"origin.x"`); and tests tagged enum witness migration (`"__variant"` + `INVALID_STATE`). |
| `tuple_struct_mixed_fields_vtable_and_witness_roundtrip` | Proves tuple structs (`TupleMixed(Vec<u32>, FfiOption<u32>, String, bool)`) generate working `push_0`/`len_0`/`get_1`/`set_2`/`set_3` slots and `"0".."3"` witness `field_info`/`export_fields`/`import_field` round-trips including clearing `FfiOption::some` back to `none`. |
| `wide_i64_and_u64_enums_discriminants_vtables_and_witness_roundtrip` | Proves extreme 64-bit signed (`i64::MIN = -9223372036854775808`, `i64::MAX`) and unsigned (`u64::MAX = 18446744073709551615`) enum discriminants round-trip cleanly without truncation across `tag`, `set_*`, `C_DECL`/`ZIG_DECL`, and witnesses. |
| `schema_fingerprint_bam_fields_and_custom_require_init_work` | Proves ABI `0.3` `SCHEMA_FINGERPRINT` diverges on field reordering, `BAM_FIELDS` matches exact `BamFieldKind`/name/offset/size (including `OPAQUE` for skipped fields), and `#[mod_component(require(MetaHealth = MetaHealth::make(125), MetaMarker))]` emits a working `BAM_REQUIRED_FLAG_INIT_FN` initializer alongside UUID-only dependencies. |

---

## 5. Property-Based Witness Suite (`tests/proptest_witness.rs` — 3 Tests)

Configured to run **512 randomized cases** natively (`4` cases under Miri for tractable runtime) using `collect_sink`, `export_all`, and `import_all` over `WitnessRecord`, `NestedLeaf`, `SchemaV1`, `SchemaV2`, and `WitnessEnum`:

| Property Test | Why It Exists & Invariant Proved |
|---|---|
| `mod_component_struct_witness_roundtrip` | Fuzzes `bool`, arbitrary UTF-8 `String`, `Vec<u8>`, `Vec<u32>`, `FfiOption<u32>`, doubly-nested `FfiOption<FfiOption<u8>>`, and nested `NestedLeaf` (`leaf.code`, `leaf.nested_opt`) exported from `src` and imported into an arbitrarily populated `dirty_dst`, asserting `dst == src`. |
| `mod_component_enum_witness_roundtrip` | Fuzzes `WitnessEnum` (`Empty`, `Payload { ... }`, `Tuple(...)`) exported with leading `"__variant"` and imported over an arbitrary `dirty_dst` enum variant, proving variant switching + payload overwrite leaves zero stale allocations or uninit bytes. |
| `mod_component_schema_migration_roundtrip` | Fuzzes `SchemaV1` → `SchemaV2` migration with reordered fields, `dropped_field` returning `FfiResult::NOT_FOUND`, `added_default` preserved untouched on `SchemaV2`, and lossless widening (`i16 → i64`, `f32 → f64` bit-exact for non-NaN values). |
