# `bamabi_docs` Architecture & Implementation Guide

`bamabi_docs` is the non-published (`publish = false`), `#![no_std]`, `#![forbid(unsafe_code)]`, zero-dependency documentation hub crate for [`bamabi-wire`](../../README.md) (Layer 0, `ABI 0.3.0`).

## 1. Why `bamabi_docs` Exists

When `cargo doc --open` (or the `.cargo/config.toml` alias `cargo docs --open`) builds a multi-crate Cargo workspace without `-p`, Cargo opens the first workspace library crate alphabetically. Before `bamabi_docs` existed, that defaulted to `bamabi_guest` (`bamabi_guest` < `bamabi_guest_sys` < `bamabi_macros` < `bamabi_sdk_gen` < `bamabi_sys` < `xtask`), which forced `bamabi_guest` to carry workspace-wide landing banners and duplicate `include_str!` submodules.

Adding `crates/bamabi_docs` (`name = "bamabi_docs"`, which sorts before `bamabi_guest`) provides a clean separation of concerns:

- **`target/doc/bamabi_docs/index.html`** serves as the canonical workspace landing page, explaining the 5-repo federated stack, the 6-crate DAG, how all components/systems/headers/CI gates tie together, and quickstart commands.
- **Functional crates** (`bamabi_sys`, `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_guest_sys`, `bamabi_guest`, `xtask`) stay focused on their own APIs, architectures, and test catalogs while linking back to `../bamabi_docs/index.html`.

## 2. Module Layout (`src/lib.rs`)

`crates/bamabi_docs/src/lib.rs` exports zero runtime symbols and seven `#[cfg(doc)]` documentation modules:

| Module | Source | Generated Page |
|---|---|---|
| Crate root (`//!`) | [`src/lib.rs`](src/lib.rs) | `target/doc/bamabi_docs/index.html` |
| `overview` | [`../../README.md`](../../README.md) | `target/doc/bamabi_docs/overview/index.html` |
| `architecture` | [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) | `target/doc/bamabi_docs/architecture/index.html` |
| `development` | [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) | `target/doc/bamabi_docs/development/index.html` |
| `testing` | [`../../TESTING.md`](../../TESTING.md) | `target/doc/bamabi_docs/testing/index.html` |
| `benchmarks` | [`../../BENCHMARKS.md`](../../BENCHMARKS.md) | `target/doc/bamabi_docs/benchmarks/index.html` |
| `wire_spec` | [`../../spec.md`](../../spec.md) | `target/doc/bamabi_docs/wire_spec/index.html` |
| `crates` | Curated crate index in [`src/lib.rs`](src/lib.rs) | `target/doc/bamabi_docs/crates/index.html` |

### Why `#[cfg(doc)]`?

Each submodule is gated with `#[cfg(doc)]` so:

1. `rustdoc` (`cargo doc` / `cargo docs`) renders all root markdown documents and `spec.md` into browsable HTML pages and checks all intra-doc links, bare URLs, and HTML tags under `RUSTDOCFLAGS="-D warnings"`.
2. `cargo test --doc -p bamabi_docs` and `cargo build -p bamabi_docs` do not attempt to compile unlabelled markdown code blocks inside `README.md`, `ARCHITECTURE.md`, `DEVELOPMENT.md`, `TESTING.md`, `BENCHMARKS.md`, or `spec.md` as Rust doctests.

## 3. Isolation & Policy Invariants

- **`publish = false`:** Prevents accidental publishing of the documentation hub crate to crates.io.
- **`#![no_std]` & zero dependencies:** Ensures `bamabi_docs` adds zero external or Bevy dependencies to `cargo tree --workspace` (`verify_zero_bevy`) and builds in milliseconds on every target.
- **`#![forbid(unsafe_code)]` & `#![warn(missing_docs)]`:** Enforced in [`src/lib.rs`](src/lib.rs) and checked on every `cargo xtask lint` run by `verify_forbid_unsafe` in [`../../xtask/src/main.rs`](../../xtask/src/main.rs).
- **`rust-version = "1.87"`:** Matches the library crate MSRV across `crates/` (while inheriting `version`, `edition = "2024"`, `license`, `authors`, `repository`, and `[lints]` from [`../../Cargo.toml`](../../Cargo.toml)).

## 4. Maintenance Recipe

1. When adding a new root-level guide (for example `MIGRATION.md`), embed it as a `#[cfg(doc)] pub mod <name> { #![doc = include_str!("../../../<FILE>.md")] }` in [`src/lib.rs`](src/lib.rs) and link it from the crate-root `//!` navigation list and `crates` module.
2. When adding a new workspace crate, add its root, `docs::architecture`, and `docs::tests` links to both the crate-root navigation strip (`../<crate>/index.html`) and `pub mod crates` (`../../<crate>/...`).
3. Verify zero warnings and clean formatting with:
   - `cargo fmt -p bamabi_docs -- --check`
   - `cargo clippy -p bamabi_docs --all-targets -- -D warnings`
   - `RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps`
   - `cargo test -p bamabi_docs`
   - `cargo test --doc -p bamabi_docs`
