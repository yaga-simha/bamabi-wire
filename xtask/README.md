# `xtask`

Zero-external-dependency workspace task runner (`cargo xtask`) for [`bamabi-wire`](../README.md).

Enforces `#![forbid(unsafe_code)]`, edition `2024`, and MSRV `1.95` ([`xtask/Cargo.toml`](Cargo.toml)), invoking only the workspace crates `bamabi_sys`, `bamabi_macros`, and `bamabi_sdk_gen` alongside standard CLI toolchains (`cargo`, `cc`/`clang`, `c++`, `zig`, `typos`, `cargo-deny`, `cargo-semver-checks`, `miri`, `kani`).

## Setup

The workspace configures the `cargo xtask` and `cargo docs` aliases in [`.cargo/config.toml`](../.cargo/config.toml):

```toml
[alias]
docs = "doc --workspace --no-deps"
xtask = "run --package xtask --"
```

Prerequisites for the default `cargo xtask ci` gate:

- Stable Rust toolchain with `rustfmt` and `clippy` (pinned in [`rust-toolchain.toml`](../rust-toolchain.toml))
- An ISO C99 compiler (`cc` or `$CC`) and `clang` (or `$CLANG`) with AddressSanitizer and UndefinedBehaviorSanitizer runtimes (`c-test`, `bench`)

Optional toolchains for extended subcommands (`bench`, `audit`, `miri`, `kani`, `nightly`):

- Polyglot benchmark guests (`bench`): ISO C++17 compiler (`c++` or `$CXX`) and Zig `0.16.0` (skipped automatically if absent unless `--strict-langs` or explicitly selected via `--langs`)
- Targets: `rustup target add i686-unknown-linux-gnu wasm32-unknown-unknown` (`audit`)
- Tools: `typos-cli`, `cargo-deny`, `cargo-semver-checks` (`audit`)
- Nightly + Miri + `rust-src`: `rustup toolchain install nightly --component miri rust-src` (`miri`, `nightly`)
- Kani `0.68.0`: `cargo install --locked kani-verifier && cargo kani setup` (`kani`)

See [`../DEVELOPMENT.md`](../DEVELOPMENT.md) for full toolchain setup instructions.

## Building

```bash
cargo build -p xtask
```

`cargo xtask ...` automatically compiles and runs the `xtask` binary on demand via the `.cargo/config.toml` alias.

## Usage

```bash
# Inspect all subcommands and per-command flags
cargo xtask --help
cargo xtask <SUBCOMMAND> --help

# Default local CI gate: fmt, header --check, gen-component-headers --check, lint, test, c-test, bench --quick
cargo xtask ci

# Skip benchmarks in local CI when iterating quickly
cargo xtask ci --no-bench

# Full gate (adds audit, dual-model Miri, and Kani formal proofs)
cargo xtask ci --full

# Individual subcommands
cargo xtask fmt                       # Check formatting (--fix to apply)
cargo xtask lint                      # #![forbid(unsafe_code)], zero-Bevy tree check, clippy, rustdoc
cargo xtask test                      # --no-default-features, --all-features, doctests
cargo xtask test --proptest --cases 64
cargo xtask c-test                    # C99 syntax-only + standalone_c_test (+ ASan/UBSan)
cargo xtask header                    # Regenerate include/bamabi.h (--check for parity)
cargo xtask gen-component-headers     # Regenerate include/generated/testbed_components.h (--check for parity)
cargo xtask audit                     # i686/wasm32 checks, typos, cargo-deny, optional semver-checks
cargo xtask miri                      # Tree Borrows + Stacked Borrows (or --s390x)
cargo xtask kani                      # Kani proofs for bamabi_sys & bamabi_guest_sys
cargo xtask nightly                   # Big-endian s390x Miri + -Zrandomize-layout extended proptests
cargo xtask bench                     # Build & run Layer-0 polyglot benchmarks (auto-updates BENCHMARKS.md on full 8-guest runs)
cargo xtask bench --quick --strict-langs --no-write-docs --json target/bench-results.json
cargo xtask bench --write-docs        # Force refreshing the recorded benchmark report in BENCHMARKS.md
cargo xtask bench --check-docs        # Validate BENCHMARKS.md markers and subject/workload table
```

## Testing

```bash
cargo fmt -p xtask -- --check
cargo clippy -p xtask --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p xtask --no-deps
cargo test -p xtask
```

`cargo test -p xtask` runs all 8 unit tests covering CLI argument parsing, flag conflict rejection, `BENCHMARKS.md` marker validation/splicing, and `include/generated/testbed_components.h` byte-for-byte parity with `ModComponent::C_DECL` (see [`TESTS.md`](TESTS.md)).

## Benchmarking

As part of Layer 0 (`bamabi-wire`), there are no Criterion `[[bench]]` targets; instead, `cargo xtask bench` orchestrates the zero-dependency polyglot host-guest benchmark suite ([`benches/bamabi_bench`](../benches/bamabi_bench/Cargo.toml)) across Rust, C99, C++17, and Zig shared-library guests (`release` and `debug` guest profiles) against a `--release` host and in-process `native` baseline (see [`../BENCHMARKS.md`](../BENCHMARKS.md) and [`../TESTING.md`](../TESTING.md)). Downstream Bevy-integrated benchmarks live in `bamabi-showcase` (`docs/BENCHMARKS.md`).

## Documentation

Browse all workspace and `xtask` docs locally in `rustdoc` via `cargo docs --open` (which lands on `target/doc/bamabi_docs/index.html` and builds `xtask::docs::architecture` and `xtask::docs::tests` alongside every workspace crate):

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — internal pipeline (`parse_args` -> `dispatch` -> `cmd_*` runners), `component_headers.rs` `ModComponent::C_DECL` extraction, toolchain isolation, and how to add a new check or subcommand.
- [`TESTS.md`](TESTS.md) — catalog of `xtask` unit tests (CLI parsing, flag conflict checks, help routing, `BENCHMARKS.md` marker parity, and checked-in header parity) and what each proves.
- Workspace docs: [`../ARCHITECTURE.md`](../ARCHITECTURE.md), [`../DEVELOPMENT.md`](../DEVELOPMENT.md), [`../TESTING.md`](../TESTING.md), [`../BENCHMARKS.md`](../BENCHMARKS.md).
