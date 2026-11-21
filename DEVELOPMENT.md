# Developer Onboarding & Workflow Guide

This guide covers setting up your local environment, building crates and guest `.cdylib` mods, running `cargo xtask` codegen/C99 tools, and verifying changes in `bamabi-wire`.

## 1. Setting Up

### Required Toolchains

1. **Rust Toolchain (`rust-toolchain.toml`):**
   - [`rust-toolchain.toml`](rust-toolchain.toml) pins `channel = "stable"` with components `rustfmt` and `clippy`.
   - Workspace edition is `2024` (`Cargo.toml`).
   - **MSRV policy:**
     - `1.87` for all library crates (`bamabi_sys`, `bamabi_macros`, `bamabi_guest_sys`, `bamabi_guest`).
     - `1.95` for [`crates/bamabi_sdk_gen`](crates/bamabi_sdk_gen/Cargo.toml) and [`xtask`](xtask/Cargo.toml) (via `[workspace.package].rust-version = "1.95"` in [`Cargo.toml`](Cargo.toml)).
   - Verify your active toolchain:
     ```bash
     rustc --version
     cargo fmt --version
     cargo clippy --version
     ```
2. **ISO C99 Compiler & Sanitizers (`cc` / `clang`):**
   - Required by `cargo xtask c-test` to syntax-check `include/bamabi.h` and `include/generated/testbed_components.h` and to compile and run [`tests/standalone_c_test.c`](tests/standalone_c_test.c) under strict `-std=c99 -Wall -Wextra -Werror -pedantic` plus `-fsanitize=address,undefined`.
   - Override compilers if needed via `CC` (defaults to `cc`) and `CLANG` (defaults to `/usr/bin/clang` on macOS when present to avoid `swiftly` shim stalls, or `clang` elsewhere).

### Optional Toolchains (Extended Verification & Polyglot Tests)

Install only the tools needed for the checks you plan to run locally:

```bash
# Cross-target wire checks (`cargo xtask audit`)
rustup target add i686-unknown-linux-gnu wasm32-unknown-unknown

# Nightly + Miri + rust-src (`cargo xtask miri`, `cargo xtask nightly`)
rustup toolchain install nightly --component miri rust-src
cargo +nightly miri setup

# Static audit tools (`cargo xtask audit`)
cargo install typos-cli
cargo install cargo-deny
cargo install cargo-semver-checks

# Kani formal verification (`cargo xtask kani`; CI pins 0.68.0)
cargo install --locked kani-verifier --version 0.68.0
cargo kani setup
```

- **Optional polyglot compilers (`sdk_compile_tests`):** `crates/bamabi_sdk_gen/tests/sdk_compile_tests.rs` automatically runs C++17 (`c++ -std=c++17`) and Zig (`zig`) compilation checks when `c++` or `zig` are on `PATH` (skipping gracefully if absent).

### Multi-Repo Local Development (`bamabi-dev-overlay`)

`bamabi-wire` builds and tests completely standalone. When iterating across sibling repositories (`bamabi-tools`, `bamabi-core`, `bamabi`, `bamabi-showcase`) under a shared parent directory, use [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay) instead of editing tracked `Cargo.toml` files:

```bash
# Check abi-lock.toml, versions, licenses, and GitHub dependency hygiene across repos
python3 ../bamabi-dev-overlay/sync_check.py --strict-git

# Enable local [patch] overrides to sibling checkouts
python3 ../bamabi-dev-overlay/overlay.py enable
python3 ../bamabi-dev-overlay/overlay.py status --strict

# Disable before committing or running standalone CI checks
python3 ../bamabi-dev-overlay/overlay.py disable
```

> **Note (`Cargo.lock` hygiene):** Never commit `Cargo.lock` edits produced while a local `[patch]` overlay is enabled.

## 2. Building

### Workspace & Feature Matrix

```bash
# Default workspace build
cargo build --workspace

# Build without optional `derive` feature on bamabi_guest
cargo build --workspace --no-default-features

# Build with all features enabled
cargo build --workspace --all-features
```

### Building a Standalone Guest `.cdylib` Mod

Guest mods depend on `bamabi_guest` (zero Bevy dependencies) and compile as a `cdylib`:

```toml
[package]
name = "my_guest_mod"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
bamabi_guest = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main", features = ["derive"] }
```

```bash
cargo build --release
```

### Cross-Target Wire Checks

`bamabi_sys` is `#![no_std]` with zero dependencies and validates 32-bit and WebAssembly wire layouts:

```bash
cargo check -p bamabi_sys --target i686-unknown-linux-gnu
cargo check -p bamabi_sys --target wasm32-unknown-unknown
```

### Generating C99, C++17 & Zig Headers/SDKs

- **C99 ABI header (`include/bamabi.h`):**
  ```bash
  cargo xtask header
  ```
- **Generated component header (`include/generated/testbed_components.h`):**
  ```bash
  cargo xtask gen-component-headers
  ```
- **Full polyglot bundle (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`):** Call `bamabi_sdk_gen::export_sdk(output_dir)` from a Rust tool (or `cargo bam export-sdk` in `bamabi-tools`).

## 3. Running

### Browsing Workspace & Crate Docs (`cargo docs --open`)

[`.cargo/config.toml`](.cargo/config.toml) defines `docs = "doc --workspace --no-deps"`. Both `cargo docs --open` and `cargo doc --workspace --no-deps --open` land directly on `target/doc/bamabi_docs/index.html` ([`bamabi_docs`](crates/bamabi_docs/README.md)), which embeds all workspace guides (`overview`, `architecture`, `development`, `testing`, `wire_spec`) and links to every crate's incremental API tutorials and `ARCHITECTURE.md` / `TESTS.md` modules in `rustdoc`:

```bash
# Primary local docs browser (lands on target/doc/bamabi_docs/index.html)
cargo docs --open

# Equivalent unaliased command (add --all-features to include derive examples)
cargo doc --workspace --no-deps --open
cargo doc --workspace --no-deps --all-features --open
```

### Workspace Task Runner (`cargo xtask`)

[`.cargo/config.toml`](.cargo/config.toml) defines `xtask = "run --package xtask --"` alongside `docs = "doc --workspace --no-deps"`. Run from the repository root:

```bash
cargo xtask --help                    # Overview of all 11 subcommands
cargo xtask <subcommand> --help       # Detailed options for one subcommand
cargo xtask ci                        # Default local CI gate
```

### Regenerating Checked-In Headers

After modifying wire types in `crates/bamabi_sys` or testbed fixtures in [`xtask/src/component_headers.rs`](xtask/src/component_headers.rs), regenerate and verify parity:

```bash
cargo xtask header
cargo xtask gen-component-headers
cargo xtask header --check
cargo xtask gen-component-headers --check
```

### Compiling & Running `tests/standalone_c_test.c`

Run through `xtask` (recommended — runs strict C99 syntax-only checks on both headers, builds & executes `tests/standalone_c_test.c`, and runs an ASan + UBSan build):

```bash
cargo xtask c-test
cargo xtask c-test --no-asan
cargo xtask c-test --asan-only
```

Or invoke your C compiler directly:

```bash
cc -std=c99 -Wall -Wextra -Werror -pedantic -Iinclude tests/standalone_c_test.c -o target/standalone_c_test
./target/standalone_c_test
```

## 4. Testing & Benchmarking

For the complete multi-tier testing guide (unit/integration tests, `trybuild`, `proptest`, Miri, Kani `PO-W1..PO-W8` & `PO-G1..PO-G9`, `audit`, nightly `-Zrandomize-layout`, and benchmarking/profiling notes), see [`TESTING.md`](TESTING.md).

Quick verification before opening a PR:

```bash
cargo xtask ci
```
