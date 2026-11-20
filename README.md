# `bamabi-wire`

Layer 0 (**zero-Bevy**, zero cross-repo dependencies) wire contract, procedural macros, polyglot SDK header generator, and Rust guest SDK for the federated **`bamabi`** (`ABI 0.3.0`, `v0.3.0`) native modding framework for Bevy ECS (`v0.20.1`).

Refactored from the original [`bevy_mod_abi`](https://github.com/yaga-simha/bevy_mod_abi) monorepo into a strictly acyclic 5-repository DAG plus [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay), `bamabi-wire` is the root dependency layer: no crate in this repository depends on `bevy_*` or any sibling repository, allowing host runtimes, CLI tooling, compiler backends (`bevy_bql`), and Rust/C99/C++17/Zig guest `.cdylib` mods to share one identical C-ABI binary contract.

## Workspace Crates

| Crate | Safety Policy | External Deps | MSRV / Edition | Responsibilities |
|---|---|---|---|---|
| [`bamabi_sys`](crates/bamabi_sys/README.md) | `#![no_std]` (`unsafe` FFI contract) | Zero | `1.87` / `2024` | `ABI 0.3.0` (`0x0003_0000`) wire types, 68-slot `BamHostApi` (552 B 64-bit / 280 B 32-bit), `offsets` (`OFF64_*`/`OFF32_*`), `ComponentUuid` (SipHash-2-4-128), `FfiResult`, descriptors, `ModComponent`, `AbiPod`, `RawInsertable` |
| [`bamabi_macros`](crates/bamabi_macros/README.md) | `#![forbid(unsafe_code)]` | `syn`, `quote`, `proc-macro2`, `proc-macro-crate` | `1.87` / `2024` | `#[derive(ModComponent, AbiPod, GuestComponent)]`, zero-padding checks, hygienic VTables, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL`/`ZIG_DECL` |
| [`bamabi_sdk_gen`](crates/bamabi_sdk_gen/README.md) | `#![forbid(unsafe_code)]` | `bamabi_sys` | `1.95` / `2024` | Generates `include/bamabi.h` (C99), `bamabi.hpp` (C++17 68/68 views + `ScopedTransaction`), `bamabi.zig` (Zig 0.16 68/68 views + lossless 18-code `BamError`), and C99 component headers with `BAM_EXPORT`, `BAM_RESTRICT`, and `BAM_HOST_API_OFFSET_*` |
| [`bamabi_guest_sys`](crates/bamabi_guest_sys/README.md) | Guest `unsafe` FFI boundary | `bamabi_sys` | `1.87` / `2024` | `GuestInstance`, `extern "C"` panic guards (`ReentrantCell`), checked 68/68 `BamHostApi` wrappers, unconditional `wire_types` Bevy `0.20` POD projections, validated iterators, commands, and migration trampolines |
| [`bamabi_guest`](crates/bamabi_guest/README.md) | `#![forbid(unsafe_code)]` | `bamabi_sys`, `bamabi_guest_sys`, opt. `bamabi_macros` (`derive`) | `1.87` / `2024` | Safe Rust `.cdylib` guest SDK (`GuestPlugin`, `GuestApp`, `BevySchedules`, `conditions::*`, `GuestEntityCommands`/`GuestChildSpawner`, `FfiDisabled`, 1..=16-term `QueryTuple`, `Or<(T0..T7)>`, `GuestTransaction`, `TypedObserverBuilder`, unconditional Bevy `0.20` subsystem bridges, `export_mod!`) |

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

## Local & CI Verification

All checks run standalone inside `bamabi-wire` with zero sibling repositories required:

```bash
# Formatting, clippy (default + all-features), rustdoc (default + all-features)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Unit, integration, header byte-parity, no-default-features, and doctests
cargo test --workspace --no-default-features
cargo test --workspace --all-features
cargo test --doc --all-features
cargo test -p bamabi_sdk_gen --test header_parity

# Verify strict zero-Bevy isolation (must print nothing)
cargo tree --workspace | sed 's/ (.*//' | grep -v bamabi | grep -i bevy

# 32-bit & Wasm wire layout checks, typos, cargo-deny, and SemVer compatibility
cargo check -p bamabi_sys --target i686-unknown-linux-gnu
cargo check -p bamabi_sys --target wasm32-unknown-unknown
typos
cargo deny --all-features check advisories licenses bans sources
cargo semver-checks check-release -p bamabi_sys --baseline-rev <tag-or-commit>

# Strict ISO C99 header syntax, layout conformance, and ASan + UBSan checks
cc -std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only -x c include/bamabi.h
cc -std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only -x c include/generated/testbed_components.h
cc -std=c99 -Wall -Wextra -Werror -pedantic -I include tests/standalone_c_test.c -o /tmp/standalone_c_test && /tmp/standalone_c_test
clang -std=c99 -Wall -Wextra -Werror -pedantic -O1 -g -fno-omit-frame-pointer \
  -fsanitize=address,undefined -fno-sanitize-recover=all \
  -I include tests/standalone_c_test.c -o /tmp/standalone_c_test_san && /tmp/standalone_c_test_san

# Nightly Miri & randomized-layout proptests (mirrored in ci.yml & nightly.yml)
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" \
  cargo +nightly miri test -p bamabi_sys -p bamabi_guest_sys -p bamabi_guest
MIRIFLAGS="-Zmiri-tree-borrows" \
  cargo +nightly miri test -p bamabi_macros --test mod_component_vtable
MIRIFLAGS="-Zmiri-tree-borrows" \
  cargo +nightly miri test --target s390x-unknown-linux-gnu -p bamabi_sys -p bamabi_guest_sys -p bamabi_guest
PROPTEST_CASES=2048 RUSTFLAGS="-Zrandomize-layout" \
  cargo +nightly test -p bamabi_sys -p bamabi_guest_sys -p bamabi_guest

# Kani formal verification for `bamabi_sys` (PO-W1..PO-W8) & `bamabi_guest_sys` (PO-G1..PO-G7)
cargo kani -p bamabi_sys -p bamabi_guest_sys -Z unstable-options --harness-timeout 15m --output-format terse -j
```

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
