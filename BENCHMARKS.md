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
- **Commit:** `9a8c7e9`
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
| noop                               |               0.52 | `zig:release`: 2.05 ns (3.98x)       |         7.18 ns (13.93x) |
| dense_query_update (1000, c256)    |             1923.0 | `cpp17:release`: 243.9 ns (0.13x)    |        1984.6 ns (1.03x) |
| dense_query_update (1000, c4096)   |             1923.5 | `cpp17:release`: 188.3 ns (0.10x)    |        1953.9 ns (1.02x) |
| dense_query_update (100000, c256)  |           216339.6 | `cpp17:release`: 114419.5 ns (0.53x) |      221036.9 ns (1.02x) |
| dense_query_update (100000, c4096) |           220303.3 | `cpp17:release`: 93856.3 ns (0.43x)  |      224066.3 ns (1.02x) |
| command_burst                      |              183.1 | `c99:release`: 365.8 ns (2.00x)      |         394.3 ns (2.15x) |
| host_api_roundtrip                 |             1183.3 | `c99:release`: 1842.1 ns (1.56x)     |        2035.2 ns (1.72x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     0.52 |        0.52 |     0.52 |      0.52 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1922.9 |      1923.0 |   2136.6 |    1952.9 |     1.923 |   520.0 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1923.4 |      1923.5 |   2112.3 |    1946.1 |     1.924 |   519.9 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 214178.8 |    216339.6 | 220079.0 |  216628.8 |     2.163 |   462.2 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 218891.8 |    220303.3 | 224727.8 |  220993.6 |     2.203 |   453.9 |     1.00x |
| native        | command_burst      |        - |     - |    182.2 |       183.1 |    227.1 |     186.6 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |   1183.1 |      1183.3 |   1249.8 |    1197.9 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     7.18 |        7.18 |     7.94 |      7.25 |         - |       - |    13.93x |
| rust:release  | dense_query_update |     1000 |   256 |   1968.0 |      1984.6 |   2150.9 |    2001.9 |     1.985 |   503.9 |     1.03x |
| rust:release  | dense_query_update |     1000 |  4096 |   1950.5 |      1953.9 |   2101.2 |    1974.1 |     1.954 |   511.8 |     1.02x |
| rust:release  | dense_query_update |   100000 |   256 | 219180.6 |    221036.9 | 225551.8 |  221740.4 |     2.210 |   452.4 |     1.02x |
| rust:release  | dense_query_update |   100000 |  4096 | 222029.8 |    224066.3 | 251792.0 |  226455.4 |     2.241 |   446.3 |     1.02x |
| rust:release  | command_burst      |        - |     - |    380.8 |       394.3 |    447.2 |     397.8 |         - |       - |     2.15x |
| rust:release  | host_api_roundtrip |        - |     - |   2012.3 |      2035.2 |   2096.5 |    2046.4 |         - |       - |     1.72x |
| rust:debug    | noop               |        - |     - |    51.01 |       52.00 |    53.01 |     52.09 |         - |       - |   100.89x |
| rust:debug    | dense_query_update |     1000 |   256 |   4946.3 |      5021.7 |   5255.7 |    5060.2 |     5.022 |   199.1 |     2.61x |
| rust:debug    | dense_query_update |     1000 |  4096 |   4057.6 |      4102.5 |   4346.2 |    4149.4 |     4.103 |   243.8 |     2.13x |
| rust:debug    | dense_query_update |   100000 |   256 | 519481.0 |    525920.9 | 530992.0 |  526097.3 |     5.259 |   190.1 |     2.43x |
| rust:debug    | dense_query_update |   100000 |  4096 | 404851.8 |    422979.4 | 430892.4 |  423101.5 |     4.230 |   236.4 |     1.92x |
| rust:debug    | command_burst      |        - |     - |   1419.6 |      1454.4 |   1532.6 |    1467.8 |         - |       - |     7.94x |
| rust:debug    | host_api_roundtrip |        - |     - |   8687.1 |      8772.1 |   8898.7 |    8776.7 |         - |       - |     7.41x |
| c99:release   | noop               |        - |     - |     2.05 |        2.05 |     2.31 |      2.11 |         - |       - |     3.98x |
| c99:release   | dense_query_update |     1000 |   256 |    288.5 |       290.9 |    295.1 |     293.9 |     0.291 |  3437.4 |     0.15x |
| c99:release   | dense_query_update |     1000 |  4096 |    301.1 |       302.3 |    303.3 |     305.0 |     0.302 |  3308.2 |     0.16x |
| c99:release   | dense_query_update |   100000 |   256 | 111652.4 |    114572.7 | 116955.8 |  113962.9 |     1.146 |   872.8 |     0.53x |
| c99:release   | dense_query_update |   100000 |  4096 |  95540.8 |     97849.8 | 100477.0 |   97568.4 |     0.978 |  1022.0 |     0.44x |
| c99:release   | command_burst      |        - |     - |    364.2 |       365.8 |    427.9 |     373.5 |         - |       - |     2.00x |
| c99:release   | host_api_roundtrip |        - |     - |   1841.8 |      1842.1 |   1912.3 |    1863.0 |         - |       - |     1.56x |
| c99:debug     | noop               |        - |     - |     2.31 |        2.31 |     2.74 |      2.41 |         - |       - |     4.48x |
| c99:debug     | dense_query_update |     1000 |   256 |   3778.3 |      3879.9 |   4064.4 |    3909.0 |     3.880 |   257.7 |     2.02x |
| c99:debug     | dense_query_update |     1000 |  4096 |   3709.8 |      3742.9 |   3932.7 |    3781.9 |     3.743 |   267.2 |     1.95x |
| c99:debug     | dense_query_update |   100000 |   256 | 411761.2 |    415441.9 | 420498.6 |  415247.1 |     4.154 |   240.7 |     1.92x |
| c99:debug     | dense_query_update |   100000 |  4096 | 384995.0 |    389340.3 | 392110.4 |  390248.7 |     3.893 |   256.8 |     1.77x |
| c99:debug     | command_burst      |        - |     - |    416.6 |       422.2 |    482.3 |     428.6 |         - |       - |     2.31x |
| c99:debug     | host_api_roundtrip |        - |     - |   1909.8 |      1941.7 |   2005.6 |    1955.5 |         - |       - |     1.64x |
| cpp17:release | noop               |        - |     - |     2.05 |        2.06 |     2.79 |      2.15 |         - |       - |     3.99x |
| cpp17:release | dense_query_update |     1000 |   256 |    243.2 |       243.9 |    246.8 |     247.7 |     0.244 |  4100.0 |     0.13x |
| cpp17:release | dense_query_update |     1000 |  4096 |    187.3 |       188.3 |    189.1 |     188.3 |     0.188 |  5311.8 |     0.10x |
| cpp17:release | dense_query_update |   100000 |   256 | 111419.8 |    114419.5 | 116830.8 |  113824.0 |     1.144 |   874.0 |     0.53x |
| cpp17:release | dense_query_update |   100000 |  4096 |  92036.6 |     93856.3 |  97413.6 |   94064.1 |     0.939 |  1065.5 |     0.43x |
| cpp17:release | command_burst      |        - |     - |    370.9 |       376.3 |    436.4 |     378.5 |         - |       - |     2.06x |
| cpp17:release | host_api_roundtrip |        - |     - |   1858.7 |      1859.3 |   1950.0 |    1881.2 |         - |       - |     1.57x |
| cpp17:debug   | noop               |        - |     - |     2.05 |        2.05 |     2.07 |      2.07 |         - |       - |     3.98x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   3855.6 |      3905.3 |   4085.4 |    3942.2 |     3.905 |   256.1 |     2.03x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   3784.2 |      3803.1 |   3985.1 |    3844.7 |     3.803 |   262.9 |     1.98x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 415617.2 |    419381.2 | 423864.8 |  419414.1 |     4.194 |   238.4 |     1.94x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 387557.0 |    392680.5 | 399882.2 |  392708.5 |     3.927 |   254.7 |     1.78x |
| cpp17:debug   | command_burst      |        - |     - |    852.2 |       875.0 |    962.8 |     889.6 |         - |       - |     4.78x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2122.9 |      2136.0 |   2200.1 |    2148.7 |         - |       - |     1.81x |
| zig:release   | noop               |        - |     - |     2.05 |        2.05 |     2.31 |      2.13 |         - |       - |     3.98x |
| zig:release   | dense_query_update |     1000 |   256 |    366.9 |       369.2 |    385.9 |     373.6 |     0.369 |  2708.9 |     0.19x |
| zig:release   | dense_query_update |     1000 |  4096 |    315.3 |       315.8 |    395.1 |     324.1 |     0.316 |  3166.8 |     0.16x |
| zig:release   | dense_query_update |   100000 |   256 | 112022.8 |    115024.2 | 117522.4 |  114432.5 |     1.150 |   869.4 |     0.53x |
| zig:release   | dense_query_update |   100000 |  4096 |  94417.6 |     96442.3 |  99465.2 |   96440.1 |     0.964 |  1036.9 |     0.44x |
| zig:release   | command_burst      |        - |     - |    428.6 |       446.1 |    502.0 |     455.5 |         - |       - |     2.44x |
| zig:release   | host_api_roundtrip |        - |     - |   1891.9 |      1892.6 |   1963.8 |    1913.4 |         - |       - |     1.60x |
| zig:debug     | noop               |        - |     - |     2.05 |        2.05 |     2.32 |      2.09 |         - |       - |     3.99x |
| zig:debug     | dense_query_update |     1000 |   256 |   2739.0 |      2756.6 |   2930.2 |    2792.1 |     2.757 |   362.8 |     1.43x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2660.9 |      2667.6 |   2822.0 |    2696.6 |     2.668 |   374.9 |     1.39x |
| zig:debug     | dense_query_update |   100000 |   256 | 290381.2 |    294016.5 | 298701.6 |  294064.1 |     2.940 |   340.1 |     1.36x |
| zig:debug     | dense_query_update |   100000 |  4096 | 270480.4 |    274745.0 | 278848.8 |  274901.4 |     2.747 |   364.0 |     1.25x |
| zig:debug     | command_burst      |        - |     - |   1390.8 |      1415.0 |   1483.1 |    1462.4 |         - |       - |     7.73x |
| zig:debug     | host_api_roundtrip |        - |     - |   9608.8 |      9699.3 |   9780.8 |    9697.4 |         - |       - |     8.20x |
<!-- BENCHMARKS_RESULTS_END -->
