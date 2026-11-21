# `xtask` Test & Verification Catalog

`cargo test -p xtask` runs **8 unit tests** across [`src/main.rs`](src/main.rs) and [`src/component_headers.rs`](src/component_headers.rs). Because `xtask` orchestrates every CI gate for `bamabi-wire`, its tests verify three critical guarantees without requiring external tools or modifying the working tree:

1. **Deterministic CLI parsing & early rejection** (`src/main.rs`): guarantees that every documented subcommand, flag alias, default resolution, and invalid/conflicting flag combination is handled identically before spawning subprocesses.
2. **Benchmark docs marker & content validation** (`src/main.rs`): guarantees that `BENCHMARKS.md` marker splicing (`update_benchmarks_doc_section`) and structural checks (`validate_benchmarks_doc`) reject missing markers or missing language/workload permutations and validate the checked-in [`../BENCHMARKS.md`](../BENCHMARKS.md).
3. **Live component-header parity** (`src/component_headers.rs`): guarantees that `ModComponent::C_DECL` expansions for the 19 canonical testbed/interop fixtures match the checked-in `include/generated/testbed_components.h` byte-for-byte.

## Running `xtask` Tests

```bash
cargo test -p xtask
```

## Unit Test Inventory

### [`xtask/src/main.rs`](src/main.rs) (`mod tests`)

| Test | Why It Exists | What It Proves |
|---|---|---|
| `parses_help_variants` | Ensures developers and scripts can request global or per-subcommand usage via `-h`, `--help`, or `help <topic>` without accidentally executing a runner. | Empty args, `--help`, and `-h` produce `ParsedArgs::Help(None)`; `help miri`, `gen-component-headers --help`, and `c-test -h` produce `ParsedArgs::Help(Some(...))` for the exact `SubcommandKind`. |
| `parses_fmt_lint_header_and_c_test` | Prevents regressions in simple mode switches (`--fix`, `--check`, `--no-asan`, `--asan-only`) used directly by `.github/workflows/ci.yml`. | `fmt`, `fmt --fix`, `lint`, `header --check`, `gen-component-headers`, `c-test --no-asan`, and `c-test --asan-only` map to the expected `CommandSpec` values and defaults. |
| `parses_test_filters_and_cases` | Verifies `cargo xtask test` supports both bare CI execution and targeted local iteration with `PROPTEST_CASES` overrides. | Bare `test` produces `TestArgs::default()` (where `run_all_default()` is true); both `--cases 64` and `--cases=128` parse into `Some(usize)` alongside `--proptest`, `--header-parity`, `--no-default-features`, and `--doc`. |
| `parses_audit_miri_kani_nightly_and_ci` | Guards complex multi-option subcommands and default-expansion helpers (`MiriArgs::resolved`, `NightlyArgs::resolved`) used by CI and nightly workflows. | `audit` (`--baseline-rev`, `--skip-targets`, `--skip-typos`), `miri` (default `(true, true, false)` vs `--s390x`), `kani` (`-p`, `--harness=`), `nightly` (`--cases 512`, default `(true, true)`, `--randomize-layout` resolving `(false, true)` with default `2048` cases), and `ci` (`--full`, `--no-bench`, `--full --no-bench`) all parse accurately. |
| `parses_bench_flags_and_rejects_invalid_inputs` | Ensures `cargo xtask bench` enforces valid polyglot language/profile filters, deduplicates CSV lists, supports `--write-docs` / `--no-write-docs` / `--check-docs`, and rejects malformed/conflicting arguments before building guests. | `help bench` / `bench -h` route to `ParsedArgs::Help(Some(SubcommandKind::Bench))`, bare `bench` defaults to `[Rust, C99, Cpp17, Zig]` × `[Release, Debug]` with `is_full_matrix_request() == true`, `--langs` / `--guest-profiles` / `--workloads` / `--entities` / `--chunks` / `--quick` / `--strict-langs` / `--json` / `--write-docs` / `--no-write-docs` / `--check-docs` parse in both `--flag val` and `--flag=val` forms, combining `--write-docs` with `--check-docs` or `--no-write-docs` fails, and unknown languages/profiles or empty values return `Err`. |
| `validates_and_updates_benchmarks_doc_markers` | Prevents drift or accidental truncation of recorded results inside [`../BENCHMARKS.md`](../BENCHMARKS.md). | `update_benchmarks_doc_section` preserves surrounding prose and replaces marker body; `validate_benchmarks_doc` rejects missing markers or omitted subjects/workloads and passes on the checked-in [`../BENCHMARKS.md`](../BENCHMARKS.md). |
| `rejects_invalid_subcommands_and_flags` | Prevents silent flag typos, conflicting sanitizer modes, or zero-case proptest runs from slipping through CLI parsing. | Rejects unknown subcommands (`unknown`), extra args on flagless commands (`lint --extra`), conflicting flags (`c-test --no-asan --asan-only`), invalid zero `--cases` (`test --cases 0`), missing flag values (`test --cases`, `kani -p`), empty `--flag=` values (`audit --baseline-rev=`), and unknown subcommand flags (`ci --bogus`). |

### [`xtask/src/component_headers.rs`](src/component_headers.rs) (`mod tests`)

| Test | Why It Exists | What It Proves |
|---|---|---|
| `generated_testbed_header_matches_checked_in_file` | Ensures `bamabi_macros` (`#[derive(ModComponent)]`), `bamabi_sdk_gen::render_component_header`, and the checked-in `include/generated/testbed_components.h` never drift out of sync. | `write_or_check_component_headers(repo_root, true)` succeeds, proving byte-for-byte parity (modulo CRLF normalization) between live `ModComponent::C_DECL` expansions of all 19 fixture structs and `include/generated/testbed_components.h`. |
