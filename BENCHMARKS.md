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
- **Commit:** `d49ea8f`
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

| Workload                           | Native Median (ns) | Fastest Release Guest               | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | ----------------------------------- | -----------------------: |
| noop                               |               0.52 | `zig:release`: 3.34 ns (6.48x)      |         9.24 ns (17.92x) |
| dense_query_update (1000, c256)    |             1973.4 | `cpp17:release`: 254.4 ns (0.13x)   |        1988.4 ns (1.01x) |
| dense_query_update (1000, c4096)   |             1923.6 | `cpp17:release`: 191.2 ns (0.10x)   |        1959.3 ns (1.02x) |
| dense_query_update (100000, c256)  |           223960.1 | `c99:release`: 119339.8 ns (0.53x)  |      229524.5 ns (1.02x) |
| dense_query_update (100000, c4096) |           222876.3 | `cpp17:release`: 93060.7 ns (0.42x) |      221590.0 ns (0.99x) |
| command_burst                      |              181.9 | `c99:release`: 398.2 ns (2.19x)     |         415.2 ns (2.28x) |
| host_api_roundtrip                 |             1183.1 | `c99:release`: 1846.5 ns (1.56x)    |        2018.2 ns (1.71x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     0.52 |        0.52 |     0.52 |      0.52 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1922.7 |      1973.4 |   2194.8 |    1978.0 |     1.973 |   506.7 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1923.5 |      1923.6 |   2111.4 |    1948.7 |     1.924 |   519.8 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 221285.6 |    223960.1 | 228415.8 |  224180.3 |     2.240 |   446.5 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 218282.8 |    222876.3 | 230553.4 |  223385.7 |     2.229 |   448.7 |     1.00x |
| native        | command_burst      |        - |     - |    181.4 |       181.9 |    214.2 |     184.5 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |   1183.0 |      1183.1 |   1262.2 |    1200.7 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     9.23 |        9.24 |    10.31 |      9.42 |         - |       - |    17.92x |
| rust:release  | dense_query_update |     1000 |   256 |   1977.0 |      1988.4 |   2184.3 |    2013.1 |     1.988 |   502.9 |     1.01x |
| rust:release  | dense_query_update |     1000 |  4096 |   1954.7 |      1959.3 |   2186.3 |    1984.0 |     1.959 |   510.4 |     1.02x |
| rust:release  | dense_query_update |   100000 |   256 | 226785.8 |    229524.5 | 235926.8 |  230152.9 |     2.295 |   435.7 |     1.02x |
| rust:release  | dense_query_update |   100000 |  4096 | 218665.6 |    221590.0 | 227151.2 |  222280.0 |     2.216 |   451.3 |     0.99x |
| rust:release  | command_burst      |        - |     - |    385.0 |       415.2 |    471.1 |     410.5 |         - |       - |     2.28x |
| rust:release  | host_api_roundtrip |        - |     - |   1993.6 |      2018.2 |   2111.0 |    2034.0 |         - |       - |     1.71x |
| rust:debug    | noop               |        - |     - |    50.52 |       51.54 |    53.23 |     51.63 |         - |       - |    99.97x |
| rust:debug    | dense_query_update |     1000 |   256 |   4575.4 |      4692.2 |   5004.3 |    4726.3 |     4.692 |   213.1 |     2.38x |
| rust:debug    | dense_query_update |     1000 |  4096 |   3685.4 |      3846.3 |   4140.4 |    3874.2 |     3.846 |   260.0 |     2.00x |
| rust:debug    | dense_query_update |   100000 |   256 | 483524.0 |    490308.4 | 500462.2 |  490397.0 |     4.903 |   204.0 |     2.19x |
| rust:debug    | dense_query_update |   100000 |  4096 | 354554.2 |    362388.2 | 368939.2 |  362378.6 |     3.624 |   275.9 |     1.63x |
| rust:debug    | command_burst      |        - |     - |   1630.5 |      1667.1 |   1763.4 |    1680.6 |         - |       - |     9.17x |
| rust:debug    | host_api_roundtrip |        - |     - |   8410.7 |      8549.2 |   8768.2 |    8561.2 |         - |       - |     7.23x |
| c99:release   | noop               |        - |     - |     3.20 |        3.38 |     4.90 |      3.71 |         - |       - |     6.56x |
| c99:release   | dense_query_update |     1000 |   256 |    273.3 |       274.4 |    275.1 |     274.4 |     0.274 |  3645.0 |     0.14x |
| c99:release   | dense_query_update |     1000 |  4096 |    292.5 |       295.0 |    306.1 |     299.7 |     0.295 |  3390.1 |     0.15x |
| c99:release   | dense_query_update |   100000 |   256 | 114792.8 |    119339.8 | 121810.6 |  118246.2 |     1.193 |   837.9 |     0.53x |
| c99:release   | dense_query_update |   100000 |  4096 |  94080.0 |     96668.9 | 100843.6 |   96833.2 |     0.967 |  1034.5 |     0.43x |
| c99:release   | command_burst      |        - |     - |    381.2 |       398.2 |    438.9 |     404.0 |         - |       - |     2.19x |
| c99:release   | host_api_roundtrip |        - |     - |   1843.1 |      1846.5 |   1942.3 |    1868.4 |         - |       - |     1.56x |
| c99:debug     | noop               |        - |     - |     3.59 |        3.85 |     5.39 |      4.25 |         - |       - |     7.46x |
| c99:debug     | dense_query_update |     1000 |   256 |   3829.3 |      3890.8 |   4120.7 |    3920.2 |     3.891 |   257.0 |     1.97x |
| c99:debug     | dense_query_update |     1000 |  4096 |   3973.9 |      4017.6 |   4232.4 |    4056.1 |     4.018 |   248.9 |     2.09x |
| c99:debug     | dense_query_update |   100000 |   256 | 420724.6 |    424532.3 | 432140.4 |  426798.9 |     4.245 |   235.6 |     1.90x |
| c99:debug     | dense_query_update |   100000 |  4096 | 388444.0 |    394911.6 | 400596.8 |  394832.6 |     3.949 |   253.2 |     1.77x |
| c99:debug     | command_burst      |        - |     - |    419.6 |       448.2 |    489.6 |     449.7 |         - |       - |     2.46x |
| c99:debug     | host_api_roundtrip |        - |     - |   1927.7 |      1950.7 |   2051.7 |    1963.4 |         - |       - |     1.65x |
| cpp17:release | noop               |        - |     - |     3.19 |        3.34 |     4.41 |      3.56 |         - |       - |     6.48x |
| cpp17:release | dense_query_update |     1000 |   256 |    252.6 |       254.4 |    259.4 |     261.8 |     0.254 |  3930.2 |     0.13x |
| cpp17:release | dense_query_update |     1000 |  4096 |    189.4 |       191.2 |    215.6 |     195.7 |     0.191 |  5228.8 |     0.10x |
| cpp17:release | dense_query_update |   100000 |   256 | 115380.6 |    119797.8 | 122465.6 |  118737.0 |     1.198 |   834.7 |     0.53x |
| cpp17:release | dense_query_update |   100000 |  4096 |  92412.4 |     93060.7 |  98927.4 |   94939.9 |     0.931 |  1074.6 |     0.42x |
| cpp17:release | command_burst      |        - |     - |    382.6 |       415.5 |    457.1 |     416.9 |         - |       - |     2.28x |
| cpp17:release | host_api_roundtrip |        - |     - |   1860.8 |      1878.9 |   1976.3 |    1891.1 |         - |       - |     1.59x |
| cpp17:debug   | noop               |        - |     - |     3.59 |        3.59 |     4.36 |      3.68 |         - |       - |     6.97x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   3861.9 |      3914.5 |   4203.4 |    3981.5 |     3.914 |   255.5 |     1.98x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   3981.8 |      4010.2 |   4241.2 |    4059.3 |     4.010 |   249.4 |     2.08x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 423230.8 |    426620.8 | 432899.4 |  427103.0 |     4.266 |   234.4 |     1.90x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 393963.6 |    397257.3 | 404464.2 |  397639.7 |     3.973 |   251.7 |     1.78x |
| cpp17:debug   | command_burst      |        - |     - |    861.1 |       882.5 |    977.3 |     898.7 |         - |       - |     4.85x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2125.4 |      2160.3 |   2299.7 |    2172.0 |         - |       - |     1.83x |
| zig:release   | noop               |        - |     - |     3.33 |        3.34 |     4.47 |      3.65 |         - |       - |     6.48x |
| zig:release   | dense_query_update |     1000 |   256 |    370.5 |       374.0 |    467.9 |     384.6 |     0.374 |  2673.5 |     0.19x |
| zig:release   | dense_query_update |     1000 |  4096 |    317.6 |       319.2 |    328.1 |     323.6 |     0.319 |  3133.1 |     0.17x |
| zig:release   | dense_query_update |   100000 |   256 | 127849.2 |    133756.3 | 136820.8 |  133001.7 |     1.338 |   747.6 |     0.60x |
| zig:release   | dense_query_update |   100000 |  4096 |  97177.8 |    101946.0 | 104841.6 |  100972.4 |     1.019 |   980.9 |     0.46x |
| zig:release   | command_burst      |        - |     - |    456.2 |       492.5 |    540.3 |     491.7 |         - |       - |     2.71x |
| zig:release   | host_api_roundtrip |        - |     - |   1910.4 |      1928.1 |   2013.4 |    1939.5 |         - |       - |     1.63x |
| zig:debug     | noop               |        - |     - |     3.59 |        4.10 |     4.59 |      3.94 |         - |       - |     7.96x |
| zig:debug     | dense_query_update |     1000 |   256 |   2733.5 |      2747.3 |   2913.9 |    2781.5 |     2.747 |   364.0 |     1.39x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2656.8 |      2674.1 |   2889.2 |    2706.1 |     2.674 |   374.0 |     1.39x |
| zig:debug     | dense_query_update |   100000 |   256 | 300542.0 |    306018.1 | 323078.2 |  307693.4 |     3.060 |   326.8 |     1.37x |
| zig:debug     | dense_query_update |   100000 |  4096 | 277437.2 |    283304.9 | 287626.2 |  282747.9 |     2.833 |   353.0 |     1.27x |
| zig:debug     | command_burst      |        - |     - |   1389.6 |      1427.2 |   1513.2 |    1436.8 |         - |       - |     7.85x |
| zig:debug     | host_api_roundtrip |        - |     - |   9561.1 |      9721.3 |   9874.4 |    9721.1 |         - |       - |     8.22x |
<!-- BENCHMARKS_RESULTS_END -->
