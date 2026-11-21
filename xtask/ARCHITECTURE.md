# `xtask` Architecture & Implementation Guide

`xtask` is a `#![forbid(unsafe_code)]`, zero-external-dependency Cargo binary crate (`publish = false`, edition `2024`, MSRV `1.95`) that automates formatting, header generation, isolation lints, testing, static audits, Miri, Kani, nightly verification, and Layer-0 polyglot benchmarking for [`bamabi-wire`](../README.md).

## Module Layout

| File | Responsibility |
|---|---|
| [`src/main.rs`](src/main.rs) | CLI argument parsing (`parse_args`), usage text (`print_help`), command dispatch (`dispatch`), subprocess wrappers (`run_cmd`, `cargo_cmd`, `clean_toolchain_cargo_cmd`, `check_tool_available`), all 12 `cmd_*` runners, and `#[cfg(doc)] pub mod docs` (`ARCHITECTURE.md` / `TESTS.md` in `rustdoc`) |
| [`src/component_headers.rs`](src/component_headers.rs) | Canonical `#[derive(ModComponent)]` testbed structs, `generate_testbed_components_header()`, `write_or_check_component_headers()`, and `first_line_diff()` |
| [`Cargo.toml`](Cargo.toml) | Declares workspace-local dependencies only (`bamabi_macros`, `bamabi_sdk_gen`, `bamabi_sys`) and inherits `[workspace.lints]` |

## Execution Pipeline (`xtask/src/main.rs`)

```text
std::env::args().skip(1)
        │
        ▼
  parse_args(&raw_args) ──► Ok(ParsedArgs::Help(topic)) ──► print_help(topic) -> ExitCode::SUCCESS
        │
        ├── Err(msg) ──► stderr + ExitCode::FAILURE (before spawning any tool)
        ▼
  Ok(ParsedArgs::Run(CommandSpec))
        │
        ▼
  dispatch(cmd)
        ├── workspace_root() (parent of CARGO_MANIFEST_DIR)
        └── routes CommandSpec -> cmd_* / component_headers::write_or_check_component_headers
```

### 1. CLI Parsing (`parse_args` & `SubcommandKind`)

- Empty arguments, `-h`, `--help`, or `help` route to `ParsedArgs::Help(None)`.
- `help <subcommand>` and `<subcommand> -h` / `--help` resolve the subcommand via `parse_subcommand_kind` and route to `ParsedArgs::Help(Some(kind))`.
- Each subcommand parses its own flags into a dedicated struct (`FmtArgs`, `TestArgs`, `CTestArgs`, `CheckArgs`, `AuditArgs`, `MiriArgs`, `KaniArgs`, `NightlyArgs`, `BenchArgs`, `CiArgs`) or calls `reject_extra_args` (`lint`).
- Flag validation happens strictly before execution:
  - `parse_positive_usize` rejects `0` and non-integers for `--cases`.
  - `next_flag_value` rejects missing or empty option arguments; `--flag=` prefixes reject empty values.
  - `parse_c_test_args` rejects combining `--no-asan` and `--asan-only`.
  - `parse_csv_enum` validates and deduplicates `--langs` (`rust`, `c99`, `cpp17`, `zig`) and `--guest-profiles` (`release`, `debug`).
  - `MiriArgs::resolved()` expands default invocation to `(tree_borrows=true, stacked_borrows=true, s390x=false)`.
  - `NightlyArgs::resolved()` expands default invocation to `(s390x=true, randomize_layout=true)` with default `cases = 2048`.

### 2. Toolchain & Subprocess Hygiene

- **`workspace_root()`**: Resolves the repository root as `Path::new(env!("CARGO_MANIFEST_DIR")).parent()`.
- **`cargo_cmd(root)`**: Spawns `"cargo"` (the `rustup` proxy, rather than `env!("CARGO")`) anchored at `root`.
- **`clean_toolchain_cargo_cmd(root)`**: Strips `RUSTUP_TOOLCHAIN` and `RUSTC` from the child environment. This is required when invoking `+nightly`, `cargo deny`, `cargo semver-checks`, or `cargo kani` from inside `cargo xtask`, because Cargo sets `RUSTUP_TOOLCHAIN`/`RUSTC` for the running `xtask` process and would otherwise override `+nightly` or tool-specific toolchains.
- **`check_tool_available`**: Probes `program args...` silently with `RUSTUP_TOOLCHAIN` and `RUSTC` cleared, returning an actionable install hint if the binary or component is missing.

### 3. Subcommand Runners (`cmd_*`)

| Subcommand | Runner | Behavior |
|---|---|---|
| `fmt` | `cmd_fmt` | Runs `cargo fmt --all -- --check` (or `cargo fmt --all` with `--fix`). |
| `lint` | `cmd_lint` | Calls `verify_forbid_unsafe` (checks line-trimmed `#![forbid(unsafe_code)]` in `bamabi_docs/src/lib.rs`, `bamabi_guest/src/lib.rs`, `bamabi_macros/src/lib.rs`, `bamabi_sdk_gen/src/lib.rs`, `xtask/src/main.rs`), `verify_zero_bevy` (parses `cargo tree --workspace`, failing if any non-`bamabi` crate name contains `bevy`), then `cargo clippy --workspace --all-targets -- -D warnings` (default & `--all-features`) and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (default & `--all-features`). |
| `test` | `cmd_test` | Without filter flags, runs `--workspace --no-default-features`, `--workspace --all-features`, and `--doc --all-features`. Filter flags run any combination of those plus `--proptest` (`proptest_wire`, `proptest_guest_sys`, `proptest_witness`) and `--header-parity` (`bamabi_sdk_gen --test header_parity`), optionally setting `PROPTEST_CASES` via `--cases N`. |
| `c-test` | `cmd_c_test` | Unless `--asan-only`: syntax-checks `include/bamabi.h` and `include/generated/testbed_components.h` with `$CC` (default `cc`) `-std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only -x c`, then compiles and runs `tests/standalone_c_test.c` -> `target/standalone_c_test`. Unless `--no-asan`: compiles and runs `tests/standalone_c_test.c` with `$CLANG` (preferring `/usr/bin/clang` on macOS to avoid `swiftly` shim stalls) `-fsanitize=address,undefined -fno-sanitize-recover=all` and platform-portable `ASAN_OPTIONS` (omitting `detect_leaks=1` on macOS ARM64) + `UBSAN_OPTIONS`. |
| `header` | `cmd_header` | With `--check`: compares `bamabi_sdk_gen::render_c99_header()` against `include/bamabi.h` (normalizing `\r\n` to `\n`) and reports `first_line_diff` on mismatch. Without `--check`: calls `bamabi_sdk_gen::write_or_check_header(root, false)`. |
| `gen-component-headers` | `component_headers::write_or_check_component_headers` | Generates or byte-checks `include/generated/testbed_components.h` (detailed below). |
| `audit` | `cmd_audit` | Unless `--skip-targets`: checks `rustup target list --installed` and runs `cargo check -p bamabi_sys --target` for `i686-unknown-linux-gnu` and `wasm32-unknown-unknown`. Unless `--skip-typos`: runs `typos`. Runs `cargo deny --all-features check advisories licenses bans sources`. With `--baseline-rev <rev>`: runs `cargo semver-checks check-release -p bamabi_sys --baseline-rev <rev>`. |
| `miri` | `cmd_miri` | Verifies `cargo +nightly miri --version`. Runs `run_miri_suite` (`-p bamabi_sys -p bamabi_guest_sys -p bamabi_guest --all-features`, plus `-p bamabi_macros --test mod_component_vtable`) under Tree Borrows (`-Zmiri-tree-borrows -Zmiri-strict-provenance`) and/or Stacked Borrows (`-Zmiri-strict-provenance`). With `--s390x`, runs `s390x-unknown-linux-gnu` Tree Borrows + strict provenance on `bamabi_sys`, `bamabi_guest_sys`, and `bamabi_guest` (`--all-features`). |
| `kani` | `cmd_kani` | Verifies `cargo kani --version`, then runs `cargo kani` on `bamabi_sys` and `bamabi_guest_sys` (or `-p` packages) with optional `--harness <pat>`, `-Z unstable-options --harness-timeout 15m --output-format terse -j`. |
| `nightly` | `cmd_nightly` | Runs `cmd_miri` with `--s390x` and/or `RUSTFLAGS="-Zrandomize-layout" PROPTEST_CASES=<cases> cargo +nightly test` across `bamabi_sys`, `bamabi_guest_sys`, `bamabi_guest` (`--all-features`), `bamabi_macros` (`--lib --test mod_component_vtable`), and `bamabi_sdk_gen`. |
| `bench` | `cmd_bench` | With `--check-docs`: validates `BENCHMARKS.md` markers and subject/workload coverage via `validate_benchmarks_doc`. Otherwise exports `bamabi.h`/`bamabi.hpp`/`bamabi.zig` via `bamabi_sdk_gen::export_sdk` into `target/bench_sdk/`, builds `bamabi_bench` in `--release`, compiles selected guest languages (`rust`, `c99`, `cpp17`, `zig`; skipping absent optional toolchains unless `--strict-langs` or explicitly listed in `--langs`) across selected profiles (`release`, `debug`) into `target/bench_guests/`, runs `bamabi_bench` passing `--guest <lang>:<profile>=<path>` alongside `--workloads`, `--entities`, `--chunks`, `--quick`, `--json`, and `--markdown target/bench-summary.md`, and updates `BENCHMARKS.md` automatically on full 8-guest runs (or `--write-docs`; disabled via `--no-write-docs`). |
| `ci` | `cmd_ci` | Runs `fmt` -> `header --check` -> `gen-component-headers --check` -> `bench --check-docs` -> `lint` -> `test` -> `c-test` -> `bench --quick --no-write-docs` (unless `--no-bench`), plus optional `--audit`, `--miri`, `--kani` (or all three via `--full`). |

## Component Header Extraction (`xtask/src/component_headers.rs`)

`component_headers.rs` bridges [`bamabi_macros`] and [`bamabi_sdk_gen`] inside `xtask` without exposing test-fixture types in the library crates:

1. Declares 19 `#[derive(ModComponent)]` structs matching the canonical `testbed::*` (17 types) and `c_interop::*` (2 types) fixtures (`Position`, `Velocity`, `CombatHealth`, `NameTag`, `MetadataTag`, `Mass`, `Armor`, `FilteredTransform`, `CFilteredTransform`, `ChangeSensor`, `CChangeSensor`, `FreshSpawnTag`, `CFreshSpawnTag`, `TransientMarker`, `CTransientMarker`, `RustFilterTelemetry`, `CFilterTelemetry`, `Vec3`, `CInteropComponent`).
2. `#[derive(ModComponent)]` computes each type's `ModComponent::C_DECL` compile-time constant string (including C99 struct definitions, VTable types, and `BAM_STATIC_ASSERT` size/offset checks).
3. `generate_testbed_components_header()` passes `&[section_for::<T>(), ...]` to `bamabi_sdk_gen::render_component_header("BAMABI_GENERATED_TESTBED_COMPONENTS_H", "../bamabi.h", ...)`.
4. `write_or_check_component_headers(root, check_only)`:
   - In check mode (`--check`), normalizes `\r\n` -> `\n` on `include/generated/testbed_components.h` and errors with `first_line_diff` if it diverges from `expected`.
   - In write mode, writes `include/generated/testbed_components.h` only when missing or different.

## Recipe: Adding a New Check or Subcommand

1. **Adding a check to an existing runner** (for example, a new lint inside `cmd_lint` or a new fixture in `component_headers.rs`):
   - Keep `xtask` free of `unsafe` (`#![forbid(unsafe_code)]`) and avoid external crates.
   - Use `cargo_cmd(root)` for standard workspace commands and `clean_toolchain_cargo_cmd(root)` for `+nightly` or standalone Cargo subcommands.
   - If adding a struct to `component_headers.rs`, derive `ModComponent` with `#[mod_component(name = "...")]`, append `section_for::<MyType>()` in `generate_testbed_components_header()`, and regenerate `include/generated/testbed_components.h` via `cargo xtask gen-component-headers`.
2. **Adding a new subcommand**:
   - Add a variant to `SubcommandKind` and `CommandSpec` (plus an `*Args` struct if it accepts flags).
   - Register the subcommand string in `parse_subcommand_kind`, wire its flag parser in `parse_args`, and document its usage in `print_help`.
   - Implement `cmd_<name>(root: &Path, ...)` and match it in `dispatch`.
   - Add unit tests in `xtask/src/main.rs` (`#[cfg(test)] mod tests`) covering valid flags, defaults, and invalid/conflicting arguments, and document them in [`TESTS.md`](TESTS.md).
3. **Verify before committing**:
   - `cargo fmt -p xtask -- --check`
   - `cargo clippy -p xtask --all-targets -- -D warnings`
   - `RUSTDOCFLAGS="-D warnings" cargo doc -p xtask --no-deps`
   - `cargo test -p xtask`
