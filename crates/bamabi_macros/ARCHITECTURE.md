# `bamabi_macros` Architecture & Implementation Guide

`bamabi_macros` (`v0.3.0`, ABI `0x0003_0000`, `edition = "2024"`, `proc-macro = true`) is the Layer 0 procedural-macro compiler crate of [`bamabi-wire`](../../README.md). It transforms Rust struct and enum definitions into deterministic C-ABI VTables, 128-bit SipHash-2-4 component UUIDs, 64-bit FNV-1a schema fingerprints, reflection descriptors, hot-reload migration witnesses, ISO C99 / C++17 / Zig declarations, and zero-overhead `AbiPod` / `GuestComponent` implementations—with **zero Bevy dependencies** and **`#![forbid(unsafe_code)]`** on the macro crate itself.

---

## 1. Crate Role & Safety Boundary

### Dependency Isolation (`Cargo.toml`)
- **Proc-macro runtime dependencies:** `syn` (`2.0`, `features = ["full"]`), `quote`, `proc-macro2`, and `proc-macro-crate` ONLY.
- **Dev-dependencies:** `bamabi_sys`, `trybuild`, `proptest`.
- **Hard invariants:** `bamabi_macros` must **never** depend on any `bevy*` crate or on `bamabi_guest`. Even `#[derive(GuestComponent)]` and `#[mod_component(..., guest)]` resolve `bamabi_guest` solely through token-path resolution (`proc-macro-crate` or `guest_crate = ...`).

### `#![forbid(unsafe_code)]` vs. Generated `unsafe`
- `crates/bamabi_macros/src/lib.rs` enforces `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]` at the crate root. The procedural macro executes purely safe AST transformations.
- Generated code wraps helper types, `extern "C"` trampolines, and `unsafe impl` blocks inside an anonymous `const _: () = { ... };` scope so internal identifiers (`__BamPrimBool`, `__BamStdString`, `__BamStdVec`, `__BamFfiOption`, `__bam_*`, `VTABLE_INSTANCE`, `SYMBOLS`) never leak into user scope or trigger name collisions—even under `#[deny(non_snake_case)]` or when multiple modules define identically named components.
- Every generated `unsafe impl` (`AbiPod`, `RawInsertable`, `ExtensibleStruct`) is guarded by compile-time assertions, and every generated `unsafe extern "C" fn` validates pointer non-nullness and alignment before dereferencing.

---

## 2. End-to-End Architecture & Module Map

```text
  DeriveInput (syn)
         │
         ├──► #[derive(AbiPod)]        ──► abi_pod.rs
         │                                   └─ repr + padding + Copy + !needs_drop + assert_abi_pod
         │                                   └─ unsafe impl RawInsertable + AbiPod
         │
         ├──► #[derive(GuestComponent)]──► guest_component.rs
         │                                   └─ repr(C|transparent) + flags + hash_field_schema
         │                                   └─ impl bamabi_guest::GuestComponent (no VTable)
         │
         └──► #[derive(ModComponent)]  ──► codegen/mod.rs
                │
                ├─ attrs.rs            (repr, #[mod_component], #[mod_field], crate resolution)
                ├─ classify.rs         (FieldKind + canonical type tags)
                │
                ├─► Enum? ──► codegen/enums.rs ──► witness::emit_enum_witnesses
                │
                └─► Struct ─► codegen/{bool,primitive,string,vec,option,nested}.rs
                                 │  (produces AccessorSpec -> CodegenState::emit_accessor)
                                 ▼
                              codegen/witness.rs  (emit_struct_witnesses)
                                 │
                                 ▼
                              codegen/vtable.rs   (emit_vtable_and_impls)
                                 ├─ codegen/c_decl.rs (C_DECL + C++17 views + ZIG_DECL)
                                 ├─ <Struct>VTable + ExtensibleStruct
                                 ├─ BAM_FIELDS + SCHEMA_FINGERPRINT + REQUIRED_COMPONENTS
                                 ├─ impl ModComponent
                                 └─ conditional RawInsertable / AbiPod / GuestComponent
```

### Source File Inventory (`src/`)

| File | Responsibility |
|---|---|
| `src/lib.rs` | Crate root (`#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`), incremental crate docs, and `derive_abi_pod`, `derive_mod_component`, `derive_guest_component` entry points |
| `src/abi_pod.rs` | Standalone `#[derive(AbiPod)]` validator and `RawInsertable` + `AbiPod` emitter |
| `src/guest_component.rs` | Zero-VTable `#[derive(GuestComponent)]` emitter with `FIELD_SCHEMA_HASH` parity |
| `src/attrs.rs` | Parsing & validation of `#[repr(...)]`, `#[mod_component(...)]`, `#[guest_component(...)]`, `#[mod_field(...)]`, UUID hex syntax, symbol sanitization, and `proc-macro-crate` resolution |
| `src/classify.rs` | Syntactic `FieldKind` classifier, forbidden-type diagnostics, and canonical type-tag builder |
| `src/codegen/mod.rs` | `CodegenState`, `CanonicalNameSpec`, `AccessorSpec`, `SlotSpec`, `ReceiverKind`, `BamFieldKind` mapping, and `emit_accessor` slot/symbol driver |
| `src/codegen/primitive.rs` | By-value `get`/`set` `AccessorSpec` for `AbiPod` primitives and `usize`/`isize` |
| `src/codegen/bool.rs` | `FfiBool` (`u8`) marshaling `AccessorSpec` for Rust `bool` |
| `src/codegen/option.rs` | `get`/`set`/`clear` `AccessorSpec` for `FfiOption<T>` (`T: AbiPod`) |
| `src/codegen/string.rs` | `StrView` `get` and alias-safe `set` `AccessorSpec` for `std::string::String` |
| `src/codegen/vec.rs` | 11-slot `HostVec` `AccessorSpec` (`get`, `set`, `len`, `clear`, `push`, `pop`, `remove`, `swap_remove`, `truncate`, `reserve`, `as_mut_slice`) for `std::vec::Vec<T>` (`T: AbiPod`) |
| `src/codegen/nested.rs` | Interior-pointer `ptr`/`ptr_mut` `AccessorSpec` for nested structs and `[T; N]` arrays |
| `src/codegen/enums.rs` | Fieldless and `#[repr(C, <int>)]` data-carrying enum `tag`, `set_<variant>`, and `try_get_<variant>_<field>` emission |
| `src/codegen/witness.rs` | `field_info`, `export_fields`, and `import_field` reflection/migration functions (with primitive widening and nested `.` recursion) |
| `src/codegen/vtable.rs` | Anti-spoofing helper traits, alias-safe `String`/`Vec` runtime helpers, `<Struct>VTable`, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `ModComponent` impl, and conditional trait impls |
| `src/codegen/c_decl.rs` | `ModComponent::C_DECL` (ISO C99 + `#ifdef __cplusplus` C++17 RAII views) and `ModComponent::ZIG_DECL` generators |

---

## 3. Hygienic Crate Resolution (`attrs.rs`)

Downstream consumers frequently import `bamabi_sys` indirectly through a facade crate (`bamabi`, `bamabi_host`, or `bamabi_guest`) or rename dependencies in `Cargo.toml`.

### `resolve_sys_crate_path` / `resolve_sys_crate_with`
1. **Explicit override:** If `#[mod_component(crate = path)]` or `#[guest_component(crate = path)]` (accepting either a bare path or string literal via `parse_crate_path_value`) is present, emit `#path` verbatim.
2. **Direct `bamabi_sys`:** Probes `proc_macro_crate::crate_name("bamabi_sys")`:
   - `FoundCrate::Itself` when `CARGO_CRATE_NAME == "bamabi_sys"` → `crate` (avoids breaking unit tests inside `bamabi_sys`).
   - `FoundCrate::Itself` otherwise (e.g., `bamabi_sys` integration tests) → `::bamabi_sys`.
   - `FoundCrate::Name(alias)` → `::#alias`.
3. **Facade crates in priority order:** Probes `["bamabi", "bamabi_host", "bamabi_guest"]` sequentially:
   - `FoundCrate::Itself` when `CARGO_CRATE_NAME == facade` → `crate::bamabi_sys`.
   - `FoundCrate::Itself` otherwise → `::#facade::bamabi_sys`.
   - `FoundCrate::Name(alias)` → `::#alias::bamabi_sys`.
4. **Default fallback:** `::bamabi_sys`.

### `resolve_guest_crate_path` / `resolve_guest_crate_with`
1. Explicit `guest_crate = path` override → `#path`.
2. `proc_macro_crate::crate_name("bamabi_guest")`:
   - `FoundCrate::Itself` when `CARGO_CRATE_NAME == "bamabi_guest"` → `crate`.
   - `FoundCrate::Itself` otherwise → `::bamabi_guest`.
   - `FoundCrate::Name(alias)` → `::#alias`.
3. Default fallback: `::bamabi_guest`.

---

## 4. AST Parsing, Validation & Syntactic Classification

### Layout & Attribute Validation (`attrs.rs`)
- **`parse_repr_flags`:** Inspects `#[repr(...)]` for `C`, `transparent`, `packed` (including `packed(N)`), `u8..u64`, `i8..i64`, `Rust`, `align(...)`, and flags unsupported integer reprs (`usize`, `isize`, `u128`, `i128`) so enum derives fail cleanly.
- **`parse_component_attrs` (`#[mod_component(...)]`):** Parses `name`, `namespace`, `uuid`, `prefix`, `crate`, `guest_crate`, `guest`, `export_symbols`, `no_pod`, `opaque`, `sparse_set`, `immutable`, `summary_tick`, and `require(CompA, CompB = expr)`.
  - Rejects empty names, interior NUL bytes (unsafe for `CStr`), malformed `::` separators (leading/trailing `::` or `::::`), invalid `uuid` hex strings (`is_valid_uuid_hex`: only 36-char `8-4-4-4-12` or 32-char hex), `sparse_set` + `summary_tick` (Bevy 0.20 `summary_tick` requires `Table` storage), and combining `namespace = "..."` with a `name = "ns::Name"` containing `::`.
- **`parse_guest_component_attrs` (`#[guest_component(...)]`):** Parses `#[guest_component(...)]` keys and falls back to `#[mod_component(...)]` when absent (or merges overrides + ORs storage flags when both are present).
- **`parse_abi_pod_crate_override`:** For standalone `#[derive(AbiPod)]`, permits `#[mod_component(crate = ...)]` (rejecting any other `#[mod_component(...)]` key) and best-effort extracts `crate = ...` from a companion `#[guest_component(...)]` when deriving `#[derive(AbiPod, GuestComponent)]`.

### Syntactic Classification & Anti-Spoofing (`classify.rs` + `vtable.rs`)
Because procedural macros inspect syntax before type resolution, `classify_type` follows a two-layer soundness protocol:
1. **Explicit `#[mod_field(...)]` always wins:** `skip`, `bool`, `primitive`, `string`, `vec` / `vec(ElemTy)`, `option` / `option(InnerTy)`, or `nested` overrides syntactic heuristics, enabling type aliases (`type Label = String;`) and disambiguating shadowed local types named `String` or `Vec`.
2. **Syntactic path matching + compile-time trait guards:**
   - `bool` / `core::primitive::bool` / `std::primitive::bool` → `FieldKind::Bool` (guarded by `trait __BamPrimBool`, implemented only for `bool`).
   - `usize` / `isize` → `FieldKind::Primitive { is_abi_pod: false }` (guarded by `assert_raw_insertable::<T>()` + `Copy + Default`).
   - `u128` / `i128` → `FieldKind::Nested { is_abi_pod_candidate: false }` (guarded by `assert_raw_insertable::<T>()`, exposed via interior pointers rather than by-value `extern "C"`).
   - Fixed-width primitives & handles (`f32`, `f64`, `i8..i64`, `u8..u64`, `FfiBool`, `FfiEntity`, `TargetEntity`, `BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `FfiAssetLoadState`, `BamEventKind`, `BamFieldKind`, `ComponentUuid`) → `FieldKind::Primitive { is_abi_pod: true }`.
   - `String` (unqualified or `std::string::String` via last segment `String` with no generic args) → `FieldKind::HostString` (guarded by `trait __BamStdString`, implemented only for `::std::string::String`).
   - `Vec<ElemTy>` → `FieldKind::HostVec` (guarded by `trait __BamStdVec<Elem>` and `assert_abi_pod::<Elem>()`).
   - `FfiOption<InnerTy>` → `FieldKind::FfiOption` (guarded by `trait __BamFfiOption<Inner>` and `assert_abi_pod::<Inner>()`).
   - References (`&T`), raw pointers (`*const T`/`*mut T`), unsized slices (`[T]`), and Rust-layout/heap containers (`Box`, `Rc`, `Arc`, `Option`, `Result`, `HashMap`, `BTreeMap`, `HashSet`, `BTreeSet`, `Cow`, `VecDeque`) → **compile error** unless `#[mod_field(skip)]`.
   - All other paths/arrays → `FieldKind::Nested { is_abi_pod_candidate: true }`, checked with `assert_raw_insertable` (or `ModComponent` if `opaque`) and `assert_abi_pod` when the parent qualifies for `AbiPod`.

If a user defines a local `struct String;` without `#[mod_field(nested)]`, `classify_type` selects `HostString` and the emitted `__BamStdString` bound fails at compile time with a `#[diagnostic::on_unimplemented(...)]` error pointing directly at the field—never producing unsound memory access.

---

## 5. Derive Entry Points

### `#[derive(AbiPod)]` (`abi_pod.rs`)
1. Rejects generics, enums/unions, `#[repr(packed)]`, and structs without `#[repr(C)]` or `#[repr(transparent)]`.
2. Emits a `const _: ()` verification block enforcing:
   - `Copy + 'static` via `assert_self_copy::<#struct_ident>()`.
   - Per-field `assert_abi_pod::<#field_ty>()` with the field's source `Span` (rejecting `bool`, `char`, `usize`, `isize`, `u128`, `i128`, enums, and heap types).
   - `!core::mem::needs_drop::<#struct_ident>()`.
   - Zero implicit compiler padding (`size_of::<#struct_ident>() == sum(size_of::<#field_ty>())`).
3. Emits `unsafe impl RawInsertable` and `unsafe impl AbiPod`.

### `#[derive(GuestComponent)]` (`guest_component.rs`)
1. Rejects generics, enums/unions, `#[repr(packed)]`, and structs lacking `#[repr(C)]` / `#[repr(transparent)]`.
2. Computes `NAME` (`LitCStr`), `FIELD_SCHEMA_HASH` (chaining `ComponentUuid::hash_field_schema` over `(field_name, canonical_field_type_tag, offset_of!)`, or `0u64` with `no_schema_hash`), bitmask `FLAGS`, and optional `UUID` (`parse_hex`).
3. Emits **only** `impl #guest_crate::GuestComponent for #struct_ident` inside `const _: ()`—relying on `GuestComponent: AbiPod` so `#[derive(AbiPod, GuestComponent)]` enforces all POD checks with zero VTable/C-decl overhead.

### `#[derive(ModComponent)]` (`codegen/mod.rs`)
1. Rejects generics, unions, and `#[repr(packed)]`.
2. Resolves `canonical_name` (`Literal` from `namespace` / namespaced `name`, or `ModulePathPrefixed` using `core::module_path!()`) and `export_fn_prefix` (`bam_<ns>_<Struct>` or `prefix = "..."`).
3. Initializes `CodegenState` with `can_impl_raw_insertable = !opaque` and `can_impl_abi_pod = !opaque && repr_flags.is_deterministic_unpacked() && !no_pod`.
4. Reserves `"header"` in `used_vtable_fields` and pushes `"bam.v0.<canonical_name>.vtable"` into `symbol_entries`.
5. Dispatches enums to `enums::expand_enum_mod_component` or iterates struct fields (`Named`, `Unnamed` indexed `"0"`, `"1"`, ..., or `Unit`).

---

## 6. `codegen/` Pipeline Deep Dive

### Unified Slot Driver (`codegen/mod.rs`)
Each field module (`primitive`, `bool`, `option`, `string`, `vec`, `nested`) returns an `AccessorSpec` containing:
- `allows_raw_insertable` and `allows_abi_pod` flags (combined via `&=` into `CodegenState`).
- Spanned compile-time `assertions` + optional `pod_assertion` (emitted only if `is_abi_pod == true`).
- `slots: Vec<SlotSpec>` describing each VTable slot (`prefix`, `ReceiverKind`, extra params, return type, `valid_body`).

`CodegenState::emit_accessor` processes every `SlotSpec` uniformly:
- Checks `used_vtable_fields` and `used_export_names` to reject duplicate/colliding slot names (e.g., field `x` + `mut_x` colliding on `ptr_mut_x`).
- Wraps `ReceiverKind::Const` with `if ptr.is_null() || !ptr.cast::<T>().is_aligned() { return on_invalid; }`.
- Wraps `ReceiverKind::Mut` with `if ptr.is_null() { return on_null; }`, optional `pre_align_check` (e.g., checking `view.len > 0 && view.ptr.is_null()` or `out_elem` validity before mutating), and `if !ptr.cast::<T>().is_aligned() { return on_unaligned; }`.
- Emits `unsafe extern "C" fn __bam_<Struct>_<slot>(...)` (with `#[unsafe(export_name = ...)]` when `export_symbols` is active), the `<Struct>VTable` `Option<unsafe extern "C" fn(...)>` field, its `Some(...)` initializer, and its `SymbolEntry` `"bam.v0.<canonical_name>.<slot>"`.

### Alias-Safe `String` and `Vec<T>` Mutation (`codegen/vtable.rs`)
If a guest reads a `StrView` via `get_tag` and passes a sub-slice of that same buffer back into `set_tag`, creating `&mut String` before copying would cause undefined behavior (overlapping mutable borrow and source slice). `__bam_write_string` and `__bam_write_vec` prevent this by inspecting the existing heap buffer interval `[as_ptr(), as_ptr() + capacity_bytes)` via raw `&*ptr` **before** constructing any `&mut`:
- If `[src_start, src_end)` overlaps `[buf_start, buf_end)`, the source bytes are cloned into a temporary `String` / `Vec<Elem>` first, then written into the cleared host container.
- Invalid UTF-8 in `__bam_write_string` returns `FfiResult::INVALID_UTF8` before mutating the host `String`.
- `push` and `reserve` on `Vec<T>` use `try_reserve` and return `FfiResult::CAPACITY_EXCEEDED` on allocation failure rather than aborting.

### Enums (`codegen/enums.rs`)
Enums always set `can_impl_raw_insertable = false` and `can_impl_abi_pod = false` (arbitrary discriminant bytes are UB) and reject `#[mod_component(guest)]`:
- **Fieldless enums** require `#[repr(u8..u64|i8..i64)]` or `#[repr(C)]`.
- **Data-carrying enums** require `#[repr(C, u8..u64|i8..i64)]`.
- Discriminants must be integer literals (or negated integer literals, supporting `i64::MIN`..`u64::MAX`) fitting within `IntRepr`.
- Emits compile-time `size_of` and discriminant value assertions, plus `tag(ptr) -> TagInt`, `set_<var_snake>(ptr, args...) -> FfiResult` (omitted if a variant has `#[mod_field(skip)]`), and `try_get_<var_snake>_<field>(ptr, out_val) -> FfiResult` (returning `FfiResult::INVALID_STATE` when the active variant differs).

### Identity, Fingerprinting & Reflection (`codegen/vtable.rs`)

1. **`ComponentUuid` Parity (`ModComponent::UUID` vs. `GuestComponent::UUID`):**
   - Explicit `uuid = "..."` → `ComponentUuid::parse_hex(hex)`.
   - `AbiPod` structs → `ComponentUuid::from_parts(CANONICAL_NAME, size_of::<Self>(), align_of::<Self>(), pod_schema_hash)`, where `pod_schema_hash` folds each non-skipped field via `ComponentUuid::hash_field_schema(h, field_name, type_tag, offset_of!(Self, field))`. This matches `#[derive(GuestComponent)]`'s `FIELD_SCHEMA_HASH` **bit-for-bit**, ensuring host and guest derive identical UUIDs.
   - Enums → `ComponentUuid::from_parts` hashing `"__enum_tag"`, each variant `(variant_name, "variant:<disc>")`, and each payload field `("<Var>::<field>", type_tag, field_idx)`.
   - Non-POD / opaque structs → `ComponentUuid::from_canonical_name(CANONICAL_NAME)` (`size = 0, align = 0, schema = 0`).

2. **`SCHEMA_FINGERPRINT` (64-bit FNV-1a):**
   Computed in `const` via `bamabi_sys::fnv1a_64_*`:
   - Starts from `fnv1a_64_init()` (`0xcbf2_9ce4_8422_2325`).
   - Folds `CANONICAL_NAME.as_bytes()`, `size_of::<Self>() as u64`, `align_of::<Self>() as u64`, `COMPONENT_FLAGS as u64`.
   - For every field in `BAM_FIELDS` (including skipped fields as `OPAQUE`): folds `(BamFieldKind).0 as u64`, `name.as_bytes()`, `type_tag.as_bytes()`, `offset_of! as u64`, `size_of::<FieldTy>() as u64`, `align_of::<FieldTy>() as u64`.
   - For enums: folds tag type name, each variant's `(name, discriminant as u64)`, and each variant field's `(name, type_tag)`.

3. **`BAM_FIELDS` & `REQUIRED_COMPONENTS`:**
   - `BAM_FIELDS: &'static [BamFieldDesc]` records every field's `BamFieldKind`, name, type tag, `offset_of!`, `size_of`, and `align_of` (driving `ReflectFromPtr` and BRP reflection in downstream `bamabi`).
   - `require(CompA, CompB = expr)` emits compile-time `T: ModComponent` assertions and populates `REQUIRED_COMPONENTS: &'static [BamRequiredComponent]`. Entries with `= expr` emit `unsafe extern "C" fn __bam_req_init_<idx>(dst, _user_data) -> FfiResult` and attach `with_init_fn(...)` (`BAM_REQUIRED_FLAG_INIT_FN | BAM_REQUIRED_FLAG_RESOLVE_BY_UUID`).

### Witness Reflection & Schema Migration (`codegen/witness.rs`)
For all non-`opaque` components, `emit_struct_witnesses` / `emit_enum_witnesses` registers three `"bam.v0.<canonical_name>.*"` symbols:
- **`field_info(index, out_name, out_type_tag, out_offset) -> FfiBool`**: Enumerates non-skipped fields (for enums, index `0` is `("__variant", "enum_variant", 0)` and `1..` are `"<Variant>::<field>"`).
- **`export_fields(src_ptr, sink, sink_ctx) -> FfiResult`**: Streams each field as `(name, type_tag, bytes)`:
  - `bool`: 1 byte (`0` or `1`, tag `"bool"`).
  - `Primitive`: raw `size_of` bytes.
  - `String`: UTF-8 bytes (tag `"String"`).
  - `Vec<T>`: packed element bytes (tag `"Vec<Elem>"`).
  - `FfiOption<T>`: `[0]` for `None`, or `[1, ...inner_bytes]` for `Some` (never reading uninitialized payload bytes on `None`).
  - `Nested`: Uses Rust autoref specialization (`&&__BamExportWrap<T>` → `ModComponent::export_fields` with `prefix.` prepending; `&__BamExportWrap<T>` → `RawInsertable` raw bytes; `__BamExportWrap<T>` → `UNSUPPORTED`).
- **`import_field(dst_ptr, field_name, type_tag, payload) -> FfiResult`**: Restores exported fields into a new version of a struct/enum:
  - Performs **lossless primitive widening** via `&&__BamImportWrap<Target>` (`i8`→`i16/i32/i64/i128`, `i16`→`i32/i64/i128`, `i32`→`i64/i128`, `i64`→`i128`, `u8`→`u16/u32/u64/u128`, `u16`→`u32/u64/u128`, `u32`→`u64/u128`, `u64`→`u128`, `f32`→`f64`).
  - Validates UTF-8 on `String`, exact element-size multiples on `Vec<T>`, and tag byte + length on `FfiOption<T>`.
  - Strips `"field."` prefixes to delegate into nested `ModComponent::import_field`.
  - On enums, `"__variant"` switches variants using `__BamDefaultWrap` (`Default` or zeroed `AbiPod`, returning `UNSUPPORTED` if neither holds) before importing `"<Variant>::<field>"` payloads.

### `C_DECL` and `ZIG_DECL` Emission (`codegen/c_decl.rs`)
- Emits full `typedef struct` / `extern struct` definitions for non-empty `IS_ABI_POD` structs (emitting helper `*_Option` structs beforehand), or opaque forward declarations (`typedef struct X X;` / `pub const X = opaque {};`) for `#[repr(Rust)]`, ZST, or enum types.
- Emits `<Struct>VTable` with `BamVTableHeader header;` at offset `0`, plus `BAM_STATIC_ASSERT` checks for `offsetof(..., header) == 0u` and exact 64-bit (`40 + slots * 8`) / 32-bit (`32 + slots * 4`) struct sizes.
- Appends `#ifdef __cplusplus` C++17 `noexcept` RAII helpers (`bam::resolve_vtable`, `VTableHandle`, `UniqueGuest`, `HostVecView`, `ConstHostVecView`, `<Struct>ConstView`, `<Struct>View`) and Zig `comptime`-verified `<Struct>View` wrappers.

---

## 7. Extending `bamabi_macros`

### Recipe: Adding Support for a New Field Type or Handle
1. **Wire trait / C declaration:** Ensure the type implements `AbiPod` (or `RawInsertable`) in `crates/bamabi_sys/src/traits.rs`, and has C99/Zig typedefs in `bamabi_sdk_gen` / `include/bamabi.h`.
2. **Syntactic classification (`src/classify.rs`):**
   - If passed by value as a primitive handle, add its unqualified ident to the `FieldKind::Primitive` arm in `classify_type`.
   - If it needs custom VTable slots (like `FfiOption<T>`), add a variant to `FieldAttrOverride` (`src/attrs.rs`), `FieldKind` (`src/classify.rs`), `WitnessFieldKind` (`src/codegen/witness.rs`), and `DeclFieldKind` (`src/codegen/c_decl.rs`).
3. **Codegen & Reflection (`src/codegen/`):**
   - Create or update the `AccessorSpec` builder (`src/codegen/<kind>.rs`), wire it in `src/codegen/mod.rs` (including `infer_bam_field_kind_ident`), add any anti-spoofing helper trait in `vtable.rs`, add witness export/import logic in `witness.rs`, and add C99/C++17/Zig declarations in `c_decl.rs` (`map_scalar_type` and slot rendering).
4. **Tests:** Add unit tests in `src/tests.rs`, runtime + witness round-trip checks in `tests/mod_component_vtable.rs` (and `tests/proptest_witness.rs` if applicable), and `trybuild` compile-fail UI fixtures in `tests/ui/` for any invalid type/layout combinations.

### Recipe: Adding a New `#[mod_component(...)]` or `#[guest_component(...)]` Attribute
1. Add the field to `ComponentAttrs` / `GuestComponentAttrs` and parse it in `parse_component_attrs` / `parse_guest_component_attrs` (`src/attrs.rs`), adding mutual-exclusion validation if needed.
2. Thread it through `CodegenState` (`src/codegen/mod.rs`) or `expand_guest_component_inner` (`src/guest_component.rs`), updating `vtable.rs` / `enums.rs` as needed.
3. Update `GuestComponent` fallback merging in `parse_guest_component_attrs` if the attribute applies to both host and guest.
4. Add expansion assertions in `src/tests.rs` and positive/negative `trybuild` UI fixtures under `tests/ui/`.

---

## 8. Setup, Building, Macro Expansion, Testing & Profiling

### Setup & Build
```bash
# Build just bamabi_macros
cargo build -p bamabi_macros --all-features

# Check formatting, clippy, rustdoc, and zero-Bevy dependency hygiene
cargo fmt -p bamabi_macros -- --check
cargo clippy -p bamabi_macros --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_macros --no-deps --all-features
cargo tree -p bamabi_macros | grep -i bevy   # Must print nothing
```

### Inspecting Macro Expansion (`cargo expand`)
To inspect the exact tokens emitted by `#[derive(ModComponent)]`, `#[derive(AbiPod)]`, or `#[derive(GuestComponent)]`:
```bash
cargo install cargo-expand   # once, if not already installed
cargo expand -p bamabi_macros --test mod_component_vtable
```

### Testing & Miri Verification
```bash
# Unit tests (src/tests.rs), trybuild UI suite (34 fixtures), VTable integration tests, and proptest witnesses
cargo test -p bamabi_macros --all-features
cargo test --doc -p bamabi_macros --all-features

# Miri UB check over VTable getters/setters, self-aliasing String/Vec writes, enums, and witness round-trips
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test mod_component_vtable
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test proptest_witness
```
*(Note: `trybuild` UI tests are gated with `#[cfg_attr(miri, ignore)]` because Miri does not execute nested `cargo` subprocesses.)*

### Benchmarking & Compile-Time Profiling Notes
- **Runtime benchmarks:** Layer 0 (`bamabi-wire`) intentionally contains **no Criterion runtime benchmarks**. End-to-end VTable dispatch, column iteration, and ECS overhead benchmarks live in the downstream `bamabi-showcase` repository.
- **Macro compile-time profiling:** To measure `bamabi_macros` proc-macro expansion overhead or generated-code compile times on downstream crates, use `cargo build --timings` or `RUSTFLAGS="-Z self-profile"` on nightly.
