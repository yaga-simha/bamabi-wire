# `bamabi_docs`

Non-published (`publish = false`), `#![no_std]`, `#![forbid(unsafe_code)]`, **zero-dependency** workspace documentation hub for [`bamabi-wire`](../../README.md) (`ABI 0.3.0`, `v0.3.0`, Layer 0).

## Role

`bamabi_docs` sorts alphabetically before every other workspace member (`bamabi_guest`, `bamabi_guest_sys`, `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_sys`, `xtask`), so running `cargo docs --open` (or `cargo doc --open`) at the repository root lands directly on `target/doc/bamabi_docs/index.html` instead of a single leaf crate.

Its crate-root documentation explains the 5-repo federated `bamabi` stack, the 6-crate Layer 0 DAG, the end-to-end component/system/command/header/CI flow, and quickstart commands, while `#[cfg(doc)]` submodules embed all root guides and index every crate in `target/doc/`:

- `bamabi_docs::overview` — [`../../README.md`](../../README.md)
- `bamabi_docs::architecture` — [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- `bamabi_docs::development` — [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md)
- `bamabi_docs::testing` — [`../../TESTING.md`](../../TESTING.md)
- `bamabi_docs::benchmarks` — [`../../BENCHMARKS.md`](../../BENCHMARKS.md)
- `bamabi_docs::wire_spec` — [`../../spec.md`](../../spec.md)
- `bamabi_docs::crates` — curated index of all 6 functional crates and their `ARCHITECTURE.md` / `TESTS.md` pages

## Documentation Map

- **Crate Architecture:** [`ARCHITECTURE.md`](ARCHITECTURE.md) — alphabetical landing-page design, `#[cfg(doc)]` module layout, isolation invariants, and maintenance recipe.
- **Test & Verification Guide:** [`TESTS.md`](TESTS.md) — `RUSTDOCFLAGS="-D warnings"` doc build, `cargo test --doc -p bamabi_docs`, `clippy`, `fmt`, `typos`, and `xtask` lint checks.
- **Workspace Guides:** [`../../README.md`](../../README.md) · [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) · [`../../TESTING.md`](../../TESTING.md) · [`../../BENCHMARKS.md`](../../BENCHMARKS.md) · [`../../spec.md`](../../spec.md)

## Setup & Building

Requires Rust edition `2024` (crate MSRV `1.87`; workspace `1.95`). Because `publish = false`, downstream mods and host crates never depend on `bamabi_docs`.

```bash
# Build and open the full workspace rustdoc starting at bamabi_docs/index.html
cargo docs --open
# Equivalent unaliased command (add --all-features to render derive examples):
cargo doc --workspace --no-deps --open

# Build only bamabi_docs documentation with warnings denied
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps
```

## Testing & Verification

See [`TESTS.md`](TESTS.md) for details on every check.

```bash
cargo fmt -p bamabi_docs -- --check
cargo clippy -p bamabi_docs --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps
cargo test -p bamabi_docs
cargo test --doc -p bamabi_docs
```

## Benchmarking

`bamabi_docs` is a pure documentation crate inside Layer 0 (`bamabi-wire`) and embeds [`../../BENCHMARKS.md`](../../BENCHMARKS.md) as `bamabi_docs::benchmarks` (powered by the standalone `cargo xtask bench` suite; see [`../../BENCHMARKS.md`](../../BENCHMARKS.md)). Downstream Bevy-integrated runtime ECS and FFI benchmarks live in Layer 4 (`bamabi-showcase`, `docs/BENCHMARKS.md`). To measure documentation build timings across the workspace, run `cargo doc --workspace --no-deps --timings`.

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
