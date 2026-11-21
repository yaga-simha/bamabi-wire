# `bamabi-wire`

Layer 0 (**zero-Bevy**, zero cross-repo dependencies) wire contract, procedural macros, polyglot SDK header generator, and Rust guest SDK for the federated **`bamabi`** (`ABI 0.3.0`, `v0.3.0`) native modding framework for Bevy ECS (`v0.20.1`).

Refactored from the original [`bevy_mod_abi`](https://github.com/yaga-simha/bevy_mod_abi) monorepo into a strictly acyclic 5-repository DAG plus [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay), `bamabi-wire` is the root dependency layer: no crate in this repository depends on `bevy_*` or any sibling repository, allowing host runtimes, CLI tooling, compiler backends (`bevy_bql`), and Rust/C99/C++17/Zig guest `.cdylib` mods to share one identical C-ABI binary contract.

## Documentation & Onboarding

The primary way to browse the full workspace overview, end-to-end architecture walkthrough, per-crate incremental API guides, and test catalogs locally is `rustdoc` via the `.cargo/config.toml` alias:

```bash
# Build and open all workspace, crate, architecture, and test docs locally
cargo docs --open
# Equivalent full command:
cargo doc --workspace --no-deps --open
```

- **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — Layer 0 crate DAG, zero-Bevy / zero-shared-Rust-ABI invariants, `ABI 0.3.0` (`0x0003_0000`) host-guest lifecycle, allocator & panic boundaries, polyglot SDK pipeline, end-to-end 6-crate walkthrough, and per-crate onboarding index.
- **[`DEVELOPMENT.md`](DEVELOPMENT.md)** — Setting up Rust (`1.87`/`1.95`), C99/ASan/UBSan, optional Miri/Kani/`audit` tools, browsing `cargo docs --open`, building workspace & guest `.cdylib` mods, generating C99/C++17/Zig headers, and `bamabi-dev-overlay`.
- **[`TESTING.md`](TESTING.md)** — Multi-tier verification pyramid (unit/integration, doctests, `trybuild`, `proptest`, header parity, C99 ASan/UBSan, Miri, Kani `PO-W1..PO-W8` & `PO-G1..PO-G9`, `audit`, nightly `-Zrandomize-layout`) and Layer 0 benchmarking/stress notes.
- **[`spec.md`](spec.md)** — Canonical byte-level wire specification, struct layouts, offsets, and error codes.

## Workspace Crates

| Crate | Safety Policy | External Deps | MSRV / Edition | Docs | Responsibilities |
|---|---|---|---|---|---|
| [`bamabi_sys`](crates/bamabi_sys/README.md) | `#![no_std]` (`unsafe` FFI contract) | Zero | `1.87` / `2024` | [Arch](crates/bamabi_sys/ARCHITECTURE.md) · [Tests](crates/bamabi_sys/TESTS.md) | `ABI 0.3.0` (`0x0003_0000`) wire types, 68-slot `BamHostApi` (552 B 64-bit / 280 B 32-bit), `offsets` (`OFF64_*`/`OFF32_*`), `ComponentUuid` (SipHash-2-4-128), `FfiResult`, descriptors, `ModComponent`, `AbiPod`, `RawInsertable` |
| [`bamabi_macros`](crates/bamabi_macros/README.md) | `#![forbid(unsafe_code)]` | `syn`, `quote`, `proc-macro2`, `proc-macro-crate` | `1.87` / `2024` | [Arch](crates/bamabi_macros/ARCHITECTURE.md) · [Tests](crates/bamabi_macros/TESTS.md) | `#[derive(ModComponent, AbiPod, GuestComponent)]`, zero-padding checks, hygienic VTables, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL`/`ZIG_DECL` |
| [`bamabi_sdk_gen`](crates/bamabi_sdk_gen/README.md) | `#![forbid(unsafe_code)]` | `bamabi_sys` | `1.95` / `2024` | [Arch](crates/bamabi_sdk_gen/ARCHITECTURE.md) · [Tests](crates/bamabi_sdk_gen/TESTS.md) | Generates `include/bamabi.h` (C99), `bamabi.hpp` (C++17 68/68 views + `ScopedTransaction`), `bamabi.zig` (Zig `addTranslateC` 68/68 views + lossless 18-code `BamError`), and C99 component headers with `BAM_EXPORT`, `BAM_RESTRICT`, and `BAM_HOST_API_OFFSET_*` |
| [`bamabi_guest_sys`](crates/bamabi_guest_sys/README.md) | Guest `unsafe` FFI boundary | `bamabi_sys` | `1.87` / `2024` | [Arch](crates/bamabi_guest_sys/ARCHITECTURE.md) · [Tests](crates/bamabi_guest_sys/TESTS.md) | `GuestInstance`, `extern "C"` panic guards (`ReentrantCell`), checked 68/68 `BamHostApi` wrappers, unconditional `wire_types` Bevy `0.20` POD projections, validated iterators, commands, and migration trampolines |
| [`bamabi_guest`](crates/bamabi_guest/README.md) | `#![forbid(unsafe_code)]` | `bamabi_sys`, `bamabi_guest_sys`, opt. `bamabi_macros` (`derive`) | `1.87` / `2024` | [Arch](crates/bamabi_guest/ARCHITECTURE.md) · [Tests](crates/bamabi_guest/TESTS.md) | Safe Rust `.cdylib` guest SDK (`GuestPlugin`, `GuestApp`, `BevySchedules`, `conditions::*`, `GuestEntityCommands`/`GuestChildSpawner`, `FfiDisabled`, 1..=16-term `QueryTuple`, `Or<(T0..T7)>`, `GuestTransaction`, `TypedObserverBuilder`, unconditional Bevy `0.20` subsystem bridges, `export_mod!`) |
| [`xtask`](xtask/README.md) | `#![forbid(unsafe_code)]` | `bamabi_sys`, `bamabi_macros`, `bamabi_sdk_gen` | `1.95` / `2024` | [Arch](xtask/ARCHITECTURE.md) · [Tests](xtask/TESTS.md) | Workspace CI, header generation/parity, C99 + ASan/UBSan checks, cross-target/deny/typos audits, Miri, Kani, and nightly `-Zrandomize-layout` runner |

## Wire Specification, Headers & Tests

- **Wire Specification:** [`spec.md`](spec.md) — byte-level struct layouts, field offsets, error codes, prefix extensibility rules, and ABI `0.3.0` contract.
- **Polyglot C99 Header:** [`include/bamabi.h`](include/bamabi.h) (and generated component headers under [`include/generated/testbed_components.h`](include/generated/testbed_components.h)). Byte-for-byte parity with `bamabi_sdk_gen::render_c99_header()` is enforced automatically by `crates/bamabi_sdk_gen/tests/header_parity.rs`.
- **Standalone C99 Contract Test:** [`tests/standalone_c_test.c`](tests/standalone_c_test.c) — compiles and validates `include/bamabi.h` layout assertions under strict ISO C99 (`-std=c99 -Wall -Wextra -Werror -pedantic`) and ASan+UBSan.
- **ABI Sync Lockfile:** [`abi-lock.toml`](abi-lock.toml) — pinned to `bam_abi_version = "0x0003_0000"`, `c_symbol_prefix = "bamabi"`, `workspace_version = "0.3.0"`, `bevy_version = "0.20.1"`, `rust_version = "1.95"`, `edition = "2024"`.

## Standalone Usage (Guest Mods & Wire Consumers)

Because `bamabi-wire` has zero cross-repo or Bevy dependencies, guest `.cdylib` mods and downstream tools can depend directly on the canonical Git repository without any sibling checkouts:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
bamabi_guest = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main", features = ["derive"] }
```

For lower-level or generator consumers (`bamabi_sys`, `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_guest_sys`), point directly to `git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main"`.

## Quickstart: Setup, Build, Run, Test & Benchmarks

All checks run standalone inside `bamabi-wire` with zero sibling repositories required via `cargo xtask` (see [`DEVELOPMENT.md`](DEVELOPMENT.md) and [`TESTING.md`](TESTING.md) for prerequisites and details):

```bash
# Build workspace (default / --no-default-features / --all-features)
cargo build --workspace --all-features

# Browse workspace + crate + architecture + test docs locally in rustdoc
cargo docs --open

# Default local CI gate: fmt, header parity checks, lint (forbid-unsafe, zero-Bevy, clippy, rustdoc), tests, C99 + ASan/UBSan
cargo xtask ci

# Full gate including cross-target/typos/deny audit, dual-model Miri, and Kani formal verification
cargo xtask ci --full

# Individual verification & codegen subcommands
cargo xtask fmt                       # Check formatting (--fix to format)
cargo xtask lint                      # #![forbid(unsafe_code)], zero-Bevy tree check, clippy & rustdoc
cargo xtask test                      # --no-default-features, --all-features, doctests (--cases N, --proptest, --header-parity)
cargo xtask c-test                    # Strict C99 header syntax + standalone_c_test (+ ASan/UBSan; --no-asan or --asan-only)
cargo xtask header                    # Regenerate include/bamabi.h (--check for byte-parity verification)
cargo xtask gen-component-headers     # Regenerate include/generated/testbed_components.h (--check for parity)
cargo xtask audit                     # i686/wasm32 checks, typos, cargo-deny (--baseline-rev <rev>, --skip-targets, --skip-typos)
cargo xtask miri                      # Tree Borrows + Stacked Borrows (--tree-borrows, --stacked-borrows, --s390x)
cargo xtask kani                      # Kani formal verification for bamabi_sys (PO-W1..PO-W8) & bamabi_guest_sys (PO-G1..PO-G9)
cargo xtask nightly                   # Big-endian s390x Miri + -Zrandomize-layout proptest (--s390x, --randomize-layout, --cases N)

# Inspect all subcommands and optional flags
cargo xtask --help
```

- **Benchmarking note:** As Layer 0, `bamabi-wire` contains no Criterion `[[bench]]` targets (runtime ECS/FFI benchmarks live downstream in `bamabi-showcase`, `docs/BENCHMARKS.md`). For Layer 0 command-queue stress tests, `"BAMT"` `SystemTelemetryDigest64` wire tests, and `cargo check --timings`, see [`TESTING.md#3-benchmarking-stress--telemetry-notes`](TESTING.md#3-benchmarking-stress--telemetry-notes).

## Federated `bamabi` Repositories

| Layer | Repository | Role |
|---|---|---|
| **0** | **[`bamabi-wire`](https://github.com/yaga-simha/bamabi-wire)** *(this repo)* | Zero-Bevy ABI wire contract, derives, polyglot headers, and Rust guest SDK |
| **1** | [`bamabi-tools`](https://github.com/yaga-simha/bamabi-tools) | Zero-Bevy Ed25519 trust (`bamabi_trust`), manifest DAG (`bamabi_manifest`), and `cargo-bam` CLI |
| **2** | [`bamabi-core`](https://github.com/yaga-simha/bamabi-core) | Headless `bevy_ecs` + `bevy_app` host runtime (`bamabi_host_sys`, `bamabi_host`, `bamabi_watcher`) and `docs/ARCHITECTURE.md` |
| **3** | [`bamabi`](https://github.com/yaga-simha/bamabi) | Canonical host library facade and Bevy `0.20.1` companion bridges/render/reflect crates |
| **4** | [`bamabi-showcase`](https://github.com/yaga-simha/bamabi-showcase) | Top-level integration repo: runnable examples (`01`–`10`), benchmarks (`docs/BENCHMARKS.md`), fuzzing, mdBook guide (`docs/book/`), and `xtask` |
| **Overlay** | [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay) | Multi-repo `abi-lock.toml` / standalone-git consistency verifier (`sync_check.py`) and local sibling `[patch]` overlay (`overlay.py`) |

## Multi-Repo Local Development (`bamabi-dev-overlay`)

When iterating on `bamabi-wire` alongside downstream sibling checkouts (`bamabi-tools`, `bamabi-core`, `bamabi`, `bamabi-showcase`) under a common parent directory, use [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay) instead of editing tracked `Cargo.toml` dependencies:

```bash
# Verify ABI lockfiles, versions, licenses, and strict GitHub dependency hygiene across all repos
python3 ../bamabi-dev-overlay/sync_check.py --strict-git

# Temporarily patch upstream github.com/yaga-simha/*.git deps to local sibling paths
python3 ../bamabi-dev-overlay/overlay.py enable
python3 ../bamabi-dev-overlay/overlay.py status --strict

# Disable the local overlay before committing or testing standalone builds (Cargo.lock hygiene)
python3 ../bamabi-dev-overlay/overlay.py disable
```

> **Note (`Cargo.lock` hygiene):** Never commit `Cargo.lock` changes generated while `[patch]` is enabled; downstream `Cargo.lock` files must retain `git+https://github.com/yaga-simha/...` sources for standalone CI.

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)).
