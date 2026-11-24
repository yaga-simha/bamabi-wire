#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! # `bamabi_macros`: Incremental Guide to ABI `0.3.0` Procedural Macros
//!
//! Zero-Bevy-dependency procedural macros (`#![forbid(unsafe_code)]`) for `bamabi` (`ABI 0.3.0`,
//! `v0.3.0`) providing [`AbiPod`], [`ModComponent`], and [`GuestComponent`].
//!
//! Read this page in order as a 6-step onboarding path, or jump to the section you need:
//!
//! ## Workspace & Crate Documentation
//!
//! - **Crate guides:** [Architecture Guide](#bamabi_macros-architecture--implementation-guide) (`crates/bamabi_macros/ARCHITECTURE.md`) · [Test & Verification Guide](#bamabi_macros-test--verification-guide) (`crates/bamabi_macros/TESTS.md`)
//! - **Workspace guides:** [`bamabi_docs`](../bamabi_docs/index.html) ([`architecture`](../bamabi_docs/architecture/index.html), [`development`](../bamabi_docs/development/index.html), [`testing`](../bamabi_docs/testing/index.html))
//! - **Sibling crates:** [`bamabi_sys`](../bamabi_sys/index.html) · [`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html) · [`bamabi_guest_sys`](../bamabi_guest_sys/index.html) · [`bamabi_guest`](../bamabi_guest/index.html) · [`xtask`](../xtask/index.html)
//!
//! > Note: `bamabi_macros` is a `proc-macro` crate, which Rust forbids from exporting `pub mod` items; therefore its `ARCHITECTURE.md` and `TESTS.md` are embedded directly below on the crate root doc page.
//!
//! 1. [Step 1: Why `bamabi_macros` Exists & `#![forbid(unsafe_code)]` Hygiene](#step-1-why-bamabi_macros-exists--forbidunsafe_code-hygiene)
//! 2. [Step 2: Choosing a Derive (`AbiPod` vs `ModComponent` vs `GuestComponent`)](#step-2-choosing-a-derive-abipod-vs-modcomponent-vs-guestcomponent)
//! 3. [Step 3: Attribute & Type-Support Matrix](#step-3-attribute--type-support-matrix)
//! 4. [Step 4: What Gets Generated](#step-4-what-gets-generated)
//! 5. [Step 5: Internal Module Tour & Architecture / Test Docs](#step-5-internal-module-tour--architecture--test-docs)
//! 6. [Step 6: Setup, Build, Test & Verification](#step-6-setup-build-test--verification)
//!
//! ---
//!
//! ## Step 1: Why `bamabi_macros` Exists & `#![forbid(unsafe_code)]` Hygiene
//!
//! Rust's default `#[repr(Rust)]` struct layout is unspecified, and passing `String`, `Vec<T>`,
//! `bool`, or `Option<T>` across dynamic-library (`.cdylib`) boundaries directly risks allocator
//! mismatches, compiler layout divergence, and invalid bit-pattern undefined behavior.
//! `bamabi_macros` bridges Rust host components and C99 / C++17 / Zig / Rust guest mods over a
//! single deterministic C-ABI contract without pulling any `bevy_*` crate or `bamabi_guest` into
//! the macro dependency graph (only `syn`, `quote`, `proc-macro2`, and `proc-macro-crate`).
//!
//! ### `#![forbid(unsafe_code)]` vs. Generated `unsafe`
//!
//! - **Macro compiler crate:** `bamabi_macros` itself compiles under `#![forbid(unsafe_code)]`—no
//!   `unsafe` runs at macro-expansion time.
//! - **Generated `const _: ()` scopes:** Every emitted `unsafe impl AbiPod`, `unsafe impl
//!   RawInsertable`, `unsafe impl ExtensibleStruct`, and `unsafe extern "C" fn` trampoline is
//!   wrapped inside a private `const _: () = { ... };` hygiene block and gated by compile-time
//!   assertions (`Copy + 'static`, `!needs_drop::<Self>()`, zero implicit padding, `assert_abi_pod`,
//!   and anti-spoofing trait bounds) plus runtime null/alignment checks on every pointer receiver.
//!
//! ### Hygienic Crate Resolution
//!
//! Downstream crates rarely depend on `bamabi_sys` directly—they usually re-export it through
//! `bamabi`, `bamabi_host`, or `bamabi_guest`. `attrs::resolve_sys_crate_path` probes Cargo
//! dependencies via `proc-macro-crate` in deterministic order:
//!
//! 1. Explicit `#[mod_component(crate = path)]` or `#[guest_component(crate = path)]` override.
//! 2. Direct `bamabi_sys` dependency (emitting `crate` when compiled inside `bamabi_sys`, or
//!    `::bamabi_sys` / renamed alias otherwise).
//! 3. Facade crates in order: `bamabi` → `bamabi_host` → `bamabi_guest`, emitting
//!    `::<facade>::bamabi_sys` (or `crate::bamabi_sys` when compiling inside the facade itself).
//! 4. Fallback `::bamabi_sys`.
//!
//! Similarly, `attrs::resolve_guest_crate_path` resolves `bamabi_guest` via `guest_crate = path`
//! override or `proc-macro-crate` without adding any Cargo dependency on `bamabi_guest`.
//!
//! ---
//!
//! ## Step 2: Choosing a Derive (`AbiPod` vs `ModComponent` vs `GuestComponent`)
//!
//! | Derive | Target Use Case | Required Repr | Emitted Trait Impls | Overhead |
//! |---|---|---|---|---|
//! | [`AbiPod`] | Standalone wire/nested POD structs | `#[repr(C)]` or `#[repr(transparent)]`, non-packed, zero implicit padding | `RawInsertable`, `AbiPod` | Zero runtime overhead (compile-time assertions only) |
//! | [`ModComponent`] | Host components, nested sub-structs, C-ABI enums | Any struct (non-packed) or `#[repr(C)]` / `#[repr(u8..i64)]` enum | `ModComponent`, `<Struct>VTable`, `ExtensibleStruct`, conditional `RawInsertable` / `AbiPod` / `GuestComponent` | Static VTable + `"bam.v0.*"` symbols + witness functions + `C_DECL`/`ZIG_DECL` |
//! | [`GuestComponent`] | Lightweight guest POD components in `.cdylib` mods | `#[repr(C)]` or `#[repr(transparent)]`, non-packed (`AbiPod`) | `bamabi_guest::GuestComponent` **only** | Zero VTable or C/Zig declaration overhead |
//!
//! ### Decision Rule
//!
//! - **Pure wire or nested POD struct?** Use `#[derive(Copy, Clone, AbiPod)]`.
//! - **Host ECS component or rich struct (`String`, `Vec<T>`, `bool`, `#[repr(Rust)]`, enum)?**
//!   Use `#[derive(ModComponent)]`. If it is `#[repr(C)]` with zero implicit padding and all
//!   `AbiPod` fields, `ModComponent` also emits `RawInsertable` and `AbiPod` automatically (unless
//!   `no_pod` or `opaque` is set).
//! - **Guest `.cdylib` component?** Pair `#[derive(Copy, Clone, AbiPod, GuestComponent)]` for zero
//!   VTable bloat, or use `#[derive(ModComponent, GuestComponent)]` / `#[mod_component(..., guest)]`
//!   when sharing a single definition between host and guest.
//!
//! ### Runnable Examples
//!
//! ```
//! use bamabi_macros::{AbiPod, ModComponent};
//! use bamabi_sys::{
//!     AbiPod as AbiPodTrait, FfiBool, FfiOption, FfiResult, ModComponent as ModComponentTrait,
//!     StrView,
//! };
//!
//! // 1. Standalone POD struct with explicit padding (`_pad`).
//! #[repr(C)]
//! #[derive(Copy, Clone, AbiPod)]
//! pub struct HealthPod {
//!     pub current: i32,
//!     pub max: i32,
//!     pub alive: FfiBool,
//!     pub _pad: [u8; 3],
//! }
//! bamabi_sys::assert_abi_pod::<HealthPod>();
//!
//! // 2. Rich `#[repr(Rust)]` host component with String, Vec<T>, bool, FfiOption<T>, and skipped state.
//! #[derive(ModComponent)]
//! #[mod_component(name = "docs_example::Actor")]
//! pub struct Actor {
//!     pub name: String,
//!     pub active: bool,
//!     pub scores: Vec<u32>,
//!     pub target: FfiOption<u32>,
//!     #[mod_field(skip)]
//!     pub host_cache: Option<String>,
//! }
//!
//! assert_eq!(Actor::CANONICAL_NAME, "docs_example::Actor");
//! assert!(!Actor::IS_ABI_POD);
//! assert!(!Actor::IS_RAW_INSERTABLE);
//!
//! let mut actor = Actor {
//!     name: String::from("knight"),
//!     active: false,
//!     scores: vec![10],
//!     target: FfiOption::none(),
//!     host_cache: Some(String::from("private")),
//! };
//! let vt = Actor::vtable();
//! let ptr = (&raw mut actor).cast::<core::ffi::c_void>();
//! unsafe {
//!     assert_eq!((vt.set_active.unwrap())(ptr, FfiBool::TRUE), FfiResult::OK);
//!     let new_name = StrView::from_str("paladin");
//!     assert_eq!((vt.set_name.unwrap())(ptr, &raw const new_name), FfiResult::OK);
//!     assert_eq!((vt.push_scores.unwrap())(ptr, 25), FfiResult::OK);
//! }
//! assert!(actor.active);
//! assert_eq!(actor.name, "paladin");
//! assert_eq!(actor.scores, vec![10, 25]);
//! ```
//!
//! Types with implicit compiler padding or non-POD fields are rejected at compile time by
//! [`AbiPod`]:
//!
//! ```compile_fail
//! use bamabi_macros::AbiPod;
//!
//! #[repr(C)]
//! #[derive(Copy, Clone, AbiPod)]
//! pub struct UnpaddedTail {
//!     pub id: u64,
//!     pub flag: u8, // 7 bytes of implicit trailing padding!
//! }
//! ```
//!
//! ---
//!
//! ## Step 3: Attribute & Type-Support Matrix
//!
//! ### Field Type Classification (`classify.rs`)
//!
//! Procedural macros run before Rust's type resolver, so `classify::classify_type` combines
//! syntactic path matching with compile-time anti-spoofing trait bounds (`__BamPrimBool`,
//! `__BamStdString`, `__BamStdVec`, `__BamFfiOption`) and `#[mod_field(...)]` overrides:
//!
//! | Field Type | Generated VTable Slots | `RawInsertable`? | `AbiPod`? | Compile-Time Guard |
//! |---|---|---|---|---|
//! | `u8..u64`, `i8..i64`, `f32`, `f64`, `FfiBool`, `FfiEntity`, `TargetEntity`, `BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `FfiAssetLoadState`, `BamEventKind`, `BamFieldKind`, `FfiChildOf`, `FfiAssetHandle`, `BamChangeTicks`, `ComponentUuid` | `get_<f>`, `set_<f>` | Yes | Yes | `assert_abi_pod::<T>()` + `Default` |
//! | `usize`, `isize` | `get_<f>`, `set_<f>` | Yes | **No** | `assert_raw_insertable::<T>()` + `Copy + Default` |
//! | `u128`, `i128` | `ptr_<f>`, `ptr_mut_<f>` | Yes | **No** | `assert_raw_insertable::<T>()` |
//! | `bool` | `get_<f>` (`-> FfiBool`), `set_<f>` (`FfiBool -> FfiResult`) | **No** | **No** | `__BamPrimBool` |
//! | `FfiOption<T>` (`T: AbiPod`) | `get_<f>`, `set_<f>`, `clear_<f>` | Yes | Yes | `__BamFfiOption<T>` + `assert_abi_pod::<T>()` |
//! | `String` | `get_<f>` (`-> StrView`), `set_<f>` (`StrView -> FfiResult`) | **No** | **No** | `__BamStdString` |
//! | `Vec<T>` (`T: AbiPod`) | `get`, `set`, `len`, `clear`, `push`, `pop`, `remove`, `swap_remove`, `truncate`, `reserve`, `as_mut_slice` (11 slots) | **No** | **No** | `__BamStdVec<T>` + `assert_abi_pod::<T>()` |
//! | Nested structs & `[T; N]` arrays | `ptr_<f>` (`-> *const c_void`), `ptr_mut_<f>` (`-> *mut c_void`) | Yes (unless `opaque`) | If `#[repr(C)]` + candidate | `assert_raw_insertable::<T>()` (or `ModComponent` if `opaque`) + `assert_abi_pod::<T>()` when POD |
//! | C-ABI enums (`#[repr(u8..i64)]` / `#[repr(C)]` fieldless, `#[repr(C, u8..i64)]` data-carrying) | `tag`, `set_<variant>`, `try_get_<variant>_<field>` | **No** | **No** | Discriminant range & `size_of` / discriminant checks |
//! | `Box`, `Rc`, `Arc`, `Option`, `Result`, `HashMap`, `BTreeMap`, `HashSet`, `BTreeSet`, `Cow`, `VecDeque`, `&T`, `*const T`, `[T]` | *Compile error unless `#[mod_field(skip)]`* | **No** | **No** | Rejected in `classify_type` |
//!
//! ### `#[mod_component(...)]` Container Attributes
//!
//! - `name = "..."`: Full canonical name if it contains `"::"` (e.g. `"physics::Position"`), or
//!   short name prefixed by `module_path!()` (or `namespace = "..."`).
//! - `namespace = "..."`: Explicit namespace prefix overriding `module_path!()` (cannot be
//!   combined with an already-namespaced `name = "ns::Name"`).
//! - `uuid = "..."`: Explicit 36-char hyphenated (`8-4-4-4-12`) or 32-char hex UUID literal
//!   validated at macro-expansion time and materialized via `ComponentUuid::from_bytes`.
//! - `prefix = "..."`: Custom C export symbol stem for `export_symbols` (auto-prefixed with `bam_`
//!   if not already starting with `bam_`).
//! - `crate = path` / `"path"`: Explicit path to `bamabi_sys` (also supported on `#[derive(AbiPod)]`).
//! - `guest` / `guest_crate = path`: Also emits `impl bamabi_guest::GuestComponent for #Ident`
//!   delegating to `ModComponent` (requires `IS_ABI_POD == true`, rejected on enums or non-POD).
//! - `export_symbols`: Emits `#[unsafe(export_name = "bam_<ns>_<Struct>_<slot>")]` on generated
//!   `extern "C"` functions.
//! - `no_pod`: Suppresses `unsafe impl AbiPod` (`IS_ABI_POD = false`) while retaining
//!   `RawInsertable` when eligible.
//! - `opaque`: Marks the component VTable-only (`IS_RAW_INSERTABLE = false`, `IS_ABI_POD = false`),
//!   skips witness symbols (`field_info`, `export_fields`, `import_field`), and enforces
//!   `ModComponent` instead of `RawInsertable` on nested fields.
//! - `sparse_set` / `immutable` / `summary_tick`: Sets `BAM_COMPONENT_FLAG_*` bits in
//!   `ModComponent::COMPONENT_FLAGS` (`sparse_set` + `summary_tick` is rejected at compile time).
//! - `require(CompA, CompB = expr)`: Emits `REQUIRED_COMPONENT_NAMES`, `REQUIRED_COMPONENT_UUIDS`,
//!   and `ModComponent::REQUIRED_COMPONENTS` (`BamRequiredComponent`), generating an
//!   `extern "C" fn __bam_req_init_<idx>(dst, user_data) -> FfiResult` callback for `= expr`.
//!
//! ### `#[guest_component(...)]` Container Attributes
//!
//! - `name = "..."`, `namespace = "..."`, `uuid = "..."`, `no_schema_hash`, `sparse_set`,
//!   `immutable`, `summary_tick`, `crate = path`, `guest_crate = path`.
//! - When `#[guest_component(...)]` is absent, [`GuestComponent`] falls back to compatible keys
//!   on `#[mod_component(...)]`. When both are present, `#[guest_component(...)]` overrides
//!   naming/UUID/crate paths while OR-ing `sparse_set` / `immutable` / `summary_tick` flags.
//!
//! ### `#[mod_field(...)]` Field Overrides (Always Override Path Inspection)
//!
//! - `skip` — Omit VTable slots and schema fields (`IS_RAW_INSERTABLE = false`, `IS_ABI_POD = false`).
//! - `bool` — Force `FfiBool` marshaling (works with `type Flag = bool;`).
//! - `primitive` — Force by-value `get`/`set` slots (works with type aliases).
//! - `string` — Force host `::std::string::String` `StrView` slots.
//! - `vec` / `vec(ElemTy)` — Force 11 `HostVec` slots for `Vec<ElemTy>` aliases.
//! - `option` / `option(InnerTy)` — Force `FfiOption<InnerTy>` (`get`/`set`/`clear`) slots.
//! - `nested` — Force `*const c_void` / `*mut c_void` interior pointers (disambiguates shadowed
//!   local structs named `String` or `Vec`).
//!
//! ---
//!
//! ## Step 4: What Gets Generated
//!
//! For a struct `Foo` deriving `ModComponent`, the macro emits:
//!
//! 1. **`pub struct FooVTable` (`#[repr(C)]`, `Copy + Clone + Debug`):** Starts at byte offset `0`
//!    with `VTableHeader` (`struct_size`, `abi_version`, `uuid`, `size`, `align`), implements
//!    `ExtensibleStruct`, and stores `Option<unsafe extern "C" fn(...)>` accessor slots.
//! 2. **Inherent constants & methods on `Foo`:** `MOD_COMPONENT_NAME`, `SUMMARY_TICK`,
//!    `REQUIRED_COMPONENT_NAMES`, `REQUIRED_COMPONENT_UUIDS`, `REQUIRED_COMPONENTS`, `VTABLE`, and
//!    `Foo::vtable()`.
//! 3. **`impl ModComponent for Foo`:**
//!    - `CANONICAL_NAME` & `COMPONENT_CSTR`.
//!    - `UUID`: `ComponentUuid::from_bytes` if `uuid = "..."` is specified; `ComponentUuid::from_parts` with
//!      `hash_field_schema` chain over `(field_name, type_tag, offset_of!)` for `AbiPod` structs
//!      (or enum tag + variants); or `ComponentUuid::from_canonical_name` for non-POD structs.
//!    - `SCHEMA_FINGERPRINT`: 64-bit FNV-1a (`fnv1a_64_init`) hashing `CANONICAL_NAME`,
//!      `size_of::<Self>()`, `align_of::<Self>()`, `COMPONENT_FLAGS`, each field's
//!      `(BamFieldKind, name, type_tag, offset_of!, size_of, align_of)`, and enum tag/variants.
//!    - `BAM_FIELDS: &'static [BamFieldDesc]`: Static reflection table (all fields in order, with
//!      skipped or `opaque` fields tagged `BamFieldKind::OPAQUE`).
//!    - `REQUIRED_COMPONENTS: &'static [BamRequiredComponent]`: Dependencies & custom initializers.
//!    - `C_DECL` & `ZIG_DECL`: ISO C99 structs/VTables + `BAM_STATIC_ASSERT`s + `#ifdef __cplusplus`
//!      C++17 RAII view classes, and Zig `extern struct` + view wrappers.
//!    - `symbols()`: Static `SymbolEntry` slice registering `"bam.v0.<canonical_name>.vtable"`,
//!      every accessor slot, and (when non-opaque) the three witness symbols:
//!      `"bam.v0.<canonical_name>.field_info"`, `".export_fields"`, and `".import_field"`.
//! 4. **Zero-padding `const` assertion (`AbiPod` / POD `ModComponent`):**
//!    Asserts `size_of::<Self>() == sum(size_of::<FieldTy>())`, `!needs_drop::<Self>()`,
//!    `Copy + 'static`, and `assert_abi_pod::<FieldTy>()` for every field.
//!
//! ---
//!
//! ## Step 5: Internal Module Tour & Architecture / Test Docs
//!
//! - `abi_pod.rs` — `#[derive(AbiPod)]` layout, drop, and zero-padding verification.
//! - `guest_component.rs` — `#[derive(GuestComponent)]` zero-overhead `GuestComponent` impls.
//! - `attrs.rs` — `#[repr(...)]`, `#[mod_component(...)]`, `#[guest_component(...)]`,
//!   `#[mod_field(...)]`, UUID hex validation, and `proc-macro-crate` hygienic path resolution.
//! - `classify.rs` — Syntactic type classifier and canonical type-tag builder.
//! - `codegen/mod.rs` — Unified `CodegenState`, `AccessorSpec`, `SlotSpec`, `ReceiverKind`
//!   null/alignment validation, and symbol collision detection.
//! - `codegen/{primitive,bool,option,string,vec,nested}.rs` — Per-field `AccessorSpec` builders.
//! - `codegen/enums.rs` — Fieldless and `#[repr(C, <int>)]` data-carrying enum `tag` / `set_*` /
//!   `try_get_*` codegen.
//! - `codegen/vtable.rs` — `<Struct>VTable`, anti-spoofing helper traits, alias-safe `String`/`Vec`
//!   mutators, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, and `ModComponent` impl.
//! - `codegen/witness.rs` — `field_info`, `export_fields`, and `import_field` (including primitive
//!   widening `i8..i128`, `u8..u128`, `f32 -> f64` and nested `prefix.` recursion).
//! - `codegen/c_decl.rs` — `C_DECL` (C99 + C++17 views) and `ZIG_DECL` emission.
//!
//! For full implementation internals and test coverage matrices, see:
//! - `crates/bamabi_macros/ARCHITECTURE.md`
//! - `crates/bamabi_macros/TESTS.md`
//! - `crates/bamabi_macros/README.md`
//!
//! ---
//!
//! ## Step 6: Setup, Build, Test & Verification
//!
//! ```bash
//! cargo fmt -p bamabi_macros -- --check
//! cargo clippy -p bamabi_macros --all-targets --all-features -- -D warnings
//! RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_macros --no-deps --all-features
//! cargo test -p bamabi_macros --all-features
//! cargo test --doc -p bamabi_macros --all-features
//! MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test mod_component_vtable
//! ```
//!
//! Layer 0 contains no Criterion benchmarks; runtime VTable/ECS overhead benchmarks live in the
//! downstream `bamabi-showcase` repository.
#![cfg_attr(doc, doc = "\n\n---\n\n")]
#![cfg_attr(doc, doc = include_str!("../ARCHITECTURE.md"))]
#![cfg_attr(doc, doc = "\n\n---\n\n")]
#![cfg_attr(doc, doc = include_str!("../TESTS.md"))]

mod abi_pod;
mod attrs;
mod classify;
mod codegen;
mod guest_component;

#[cfg(test)]
mod tests;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Derive macro for standalone `#[repr(C)]` / `#[repr(transparent)]` cross-language Plain Old Data
/// (`AbiPod`) types.
///
/// # Intent
///
/// `AbiPod` marks types whose in-memory layout is deterministic across Rust, C99, C++17, and Zig
/// compilers, allowing direct zero-copy slice and column access across the C-ABI boundary.
///
/// # Expansion & Compile-Time Checks
///
/// Enforces at compile time that:
/// 1. The target is a struct annotated with `#[repr(C)]` or `#[repr(transparent)]` (and not
///    `packed`), with no type or lifetime generic parameters (`const` generic parameters such as
///    `const N: usize` are supported).
/// 2. Every field implements `AbiPod` via `assert_abi_pod::<FieldTy>()` (`u128`, `i128`, `usize`,
///    `isize`, `bool`, `char`, `String`, `Vec<T>`, enums, and `#[repr(Rust)]` structs are rejected).
/// 3. The struct implements `Copy + 'static`, has no drop glue (`!core::mem::needs_drop::<Self>()`),
///    and has zero implicit padding (`core::mem::size_of::<Self>() == sum(size_of::<FieldTy>())`).
#[proc_macro_derive(AbiPod, attributes(mod_component))]
pub fn derive_abi_pod(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match abi_pod::expand_abi_pod(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro for `ModComponent` host components, nested sub-structs, and C-ABI enums.
///
/// # Intent
///
/// Enables Bevy host components (including `#[repr(Rust)]` structs or types containing host-owned
/// `String` / `Vec<T>` fields) and C-ABI enums to be inspected and mutated by native guest mods
/// over `extern "C"` without JSON serialization or cross-allocator freeing.
///
/// # Expansion Behavior
///
/// Generates:
/// - A `#[repr(C)]` `<Struct>VTable` prefixed by `VTableHeader` (`struct_size`, `abi_version`,
///   `uuid`, `size`, `align`) with `Option<unsafe extern "C" fn(...)>` field accessor slots.
/// - `extern "C"` getter, error-returning `FfiResult` setter, `Vec` (11 slots), `FfiOption`,
///   interior-pointer, enum (`tag`/`set_*`/`try_get_*`), and witness (`field_info`,
///   `export_fields`, `import_field`) functions.
/// - A `ModComponent` implementation with deterministic 128-bit SipHash-2-4 `ComponentUuid`,
///   64-bit FNV-1a `SCHEMA_FINGERPRINT`, static `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL` /
///   `ZIG_DECL`, `SymbolEntry` metadata (`"bam.v0.<canonical_name>.*"`), and conditional
///   `RawInsertable` / `AbiPod` / `GuestComponent` implementations when layout and fields qualify.
#[proc_macro_derive(ModComponent, attributes(mod_component, mod_field))]
pub fn derive_mod_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match codegen::expand_mod_component(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro for lightweight `bamabi_guest::GuestComponent` implementations.
///
/// # Intent
///
/// Eliminates manual `impl GuestComponent` boilerplate for guest POD components while preserving
/// bit-for-bit `ComponentUuid` parity with host `#[derive(ModComponent)]` via `FIELD_SCHEMA_HASH`.
///
/// Emits **only** `impl bamabi_guest::GuestComponent for #Ident` (never emits `AbiPod` or
/// `RawInsertable`), so it composes cleanly with both `#[derive(AbiPod, GuestComponent)]` and
/// `#[derive(ModComponent, GuestComponent)]`.
#[proc_macro_derive(GuestComponent, attributes(guest_component, mod_component))]
pub fn derive_guest_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match guest_component::expand_guest_component(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
