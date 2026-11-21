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
- **Commit:** `d151170`
- **Host target:** `linux/x86_64` (`bamabi_bench` built with `--release`)
- **Rust:** `rustc 1.100.0 (0ef1bc386 2026-11-09)`
- **C99 (`cc`):** `cc (Ubuntu 15.2.0-16ubuntu1) 15.2.0`
- **C++17 (`c++`):** `c++ (Ubuntu 15.2.0-16ubuntu1) 15.2.0`
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

| Workload                           | Native Median (ns) | Fastest Release Guest                | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | ------------------------------------ | -----------------------: |
| noop                               |               0.48 | `cpp17:release`: 5.25 ns (10.96x)    |        11.46 ns (23.91x) |
| dense_query_update (1000, c256)    |             1800.6 | `c99:release`: 269.2 ns (0.15x)      |        1898.0 ns (1.05x) |
| dense_query_update (1000, c4096)   |             1790.7 | `cpp17:release`: 180.7 ns (0.10x)    |        1881.3 ns (1.05x) |
| dense_query_update (100000, c256)  |           204490.0 | `cpp17:release`: 112999.1 ns (0.55x) |      204732.6 ns (1.00x) |
| dense_query_update (100000, c4096) |           202339.8 | `zig:release`: 94237.6 ns (0.47x)    |      207784.0 ns (1.03x) |
| command_burst                      |              158.2 | `cpp17:release`: 381.0 ns (2.41x)    |         552.8 ns (3.49x) |
| host_api_roundtrip                 |             1423.3 | `c99:release`: 2029.0 ns (1.43x)     |        2220.5 ns (1.56x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     0.48 |        0.48 |     0.48 |      0.48 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1799.9 |      1800.6 |   1954.0 |    1819.8 |     1.801 |   555.4 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1790.3 |      1790.7 |   1962.8 |    1808.9 |     1.791 |   558.4 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 204179.2 |    204490.0 | 206803.0 |  204757.7 |     2.045 |   489.0 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 200780.0 |    202339.8 | 204508.2 |  202523.4 |     2.023 |   494.2 |     1.00x |
| native        | command_burst      |        - |     - |    157.7 |       158.2 |    158.7 |     159.7 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |   1422.0 |      1423.3 |   1463.3 |    1433.3 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |    11.45 |       11.46 |    12.26 |     11.55 |         - |       - |    23.91x |
| rust:release  | dense_query_update |     1000 |   256 |   1893.5 |      1898.0 |   2079.7 |    1921.8 |     1.898 |   526.9 |     1.05x |
| rust:release  | dense_query_update |     1000 |  4096 |   1877.8 |      1881.3 |   2025.2 |    1901.2 |     1.881 |   531.5 |     1.05x |
| rust:release  | dense_query_update |   100000 |   256 | 203044.6 |    204732.6 | 207053.4 |  204786.4 |     2.047 |   488.4 |     1.00x |
| rust:release  | dense_query_update |   100000 |  4096 | 206711.4 |    207784.0 | 210013.0 |  208064.6 |     2.078 |   481.3 |     1.03x |
| rust:release  | command_burst      |        - |     - |    541.9 |       552.8 |    706.4 |     575.7 |         - |       - |     3.49x |
| rust:release  | host_api_roundtrip |        - |     - |   2217.9 |      2220.5 |   2276.3 |    2236.2 |         - |       - |     1.56x |
| rust:debug    | noop               |        - |     - |    226.4 |       226.9 |    229.6 |     227.7 |         - |       - |   473.39x |
| rust:debug    | dense_query_update |     1000 |   256 |   5196.8 |      5259.5 |   5476.4 |    5297.3 |     5.260 |   190.1 |     2.92x |
| rust:debug    | dense_query_update |     1000 |  4096 |   4286.1 |      4506.2 |   5922.3 |    4704.9 |     4.506 |   221.9 |     2.52x |
| rust:debug    | dense_query_update |   100000 |   256 | 474317.6 |    478461.3 | 488933.2 |  479546.5 |     4.785 |   209.0 |     2.34x |
| rust:debug    | dense_query_update |   100000 |  4096 | 343777.2 |    348509.3 | 352540.6 |  348735.4 |     3.485 |   286.9 |     1.72x |
| rust:debug    | command_burst      |        - |     - |   1916.8 |      1934.1 |   1992.9 |    1947.6 |         - |       - |    12.23x |
| rust:debug    | host_api_roundtrip |        - |     - |   9829.0 |      9889.5 |   9937.6 |    9892.0 |         - |       - |     6.95x |
| c99:release   | noop               |        - |     - |     5.25 |        5.25 |     5.80 |      5.30 |         - |       - |    10.96x |
| c99:release   | dense_query_update |     1000 |   256 |    268.2 |       269.2 |    270.9 |     271.8 |     0.269 |  3715.3 |     0.15x |
| c99:release   | dense_query_update |     1000 |  4096 |    201.7 |       202.3 |    203.1 |     204.6 |     0.202 |  4942.7 |     0.11x |
| c99:release   | dense_query_update |   100000 |   256 | 112196.0 |    114675.0 | 115936.4 |  113974.3 |     1.147 |   872.0 |     0.56x |
| c99:release   | dense_query_update |   100000 |  4096 |  94947.6 |     97544.0 | 101208.0 |   97292.6 |     0.975 |  1025.2 |     0.48x |
| c99:release   | command_burst      |        - |     - |    367.1 |       389.1 |    629.1 |     406.4 |         - |       - |     2.46x |
| c99:release   | host_api_roundtrip |        - |     - |   2025.6 |      2029.0 |   2107.2 |    2049.8 |         - |       - |     1.43x |
| c99:debug     | noop               |        - |     - |     5.49 |        5.49 |     6.13 |      5.53 |         - |       - |    11.46x |
| c99:debug     | dense_query_update |     1000 |   256 |   3536.7 |      3597.9 |   3766.8 |    3620.1 |     3.598 |   277.9 |     2.00x |
| c99:debug     | dense_query_update |     1000 |  4096 |   3468.0 |      3480.3 |   3698.3 |    3515.6 |     3.480 |   287.3 |     1.94x |
| c99:debug     | dense_query_update |   100000 |   256 | 380416.6 |    385043.5 | 389031.0 |  385405.4 |     3.850 |   259.7 |     1.88x |
| c99:debug     | dense_query_update |   100000 |  4096 | 357439.6 |    360764.9 | 362269.6 |  360505.0 |     3.608 |   277.2 |     1.78x |
| c99:debug     | command_burst      |        - |     - |    439.1 |       445.0 |    518.6 |     459.2 |         - |       - |     2.81x |
| c99:debug     | host_api_roundtrip |        - |     - |   2107.3 |      2115.3 |   2195.8 |    2130.3 |         - |       - |     1.49x |
| cpp17:release | noop               |        - |     - |     5.25 |        5.25 |     5.78 |      5.31 |         - |       - |    10.96x |
| cpp17:release | dense_query_update |     1000 |   256 |    276.2 |       277.3 |    278.4 |     277.3 |     0.277 |  3606.3 |     0.15x |
| cpp17:release | dense_query_update |     1000 |  4096 |    179.7 |       180.7 |    181.9 |     183.2 |     0.181 |  5535.3 |     0.10x |
| cpp17:release | dense_query_update |   100000 |   256 | 110356.8 |    112999.1 | 114354.6 |  112155.4 |     1.130 |   885.0 |     0.55x |
| cpp17:release | dense_query_update |   100000 |  4096 |  95203.0 |     97753.2 | 103291.8 |   97840.4 |     0.978 |  1023.0 |     0.48x |
| cpp17:release | command_burst      |        - |     - |    370.9 |       381.0 |    434.2 |     383.6 |         - |       - |     2.41x |
| cpp17:release | host_api_roundtrip |        - |     - |   2055.8 |      2068.9 |   2117.4 |    2078.4 |         - |       - |     1.45x |
| cpp17:debug   | noop               |        - |     - |     5.49 |        5.49 |     6.10 |      5.55 |         - |       - |    11.46x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   3616.7 |      3763.6 |   3908.3 |    3752.4 |     3.764 |   265.7 |     2.09x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   3517.5 |      3655.3 |   3762.6 |    3632.6 |     3.655 |   273.6 |     2.04x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 387054.4 |    390139.5 | 394670.2 |  390484.0 |     3.901 |   256.3 |     1.91x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 360573.2 |    363972.8 | 371802.6 |  365335.0 |     3.640 |   274.7 |     1.80x |
| cpp17:debug   | command_burst      |        - |     - |    833.7 |       856.0 |    914.1 |     863.6 |         - |       - |     5.41x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2519.9 |      2525.7 |   2587.5 |    2542.8 |         - |       - |     1.77x |
| zig:release   | noop               |        - |     - |     5.25 |        5.25 |     5.87 |      5.30 |         - |       - |    10.96x |
| zig:release   | dense_query_update |     1000 |   256 |    343.6 |       345.6 |    348.1 |     351.4 |     0.346 |  2893.1 |     0.19x |
| zig:release   | dense_query_update |     1000 |  4096 |    296.1 |       300.2 |    304.7 |     302.7 |     0.300 |  3331.6 |     0.17x |
| zig:release   | dense_query_update |   100000 |   256 | 112714.4 |    115023.2 | 115943.4 |  114438.7 |     1.150 |   869.4 |     0.56x |
| zig:release   | dense_query_update |   100000 |  4096 |  93678.8 |     94237.6 |  97667.4 |   95372.4 |     0.942 |  1061.1 |     0.47x |
| zig:release   | command_burst      |        - |     - |    385.3 |       399.2 |    446.3 |     410.7 |         - |       - |     2.52x |
| zig:release   | host_api_roundtrip |        - |     - |   2114.7 |      2123.5 |   2217.1 |    2141.0 |         - |       - |     1.49x |
| zig:debug     | noop               |        - |     - |     5.49 |        5.49 |     6.02 |      5.52 |         - |       - |    11.46x |
| zig:debug     | dense_query_update |     1000 |   256 |   2799.1 |      2850.8 |   3036.1 |    2874.8 |     2.851 |   350.8 |     1.58x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2719.3 |      2729.0 |   2876.7 |    2750.7 |     2.729 |   366.4 |     1.52x |
| zig:debug     | dense_query_update |   100000 |   256 | 288528.6 |    291363.1 | 294212.8 |  291699.6 |     2.914 |   343.2 |     1.42x |
| zig:debug     | dense_query_update |   100000 |  4096 | 256116.4 |    263986.7 | 271661.2 |  264787.5 |     2.640 |   378.8 |     1.30x |
| zig:debug     | command_burst      |        - |     - |  14202.6 |     14246.2 |  14327.0 |   14254.7 |         - |       - |    90.05x |
| zig:debug     | host_api_roundtrip |        - |     - |  18255.9 |     18342.1 |  18454.3 |   18353.6 |         - |       - |    12.89x |
<!-- BENCHMARKS_RESULTS_END -->
