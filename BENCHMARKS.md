# Benchmarking & Testing Strategy (`bamabi-wire`)

`bamabi-wire` is **Layer 0** of the federated `bamabi` framework: zero `bevy_*` dependencies, zero external runtime dependencies in `bamabi_sys`, `bamabi_sdk_gen`, `bamabi_guest_sys`, `xtask`, or `benches/bamabi_bench`, and a single `ABI 0.3.0` (`0x0003_0000`) C-ABI wire contract shared across Rust, C99, C++17, and Zig guests.

Because Layer 0 owns the FFI boundary rather than Bevy's scheduler or renderer, **testing** and **benchmarking** are cleanly separated:

1. **Correctness & Soundness Pyramid ([`TESTING.md`](TESTING.md)):** An 8-tier verification suite (`fmt`, isolation/lints, unit/integration/doc tests, `trybuild` compile-fail controls, `proptest` + nightly `-Zrandomize-layout`, strict C99/C++17/Zig compiler & ASan/UBSan checks, Tree/Stacked Borrows + big-endian `s390x` Miri, `Kani` formal proofs `PO-W1..PO-W8` & `PO-G1..PO-G9`, and `cargo xtask audit`) proves ABI layout safety, memory provenance, and panic-boundary soundness.
2. **Polyglot Host-Guest Benchmark Harness ([`benches/bamabi_bench`](benches/bamabi_bench/Cargo.toml) + [`benches/guests`](benches/guests/rust_guest/Cargo.toml)):** Measures raw C-ABI boundary dispatch, chunk iteration, command encoding, and host-slot round-trip costs across real dynamically loaded `.dylib` / `.so` / `.dll` guest modules against an in-process `native` Rust baseline, with **zero Criterion or external benchmarking dependencies**.

> Downstream end-to-end Bevy ECS scheduling, hot-reload, and multi-mod Criterion benchmarks live in **Layer 4** (`bamabi-showcase`, `docs/BENCHMARKS.md`).

---

## 1. Testing & Benchmarking Strategy

### Correctness First on Every Benchmark Permutation

Every benchmark permutation executes a deterministic **pre-timing verification pass** (and per-workload post-condition assertions) before recording any timing sample:

- **Manifest & ABI Handshake:** Verifies `abi_version == 0x0003_0000` (`ABI 0.3.0`), non-empty manifest name/version, and registration of all four `BAM_SCHEDULE_UPDATE` benchmark systems.
- **Dense Iteration Parity (`dense_query_update`):** Compares component checksums across all entities against the in-process `native` Rust baseline within tight floating-point tolerance (`1e-3` relative) and verifies change-tick marking on `bench::Position`.
- **Command Queue Integrity (`command_burst`):** Verifies each tick commits **exactly 64 spawns**, **64 `bench::Position` pod inserts**, **1 checkpoint**, **1 rollback**, and **1 rolled-back spawn**.
- **Host API Round-Trip Integrity (`host_api_roundtrip`):** Verifies each tick invokes `intern_symbol` and `res_ticks` **64 times** against `bench::SimParams` (`RESOURCE_READ`).
- **Clean Teardown:** Invokes `bamabi_shutdown` before unloading every dynamic library handle (`dlclose` / `FreeLibrary`).

### Permutation Matrix

The host harness (`bamabi_bench`) is **always compiled in `--release` mode** (`opt-level = 3`) so host-side storage, chunk iteration, and command recording never dilute guest measurements. Each guest language is compiled into real shared libraries in **both `release` and `debug` profiles**:

| Subject | Guest SDK / Header | Build Profiles | Key Boundary Characteristics |
| --- | --- | --- | --- |
| `native` | In-process Rust baseline | Host `--release` | Direct Rust slice loop & host queue calls with no `dlopen` / `extern "C"` boundary |
| `rust:release` / `rust:debug` | [`bamabi_guest`](crates/bamabi_guest/src/lib.rs) (`#![forbid(unsafe_code)]`) | `--release` / dev | Safe typed queries (`TypedChunkIter`), `catch_unwind` panic guards, `ReentrantCell` guard |
| `c99:release` / `c99:debug` | [`include/bamabi.h`](include/bamabi.h) (`-std=c99 -Wall -Wextra -Werror -pedantic`) | `-O3` / `-O0 -g` | Raw function-pointer slots, `BAM_RESTRICT` pointers, explicit `bam_host_api_covers_slot` |
| `cpp17:release` / `cpp17:debug` | `bamabi.hpp` (`-std=c++17 -Wall -Wextra -Werror -Wpedantic`) | `-O3` / `-O0 -g` | `SystemContextView`, `ChunkView`, `CommandsView`, RAII `ScopedTransaction`, `guard_boundary` |
| `zig:release` / `zig:debug` | `bamabi.zig` + `b.addTranslateC` (`Zig 0.16.0`) | `ReleaseFast` / `Debug` | Typed error-mapped views, `noalias` chunk slice loop, explicit `callconv(.c)` callbacks |

### Workloads & Metrics

| Workload | What It Exercises | Default Sweeps (`--quick` vs full) |
| --- | --- | --- |
| `noop` | Empty `BAM_SCHEDULE_UPDATE` system returning `OK` immediately | Single configuration (pure FFI trampoline + panic/reentrancy guard overhead) |
| `dense_query_update` | Chunked SoA read/write iteration integrating `Position += Velocity * 0.016` | Full: `entities ∈ {1000, 100000}` × `chunks ∈ {256, 4096}`; Quick: `chunks = {256}` |
| `command_burst` | 64× `cmd_spawn` + `cmd_insert` (`Position`), plus `cmd_checkpoint` + `cmd_spawn` + `cmd_rollback` | Single configuration (amortized command queue + transaction rollback cost) |
| `host_api_roundtrip` | 64× `res_ptr` + `res_ticks` + `intern_symbol` accumulated into a sink | Single configuration (repeated guest → host vtable lookup and validation cost) |

- **Sampling:** Full runs execute `15` warmup + `60` timed samples per permutation; `--quick` executes `5` warmup + `20` timed samples.
- **Metrics:** Reports `Min`, `Median`, `P95`, and `Mean` (`ns/op`), `ns/entity` and `M ent/s` throughput for `dense_query_update`, and `vs Native` ratio (`subject median / native median`).
- **Interpreting Results:**
  - `noop` isolates per-system entry overhead: raw C99/C++17/Zig callbacks cross only a function pointer (`~2–5 ns`), whereas `rust:release` pays for safe `catch_unwind` + `ReentrantCell` atomic re-entrancy guards with lazy `sys_host_ctx` / `sys_commands` (`~6 ns`).
  - As entity count scales (`1000` -> `100000`), per-call trampoline overhead amortizes away and all four `release` guests approach `1.05x–1.35x` of the `native` baseline.
  - `debug` guest profiles show the impact of unoptimized guest codegen (`-O0` / `Debug` / `dev`) even when the host engine itself runs in `--release`.

---

## 2. Running & Integrating with Docs & CI

```bash
# Run the full default matrix across all 4 languages (release & debug) and auto-update BENCHMARKS.md
cargo xtask bench --strict-langs

# Run without modifying BENCHMARKS.md (for example during quick local checks)
cargo xtask bench --no-write-docs

# Fast CI-style run requiring all 4 toolchains (Rust, C99, C++17, Zig) and emitting JSON + Markdown
cargo xtask bench --quick --strict-langs --json target/bench-results.json

# Verify BENCHMARKS.md contains valid markers and complete subject/workload coverage
cargo xtask bench --check-docs
```

- **Automatic Report Updates:** Every full 8-guest run of `cargo xtask bench` (all 4 languages × 2 profiles with no `--langs`, `--guest-profiles`, or `--workloads` filter) automatically refreshes the **Latest Recorded Report** section below (pass `--no-write-docs` to skip updating disk, or `--write-docs` to force updating). Filtered runs leave `BENCHMARKS.md` untouched unless `--write-docs` is given.
- **Local CI (`cargo xtask ci`):** Runs `cargo xtask bench --check-docs` alongside header parity checks, strict `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (which renders this file at `bamabi_docs::benchmarks`), workspace tests, C99 ASan/UBSan tests, and `bench --quick --no-write-docs` (skip benchmarks via `cargo xtask ci --no-bench`).
- **GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)):**
  - The `quality-and-tests` job runs `cargo xtask bench --check-docs` and `cargo xtask lint` (enforcing strict `rustdoc` checks on `bamabi_docs::benchmarks`).
  - The `benchmarks` job installs Rust + Zig `0.16.0`, runs the full matrix on `push` to `main`/`master` (or `--quick` on pull requests) with `--strict-langs --write-docs --json target/bench-results.json`, appends `target/bench-summary.md` to `$GITHUB_STEP_SUMMARY`, verifies `RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_docs --no-deps`, uploads `target/bench-results.json`, `target/bench-summary.md`, and `BENCHMARKS.md` as artifacts, and auto-commits the updated `BENCHMARKS.md` on `main`/`master` pushes (`chore(bench): update BENCHMARKS.md report [skip ci]`).

---

## 3. Latest Recorded Benchmark Report

Updated automatically on every full 8-guest `cargo xtask bench` run (and on every `main`/`master` CI push).

<!-- BENCHMARKS_RESULTS_START -->
- **Commit:** `f45ca9b`
- **Host target:** `macos/aarch64` (`bamabi_bench` built with `--release`)
- **Rust:** `rustc 1.98.1 (48a229cea 2026-09-01)`
- **C99 (`cc`):** `Apple clang version 21.0.0 (clang-2100.4.1.1)`
- **C++17 (`c++`):** `Apple clang version 21.0.0 (clang-2100.4.1.1)`
- **Zig:** `0.16.0`

### bamabi-wire ABI 0.3.0 Polyglot Benchmark Report (full, warmup=15, samples=60)

#### Verified Guests

- `rust:release`: `bench_guest_rust` v0.3.0 (ABI 0.3.0)
- `rust:debug`: `bench_guest_rust` v0.3.0 (ABI 0.3.0)
- `c99:release`: `bench_guest_c99` v0.3.0 (ABI 0.3.0)
- `c99:debug`: `bench_guest_c99` v0.3.0 (ABI 0.3.0)
- `cpp17:release`: `bench_guest_cpp17` v0.3.0 (ABI 0.3.0)
- `cpp17:debug`: `bench_guest_cpp17` v0.3.0 (ABI 0.3.0)
- `zig:release`: `bench_guest_zig` v0.3.0 (ABI 0.3.0)
- `zig:debug`: `bench_guest_zig` v0.3.0 (ABI 0.3.0)

#### Summary Highlights (Release Guests vs Native)

| Workload                           | Native Median (ns) | Fastest Release Guest               | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | ----------------------------------- | -----------------------: |
| noop                               |               1.15 | `cpp17:release`: 3.48 ns (3.03x)    |          6.21 ns (5.40x) |
| dense_query_update (1000, c256)    |              285.8 | `cpp17:release`: 346.7 ns (1.21x)   |         531.7 ns (1.86x) |
| dense_query_update (1000, c4096)   |              295.0 | `c99:release`: 291.7 ns (0.99x)     |         349.2 ns (1.18x) |
| dense_query_update (100000, c256)  |            56333.4 | `cpp17:release`: 60929.2 ns (1.08x) |       77062.5 ns (1.37x) |
| dense_query_update (100000, c4096) |            51616.6 | `cpp17:release`: 51725.0 ns (1.00x) |       53120.9 ns (1.03x) |
| command_burst                      |              185.6 | `c99:release`: 598.2 ns (3.22x)     |         989.7 ns (5.33x) |
| host_api_roundtrip                 |             1232.5 | `c99:release`: 1778.3 ns (1.44x)    |        2430.6 ns (1.97x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.14 |        1.15 |     1.16 |      1.15 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |    282.5 |       285.8 |    294.2 |     287.8 |     0.286 |  3498.7 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |    294.2 |       295.0 |    299.2 |     296.4 |     0.295 |  3389.8 |     1.00x |
| native        | dense_query_update |   100000 |   256 |  56216.6 |     56333.4 |  58525.0 |   56721.3 |     0.563 |  1775.1 |     1.00x |
| native        | dense_query_update |   100000 |  4096 |  51133.4 |     51616.6 |  52941.8 |   51912.2 |     0.516 |  1937.4 |     1.00x |
| native        | command_burst      |        - |     - |    184.4 |       185.6 |    186.4 |     185.6 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |   1230.8 |      1232.5 |   1275.8 |    1239.5 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     6.20 |        6.21 |     6.49 |      6.24 |         - |       - |     5.40x |
| rust:release  | dense_query_update |     1000 |   256 |    525.8 |       531.7 |    540.0 |     533.0 |     0.532 |  1880.9 |     1.86x |
| rust:release  | dense_query_update |     1000 |  4096 |    347.5 |       349.2 |    351.7 |     349.6 |     0.349 |  2864.0 |     1.18x |
| rust:release  | dense_query_update |   100000 |   256 |  76766.6 |     77062.5 |  78608.4 |   77381.5 |     0.771 |  1297.6 |     1.37x |
| rust:release  | dense_query_update |   100000 |  4096 |  53008.4 |     53120.9 |  54200.0 |   53354.7 |     0.531 |  1882.5 |     1.03x |
| rust:release  | command_burst      |        - |     - |    976.7 |       989.7 |   1018.1 |     992.4 |         - |       - |     5.33x |
| rust:release  | host_api_roundtrip |        - |     - |   2428.3 |      2430.6 |   2473.3 |    2440.0 |         - |       - |     1.97x |
| rust:debug    | noop               |        - |     - |    139.5 |       140.0 |    141.2 |     140.1 |         - |       - |   121.73x |
| rust:debug    | dense_query_update |     1000 |   256 |  12432.5 |     12503.3 |  12610.0 |   12500.6 |    12.503 |    80.0 |    43.75x |
| rust:debug    | dense_query_update |     1000 |  4096 |   9853.3 |      9922.5 |  10095.0 |    9934.0 |     9.923 |   100.8 |    33.64x |
| rust:debug    | dense_query_update |   100000 |   256 |  1188058 |     1211379 |  1275508 |   1215190 |    12.114 |    82.6 |    21.50x |
| rust:debug    | dense_query_update |   100000 |  4096 | 856291.8 |    866641.6 | 882925.0 |  867651.4 |     8.666 |   115.4 |    16.79x |
| rust:debug    | command_burst      |        - |     - |   7170.0 |      7210.6 |   7276.7 |    7213.7 |         - |       - |    38.86x |
| rust:debug    | host_api_roundtrip |        - |     - |  25880.3 |     26040.7 |  26451.1 |   26074.3 |         - |       - |    21.13x |
| c99:release   | noop               |        - |     - |     3.54 |        3.55 |     3.83 |      3.58 |         - |       - |     3.08x |
| c99:release   | dense_query_update |     1000 |   256 |    341.7 |       347.5 |    351.7 |     347.2 |     0.347 |  2877.7 |     1.22x |
| c99:release   | dense_query_update |     1000 |  4096 |    289.2 |       291.7 |    294.2 |     291.7 |     0.292 |  3428.6 |     0.99x |
| c99:release   | dense_query_update |   100000 |   256 |  60766.6 |     61441.7 |  63533.2 |   61716.1 |     0.614 |  1627.6 |     1.09x |
| c99:release   | dense_query_update |   100000 |  4096 |  51608.2 |     52291.6 |  56800.0 |   53084.5 |     0.523 |  1912.4 |     1.01x |
| c99:release   | command_burst      |        - |     - |    531.9 |       598.2 |    621.1 |     592.1 |         - |       - |     3.22x |
| c99:release   | host_api_roundtrip |        - |     - |   1777.5 |      1778.3 |   1819.7 |    1785.7 |         - |       - |     1.44x |
| c99:debug     | noop               |        - |     - |     6.32 |        6.81 |    14.82 |      7.97 |         - |       - |     5.92x |
| c99:debug     | dense_query_update |     1000 |   256 |   3806.7 |      3823.3 |   3889.2 |    3832.4 |     3.823 |   261.6 |    13.38x |
| c99:debug     | dense_query_update |     1000 |  4096 |   3696.7 |      3716.7 |   3843.3 |    3732.4 |     3.717 |   269.1 |    12.60x |
| c99:debug     | dense_query_update |   100000 |   256 | 400483.2 |    401904.2 | 406791.6 |  402640.0 |     4.019 |   248.8 |     7.13x |
| c99:debug     | dense_query_update |   100000 |  4096 | 389375.0 |    390716.6 | 395175.0 |  391410.4 |     3.907 |   255.9 |     7.57x |
| c99:debug     | command_burst      |        - |     - |    617.5 |       652.1 |    683.1 |     653.2 |         - |       - |     3.51x |
| c99:debug     | host_api_roundtrip |        - |     - |   1871.9 |      1886.7 |   1928.3 |    1893.8 |         - |       - |     1.53x |
| cpp17:release | noop               |        - |     - |     3.47 |        3.48 |     3.61 |      3.50 |         - |       - |     3.03x |
| cpp17:release | dense_query_update |     1000 |   256 |    342.5 |       346.7 |    397.5 |     351.8 |     0.347 |  2884.5 |     1.21x |
| cpp17:release | dense_query_update |     1000 |  4096 |    290.0 |       291.7 |    293.3 |     291.9 |     0.292 |  3428.4 |     0.99x |
| cpp17:release | dense_query_update |   100000 |   256 |  60650.0 |     60929.2 |  62233.4 |   61128.8 |     0.609 |  1641.2 |     1.08x |
| cpp17:release | dense_query_update |   100000 |  4096 |  51633.2 |     51725.0 |  53308.4 |   52013.2 |     0.517 |  1933.3 |     1.00x |
| cpp17:release | command_burst      |        - |     - |    608.1 |       659.0 |    687.2 |     658.7 |         - |       - |     3.55x |
| cpp17:release | host_api_roundtrip |        - |     - |   1824.2 |      1825.1 |   1871.1 |    1834.7 |         - |       - |     1.48x |
| cpp17:debug   | noop               |        - |     - |     6.32 |        7.05 |    15.74 |      8.77 |         - |       - |     6.13x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   3855.0 |      3885.8 |   3985.0 |    3906.4 |     3.886 |   257.3 |    13.60x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   3740.0 |      3763.7 |   3900.0 |    3775.8 |     3.764 |   265.7 |    12.76x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 404266.8 |    406041.6 | 408608.4 |  406284.7 |     4.060 |   246.3 |     7.21x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 390475.0 |    392091.7 | 394991.8 |  392332.6 |     3.921 |   255.0 |     7.60x |
| cpp17:debug   | command_burst      |        - |     - |   1136.7 |      1155.3 |   1198.3 |    1157.3 |         - |       - |     6.23x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2798.1 |      2810.1 |   2858.9 |    2817.2 |         - |       - |     2.28x |
| zig:release   | noop               |        - |     - |     3.91 |        3.92 |     4.20 |      3.96 |         - |       - |     3.41x |
| zig:release   | dense_query_update |     1000 |   256 |    517.5 |       520.8 |    540.8 |     522.3 |     0.521 |  1920.0 |     1.82x |
| zig:release   | dense_query_update |     1000 |  4096 |    418.3 |       419.2 |    424.2 |     421.0 |     0.419 |  2385.7 |     1.42x |
| zig:release   | dense_query_update |   100000 |   256 |  64291.6 |     64495.8 |  65916.6 |   64777.1 |     0.645 |  1550.5 |     1.14x |
| zig:release   | dense_query_update |   100000 |  4096 |  54100.0 |     54216.7 |  55758.4 |   54437.2 |     0.542 |  1844.5 |     1.05x |
| zig:release   | command_burst      |        - |     - |    679.4 |       740.0 |    781.4 |     738.6 |         - |       - |     3.99x |
| zig:release   | host_api_roundtrip |        - |     - |   1833.1 |      1839.2 |   1868.6 |    1845.3 |         - |       - |     1.49x |
| zig:debug     | noop               |        - |     - |     4.10 |        4.11 |     4.36 |      4.13 |         - |       - |     3.58x |
| zig:debug     | dense_query_update |     1000 |   256 |   4039.2 |      4217.9 |   4380.0 |    4222.3 |     4.218 |   237.1 |    14.76x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3657.5 |      3859.2 |   4109.2 |    3882.2 |     3.859 |   259.1 |    13.08x |
| zig:debug     | dense_query_update |   100000 |   256 | 384183.4 |    393108.4 | 413616.8 |  395419.7 |     3.931 |   254.4 |     6.98x |
| zig:debug     | dense_query_update |   100000 |  4096 | 384950.0 |    415654.1 | 426858.2 |  414892.5 |     4.157 |   240.6 |     8.05x |
| zig:debug     | command_burst      |        - |     - |   1626.1 |      1647.5 |   1691.7 |    1651.3 |         - |       - |     8.88x |
| zig:debug     | host_api_roundtrip |        - |     - |   3948.3 |      3981.7 |   4032.2 |    3984.5 |         - |       - |     3.23x |
<!-- BENCHMARKS_RESULTS_END -->
