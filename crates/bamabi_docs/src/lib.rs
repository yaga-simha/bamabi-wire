#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! # `bamabi-wire` Workspace Documentation Hub (`ABI 0.3.0`)
//!
//! Welcome to **`bamabi_docs`**, the non-published (`publish = false`, `#![no_std]`,
//! `#![forbid(unsafe_code)]`, zero-dependency) documentation hub for **`bamabi-wire`** (**Layer 0**
//! of the federated **`bamabi`** native modding framework for Bevy ECS `v0.20.1`).
//!
//! Running `cargo docs --open` (aliased in `.cargo/config.toml` to `cargo doc --workspace --no-deps`)
//! or `cargo doc --open` at the workspace root lands directly here (`target/doc/bamabi_docs/index.html`).
//!
//! ---
//!
//! ## Workspace Guides & Crate Index
//!
//! - **Workspace Guides (rendered in `rustdoc`):**
//!   - [`overview`] — Workspace overview, quickstart, and federated repo map (`README.md`)
//!   - [`architecture`] — Layer 0 crate DAG, `ABI 0.3.0` lifecycle, allocator/panic boundaries, and
//!     6-crate walkthrough (`ARCHITECTURE.md`)
//!   - [`development`] — Toolchains, `.cdylib` builds, `cargo xtask`, and `bamabi-dev-overlay`
//!     (`DEVELOPMENT.md`)
//!   - [`testing`] — 8-tier verification pyramid, Miri, Kani `PO-W1..PO-W8` / `PO-G1..PO-G9`, and
//!     Layer 0 stress/telemetry notes (`TESTING.md`)
//!   - [`benchmarks`] — Layer 0 testing & polyglot host-guest benchmarking strategy and recorded
//!     results (`BENCHMARKS.md`)
//!   - [`profiling`] — Layered `hotpath-rs` + disassembly profiling & optimization record
//!     (`PROFILING.md`)
//!   - [`wire_spec`] — Canonical byte-level wire specification (`spec.md`)
//!   - [`crates`] — Curated index of all 6 functional crates with direct links to every crate root,
//!     architecture guide, and test catalog
//! - **Workspace Crates:**
//!   [`bamabi_sys`](../bamabi_sys/index.html) ·
//!   [`bamabi_macros`](../bamabi_macros/index.html) ·
//!   [`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html) ·
//!   [`bamabi_guest_sys`](../bamabi_guest_sys/index.html) ·
//!   [`bamabi_guest`](../bamabi_guest/index.html) ·
//!   [`xtask`](../xtask/index.html)
//!
//! ---
//!
//! ## 1. Federated 5-Repository Position
//!
//! `bamabi` is organized as a strictly acyclic 5-repository DAG plus `bamabi-dev-overlay`.
//! `bamabi-wire` sits at **Layer 0** with **zero `bevy_*` dependencies** and **zero cross-repo
//! dependencies**, so hosts, CLI tools, compiler backends (`bevy_bql`), and Rust/C99/C++17/Zig
//! guest `.cdylib` mods all compile against one identical `ABI 0.3.0` (`0x0003_0000`) binary
//! contract:
//!
//! | Layer | Repository | Role |
//! |---|---|---|
//! | **0** | **`bamabi-wire`** *(this repo)* | Zero-Bevy ABI `0.3.0` wire contract, derives, C99/C++17/Zig SDK generator, and Rust guest SDK |
//! | **1** | `bamabi-tools` | Zero-Bevy Ed25519 trust (`bamabi_trust`), `mod.toml` SemVer DAG (`bamabi_manifest`), and `cargo-bam` CLI |
//! | **2** | `bamabi-core` | Headless `bevy_ecs` + `bevy_app` host runtime (`bamabi_host_sys`, `bamabi_host`, `bamabi_watcher`) |
//! | **3** | `bamabi` | Canonical host facade (`bamabi`) and Bevy `0.20.1` companion bridges/render/reflect crates |
//! | **4** | `bamabi-showcase` | Integration suite: runnable examples (`01`–`10`), Criterion benchmarks (`docs/BENCHMARKS.md`), fuzzers, mdBook guide |
//! | **Overlay** | `bamabi-dev-overlay` | Multi-repo `abi-lock.toml` verifier (`sync_check.py`) and local sibling `[patch]` manager (`overlay.py`) |
//!
//! ---
//!
//! ## 2. The 6-Crate DAG Inside `bamabi-wire`
//!
//! Inside this workspace, 6 functional crates (plus this non-published `bamabi_docs` hub) form an
//! acyclic dependency graph that isolates `unsafe` FFI machinery (`bamabi_sys`, `bamabi_guest_sys`)
//! from `#![forbid(unsafe_code)]` crates (`bamabi_macros`, `bamabi_sdk_gen`, `bamabi_guest`,
//! `xtask`, `bamabi_docs`):
//!
//! ```text
//!                 ┌──────────────────────────┐
//!                 │       bamabi_guest       │  #![forbid(unsafe_code)]
//!                 │  Safe Rust .cdylib SDK   │
//!                 └─────┬───────┬────────┬───┘
//!                       │       │        │ (optional feature: `derive`)
//!                       ▼       │        ▼
//!   ┌───────────────────────┐   │   ┌────────────────────┐
//!   │   bamabi_guest_sys    │   │   │   bamabi_macros    │  #![forbid(unsafe_code)]
//!   │  Guest unsafe boundary│   │   │  proc-macro2/syn   │
//!   └───────────┬───────────┘   │   └─────────┬──────────┘
//!               │               │             │
//!               │               ▼             │
//!               │   ┌────────────────────┐    │     ┌────────────────────┐
//!               └──►│     bamabi_sys     │◄───┴─────│   bamabi_sdk_gen   │  #![forbid(unsafe_code)]
//!                   │ #![no_std], 0 deps │          │ C99 / C++17 / Zig  │
//!                   └────────────────────┘          └─────────▲──────────┘
//!                             ▲                               │
//!                             └─────────────┬─────────────────┘
//!                                     ┌─────┴──────┐
//!                                     │   xtask    │  #![forbid(unsafe_code)]
//!                                     │  CI/codegen│
//!                                     └────────────┘
//! ```
//!
//! 1. **[`bamabi_sys`](../bamabi_sys/index.html)** (`#![no_std]`, zero dependencies): Single source
//!    of truth for `ABI 0.3.0` (`0x0003_0000`). Defines `#[repr(C)]` / `#[repr(transparent)]`
//!    primitives (`FfiResult`, `FfiBool`, `FfiEntity`, `StrView`, `FfiSlice`, `FfiOption`), const
//!    SipHash-2-4-128 `ComponentUuid`, prefix-extensible descriptors (`ExtensibleStruct`,
//!    `BamManifest`, `VTableHeader`, `BamQueryDesc`, `BamSystemDesc`, `BamObserverDesc`,
//!    `BamRelationDesc`, `BamScheduleSetDesc`), the **73-slot** `BamHostApi` table (`MIN_V0_3_SIZE`
//!    covering the first 68 slots; full `592` B / `300` B covering `68..=72`) plus byte-offset
//!    constants (`offsets::OFF64_*` / `OFF32_*`), and `AbiPod` / `ModComponent` / `RawInsertable`
//!    traits.
//! 2. **[`bamabi_macros`](../bamabi_macros/index.html)** (`#![forbid(unsafe_code)]`): Proc-macro
//!    compiler plugin generating `#[derive(AbiPod)]` (compile-time zero-padding and field checks),
//!    `#[derive(GuestComponent)]`, and `#[derive(ModComponent)]`, emitting `ComponentUuid`, 64-bit
//!    FNV-1a `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, hygienic `"bam.v0.*"`
//!    VTables, witness adapters (`export_fields` / `import_field`), and `C_DECL` / `ZIG_DECL`
//!    constants.
//! 3. **[`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html)** (`#![forbid(unsafe_code)]`): Reads
//!    `bamabi_sys` constants and `offsets` to render the checked-in ISO C99 header
//!    (`include/bamabi.h`), the polyglot SDK bundle (`bamabi.h`, C++17 `bamabi.hpp` with 73/73
//!    slot views and RAII `ScopedTransaction`, and Zig `bamabi.zig` with `addTranslateC` views and
//!    lossless `BamError`), and `ModComponent::C_DECL` component headers
//!    (`include/generated/testbed_components.h`).
//! 4. **[`bamabi_guest_sys`](../bamabi_guest_sys/index.html)** (guest `unsafe` boundary): Wraps
//!    `*const BamHostApi` (`Host` + `host_slot!` bounds checking across all 73 slots), per-World
//!    heap-owned `GuestInstance` with LIFO teardown, `catch_unwind` `extern "C"` panic barriers
//!    (`panic`), lock-free `ReentrantCell`, validated slices/iterators (`ChunkColumnDispenser`,
//!    `SparseRows`), checked `ValidatedCommands`, schema migrations, and unconditional Bevy `0.20`
//!    POD `wire_types`.
//! 5. **[`bamabi_guest`](../bamabi_guest/index.html)** (`#![forbid(unsafe_code)]`): Safe,
//!    idiomatic Rust `.cdylib` guest SDK built on `bamabi_guest_sys` (and optionally
//!    `bamabi_macros` via `feature = "derive"`): `export_mod!`, `GuestPlugin`, `GuestApp`,
//!    `SystemBuilder`, `1..=16`-term sealed `QueryTuple` + `Or<(T0..T7)>`, `GuestCommands` + RAII
//!    `GuestTransaction`, `GuestEntityCommands`, `BevySchedules`, `conditions`,
//!    `TypedObserverBuilder`, and unconditional Bevy `0.20` `bridges`.
//! 6. **[`xtask`](../xtask/index.html)** (`#![forbid(unsafe_code)]`): Orchestrates formatting,
//!    `#![forbid(unsafe_code)]` and zero-Bevy audits, `clippy`, `RUSTDOCFLAGS="-D warnings"` doc
//!    builds, workspace tests, `bamabi.h` and `testbed_components.h` byte-parity checks, strict
//!    C99 + ASan/UBSan checks, cross-target `i686`/`wasm32` checks, `typos` / `cargo-deny` /
//!    `cargo-semver-checks`, dual-model Miri, Kani formal proofs, and nightly `-Zrandomize-layout`.
//!
//! ---
//!
//! ## 3. How Everything Ties Together: End-to-End Flow
//!
//! One concrete trace across components, systems, commands, polyglot headers, and CI shows how
//! all 6 functional crates interlock with zero `bevy_*` dependencies:
//!
//! 1. **Contract (`bamabi_sys`):** `abi-lock.toml` pins `ABI 0.3.0` (`0x0003_0000`). `bamabi_sys`
//!    freezes the `73`-slot `BamHostApi` (`MIN_V0_2_SIZE = 224` B / `116` B; `MIN_V0_3_SIZE = 552` B
//!    / `280` B for the first 68 slots; full `592` B / `300` B), computes `OFF64_*` / `OFF32_*`
//!    offsets via `offset_of!`, defines `FfiResult` (18 status codes), and provides `const`
//!    SipHash-2-4-128 `ComponentUuid::from_parts` plus `AbiPod` / `ModComponent` / `RawInsertable`.
//! 2. **Compile-Time Derivation (`bamabi_macros`):** A component annotated with
//!    `#[derive(ModComponent, GuestComponent)]` (or `#[derive(AbiPod, GuestComponent)]`) is
//!    verified for `#[repr(C)]`, zero implicit padding, and POD field bounds at compile time,
//!    emitting `UUID`, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `"bam.v0.*"`
//!    VTables, and `C_DECL` / `ZIG_DECL` layout declarations.
//! 3. **Polyglot Header & SDK Synthesis (`bamabi_sdk_gen`):** `header_gen::render_c99_header`
//!    reads `bamabi_sys::offsets` to emit `include/bamabi.h`; `sdk::export_sdk` packages
//!    `bamabi.h`, `bamabi.hpp`, and `bamabi.zig`; and `component_header::render_component_header`
//!    emits C99 component headers with `BAM_STATIC_ASSERT` checks.
//! 4. **Host-Guest Handshake & Tick Execution (`bamabi_guest_sys` -> `bamabi_guest`):**
//!    - On load, `export_mod!` delegates `bamabi_init` into `bamabi_guest_sys`, which copies
//!      `BamHostApi` via bounded `read_descriptor`, validates ABI compatibility and mandatory slots,
//!      allocates an isolated per-World `GuestInstance`, and runs `GuestPlugin::build(&mut app)`.
//!    - On every frame tick, host `BamSystemFn` calls cross `extern "C"` into
//!      `catch_guest_result` (`catch_unwind`) + `ReentrantCell`, exposing `ChunkColumnDispenser`-
//!      checked query slices and `GuestTransaction`-capable `GuestCommands` to safe `bamabi_guest`
//!      closures without ever unwinding across `extern "C"`.
//!    - On unload, `bamabi_shutdown` drops all guest closures and `GuestInstance` allocations in
//!      deterministic LIFO order on the guest allocator.
//! 5. **Continuous Verification (`xtask`):** `cargo xtask ci` runs `fmt`, `header --check`,
//!    `gen-component-headers --check`, `lint` (`#![forbid(unsafe_code)]`, zero-Bevy `cargo tree`,
//!    `clippy -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc`), workspace + doc tests, and
//!    strict C99 + ASan/UBSan `tests/standalone_c_test.c`.
//!
//! ---
//!
//! ## 4. Setup, Build, Run, Test & Benchmark Quickstart
//!
//! All commands run standalone inside `bamabi-wire` with zero sibling repositories required:
//!
//! ```bash
//! # Build all crates (default / --no-default-features / --all-features)
//! cargo build --workspace --all-features
//!
//! # Build and open this workspace documentation hub + all crate docs locally
//! cargo docs --open
//! # Equivalent full command (add --all-features to render derive examples):
//! cargo doc --workspace --no-deps --all-features --open
//!
//! # Default local CI gate (fmt, header parity, lint, workspace + doc tests, C99 + ASan/UBSan)
//! cargo xtask ci
//!
//! # Extended gate (adds i686/wasm32 + typos + cargo-deny audit, dual-model Miri, and Kani proofs)
//! cargo xtask ci --full
//!
//! # Individual xtask & crate checks
//! cargo xtask header --check
//! cargo xtask gen-component-headers --check
//! cargo xtask test --proptest --cases 256
//! cargo xtask miri
//! cargo xtask kani
//! cargo test -p bamabi_docs --doc
//! ```
//!
//! - **Benchmarking note:** Layer 0 includes a zero-dependency polyglot host-guest benchmark
//!   harness (`benches/bamabi_bench` in `--release` mode + Rust/C99/C++17/Zig guests in `release`
//!   and `debug` profiles, orchestrated via `cargo xtask bench`). See [`benchmarks`] for the
//!   testing/benchmarking strategy and latest recorded results, and [`testing`] for command-queue
//!   stress tests, `"BAMT"` `SystemTelemetryDigest64` wire framing tests, `PROPTEST_CASES` scaling,
//!   and `cargo check --workspace --all-features --timings`. End-to-end Bevy-integrated ECS and
//!   hot-reload Criterion benchmarks live downstream in Layer 4 (`bamabi-showcase`, `docs/BENCHMARKS.md`).

#[cfg(doc)]
pub mod overview {
    #![doc = include_str!("../../../README.md")]
}

#[cfg(doc)]
pub mod architecture {
    #![doc = include_str!("../../../ARCHITECTURE.md")]
}

#[cfg(doc)]
pub mod development {
    #![doc = include_str!("../../../DEVELOPMENT.md")]
}

#[cfg(doc)]
pub mod testing {
    #![doc = include_str!("../../../TESTING.md")]
}

#[cfg(doc)]
pub mod benchmarks {
    #![doc = include_str!("../../../BENCHMARKS.md")]
}

#[cfg(doc)]
pub mod profiling {
    #![doc = include_str!("../../../PROFILING.md")]
}

#[cfg(doc)]
pub mod wire_spec {
    #![doc = include_str!("../../../spec.md")]
}

#[cfg(doc)]
pub mod crates {
    //! Curated index of every workspace crate in `bamabi-wire`.
    //!
    //! Each crate below provides incremental crate-root API tutorials alongside its architecture
    //! guide and test/proof catalog inside `target/doc/`:
    //!
    //! | Crate | Safety & Policy | Root Docs | Architecture Guide | Test & Proof Catalog |
    //! |---|---|---|---|---|
    //! | **`bamabi_sys`** | `#![no_std]`, zero deps, `unsafe` FFI contract | [`bamabi_sys`](../../bamabi_sys/index.html) | [`bamabi_sys::docs::architecture`](../../bamabi_sys/docs/architecture/index.html) | [`bamabi_sys::docs::tests`](../../bamabi_sys/docs/tests/index.html) |
    //! | **`bamabi_macros`** | `#![forbid(unsafe_code)]`, proc-macro derives | [`bamabi_macros`](../../bamabi_macros/index.html) *(includes Architecture & Tests inline)* | [`bamabi_macros`](../../bamabi_macros/index.html) | [`bamabi_macros`](../../bamabi_macros/index.html) |
    //! | **`bamabi_sdk_gen`** | `#![forbid(unsafe_code)]`, C99/C++17/Zig generator | [`bamabi_sdk_gen`](../../bamabi_sdk_gen/index.html) | [`bamabi_sdk_gen::docs::architecture`](../../bamabi_sdk_gen/docs/architecture/index.html) | [`bamabi_sdk_gen::docs::tests`](../../bamabi_sdk_gen/docs/tests/index.html) |
    //! | **`bamabi_guest_sys`** | Guest `unsafe` FFI & panic boundary | [`bamabi_guest_sys`](../../bamabi_guest_sys/index.html) | [`bamabi_guest_sys::docs::architecture`](../../bamabi_guest_sys/docs/architecture/index.html) | [`bamabi_guest_sys::docs::tests`](../../bamabi_guest_sys/docs/tests/index.html) |
    //! | **`bamabi_guest`** | `#![forbid(unsafe_code)]`, safe `.cdylib` guest SDK | [`bamabi_guest`](../../bamabi_guest/index.html) | [`bamabi_guest::docs::architecture`](../../bamabi_guest/docs/architecture/index.html) | [`bamabi_guest::docs::tests`](../../bamabi_guest/docs/tests/index.html) |
    //! | **`xtask`** | `#![forbid(unsafe_code)]`, workspace CI & QA runner | [`xtask`](../../xtask/index.html) | [`xtask::docs::architecture`](../../xtask/docs/architecture/index.html) | [`xtask::docs::tests`](../../xtask/docs/tests/index.html) |
    //!
    //! ## Workspace Guides in `bamabi_docs`
    //!
    //! - [`overview`](../overview/index.html) — Workspace `README.md`
    //! - [`architecture`](../architecture/index.html) — Workspace `ARCHITECTURE.md`
    //! - [`development`](../development/index.html) — Workspace `DEVELOPMENT.md`
    //! - [`testing`](../testing/index.html) — Workspace `TESTING.md`
    //! - [`benchmarks`](../benchmarks/index.html) — Workspace `BENCHMARKS.md`
    //! - [`profiling`](../profiling/index.html) — Workspace `PROFILING.md`
    //! - [`wire_spec`](../wire_spec/index.html) — Canonical `spec.md`
}
