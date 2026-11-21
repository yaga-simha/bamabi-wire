# `bamabi_macros`

Zero-Bevy-dependency procedural macros (`#![forbid(unsafe_code)]`, `edition = "2024"`, `MSRV 1.87`) for `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

Provides three derive macros usable in both Bevy host applications (hygienically resolving `bamabi_sys`, `bamabi`, `bamabi_host`, or `bamabi_guest` via `proc-macro-crate`) and decoupled guest `.cdylib` crates across named structs, tuple structs, `FfiOption<T>`, and C-ABI `#[repr(i8..u64)]` enums:

- **`#[derive(ModComponent)]`**:
  - Generates a `#[repr(C)]` `<Struct>VTable` prefixed at offset `0` by [`VTableHeader`](../bamabi_sys/src/component.rs) and populated with `extern "C"` getter, error-returning `FfiResult` setter, interior-pointer, 11-slot `HostVec`, and schema-evolution witness (`field_info`, `export_fields`, `import_field`) slots.
  - Emits deterministic 64-bit FNV-1a `SCHEMA_FINGERPRINT`, static `BAM_FIELDS: &[BamFieldDesc]`, `REQUIRED_COMPONENTS` (including `#[mod_component(require(Comp = expr))]` custom initializers), `"bam.v0.<canonical_name>.*"` symbol metadata, optional C-linkage exports (`#[mod_component(export_symbols)]`), and compile-time `C_DECL` / `ZIG_DECL` layout constants.
  - Computes a deterministic 128-bit keyed SipHash-2-4 [`ComponentUuid`](../bamabi_sys/src/primitives/uuid.rs) and conditionally implements `RawInsertable`, `AbiPod`, and (with `#[mod_component(..., guest)]`) `bamabi_guest::GuestComponent`.
  - Supports explicit `#[mod_component(...)]` (`name`, `namespace`, `uuid`, `prefix`, `crate`, `guest`, `guest_crate`, `export_symbols`, `no_pod`, `opaque`, `sparse_set`, `immutable`, `summary_tick`, `require(...)`) and `#[mod_field(...)]` (`skip`, `bool`, `primitive`, `string`, `vec(T)`, `option(T)`, `nested`) overrides.
- **`#[derive(AbiPod)]`**:
  - Validates standalone `#[repr(C)]` / `#[repr(transparent)]` (non-packed) Plain Old Data types (`Copy + Send + Sync + 'static`, zero drop glue, compile-time zero implicit padding `size_of::<Self>() == sum(size_of::<FieldTy>())`, and fixed-width C-compatible fields) and emits `unsafe impl AbiPod` + `unsafe impl RawInsertable`.
- **`#[derive(GuestComponent)]`**:
  - Lightweight zero-boilerplate derive for guest `AbiPod` structs (`#[guest_component(name = "...", namespace = "...", uuid = "...", sparse_set, immutable, summary_tick, no_schema_hash)]`, falling back to compatible `#[mod_component(...)]` keys), emitting **only** `impl bamabi_guest::GuestComponent` with bit-for-bit `FIELD_SCHEMA_HASH` parity and zero VTable/C-decl overhead when paired with `#[derive(AbiPod, GuestComponent)]`.

## Documentation Map

- **Crate Architecture & Internals:** [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Test & Verification Catalog:** [`TESTS.md`](TESTS.md)
- **Workspace Docs:** [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md), [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md), [`../../TESTING.md`](../../TESTING.md)
- **Wire Specification:** [`../../spec.md`](../../spec.md)

## Setup & Building

Add `bamabi_macros` directly (typically for host components or `ModComponent` sub-structs):

```toml
[dependencies]
bamabi_macros = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main" }
```

*(Guest `.cdylib` mods typically enable `bamabi_guest = { ..., features = ["derive"] }` rather than importing `bamabi_macros` directly.)*

Build locally from the workspace root:

```bash
cargo build -p bamabi_macros --all-features
```

To inspect generated VTables, assertions, and declarations on a target:

```bash
cargo expand -p bamabi_macros --test mod_component_vtable
```

## Testing & Verification

```bash
cargo fmt -p bamabi_macros -- --check
cargo clippy -p bamabi_macros --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_macros --no-deps --all-features
cargo test -p bamabi_macros --all-features
cargo test --doc -p bamabi_macros --all-features
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test mod_component_vtable
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p bamabi_macros --test proptest_witness
```

## Benchmarks & Profiling

`bamabi_macros` and the rest of Layer 0 (`bamabi-wire`) contain **no Criterion runtime benchmarks**; runtime VTable dispatch and ECS overhead benchmarks live in the downstream `bamabi-showcase` repository. For compile-time macro expansion profiling, run `cargo build -p bamabi_macros --timings` or `RUSTFLAGS="-Z self-profile"` on nightly.

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
