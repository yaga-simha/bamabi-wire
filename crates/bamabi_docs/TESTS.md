# `bamabi_docs` Test & Verification Guide

Because `bamabi_docs` is a non-published (`publish = false`), documentation-only crate with `#![no_std]`, `#![forbid(unsafe_code)]`, and zero runtime code or dependencies, its verification surface focuses on rustdoc cleanliness, lint compliance, spellchecking, and workspace landing-page structure rather than unit/runtime tests or Criterion benchmarks.

## 1. Verification Matrix

| Check | Command | What It Proves |
|---|---|---|
| **Rustdoc strict build** | `RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps` | `src/lib.rs`, `README.md`, `ARCHITECTURE.md`, `DEVELOPMENT.md`, `TESTING.md`, `BENCHMARKS.md`, `spec.md`, and `bamabi_docs::crates` render cleanly into `target/doc/bamabi_docs/` with **zero** broken links, bare-URL warnings, invalid HTML tags, or `missing_docs` warnings. |
| **Workspace landing check** | `cargo docs --no-deps` (or `cargo doc --workspace --no-deps`) | Builds the entire workspace into `target/doc/`, producing `target/doc/bamabi_docs/index.html` ahead of `bamabi_guest` alphabetically so `--open` lands on `bamabi_docs`. |
| **Doctest harness** | `cargo test --doc -p bamabi_docs` | Verifies the crate compiles cleanly under `rustdoc --test` with zero doctest failures (`0 passed; 0 failed; 1 ignored`, where the ignored block is `ARCHITECTURE.md`'s `rust,ignore` component snippet). |
| **Crate build & test target** | `cargo test -p bamabi_docs` | Verifies `bamabi_docs` compiles under the `test` profile (`#![no_std]`, `#![forbid(unsafe_code)]`, edition `2024`, MSRV `1.87`) with zero warnings. |
| **Clippy & workspace lints** | `cargo clippy -p bamabi_docs --all-targets -- -D warnings` | Enforces `[workspace.lints]` and `clippy.toml` `doc-valid-idents` rules across `bamabi_docs`. |
| **Formatting** | `cargo fmt -p bamabi_docs -- --check` | Ensures `crates/bamabi_docs/src/lib.rs` matches `rustfmt` defaults. |
| **Isolation & safety policy** | `cargo xtask lint` | `verify_forbid_unsafe` confirms `#![forbid(unsafe_code)]` is present in `crates/bamabi_docs/src/lib.rs`, and `verify_zero_bevy` confirms zero non-`bamabi` Bevy dependencies in `cargo tree --workspace`. |
| **Spellcheck** | `typos crates/bamabi_docs` | Confirms zero spelling errors against `.typos.toml`. |

## 2. Running All `bamabi_docs` Checks

```bash
cargo fmt -p bamabi_docs -- --check
cargo clippy -p bamabi_docs --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps
cargo test -p bamabi_docs
cargo test --doc -p bamabi_docs
typos crates/bamabi_docs
```
