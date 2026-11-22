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
- **Commit:** `0e8a9f8`
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
| noop                               |               1.63 | `cpp17:release`: 5.14 ns (3.16x)    |          6.62 ns (4.07x) |
| dense_query_update (1000, c256)    |             1328.8 | `c99:release`: 338.5 ns (0.25x)     |        1445.1 ns (1.09x) |
| dense_query_update (1000, c4096)   |             1296.0 | `cpp17:release`: 251.2 ns (0.19x)   |        1363.5 ns (1.05x) |
| dense_query_update (100000, c256)  |           140753.7 | `cpp17:release`: 49486.0 ns (0.35x) |      151843.1 ns (1.08x) |
| dense_query_update (100000, c4096) |           131154.6 | `cpp17:release`: 36355.7 ns (0.28x) |      132527.6 ns (1.01x) |
| command_burst                      |              200.9 | `c99:release`: 538.8 ns (2.68x)     |         558.2 ns (2.78x) |
| host_api_roundtrip                 |              665.8 | `cpp17:release`: 1241.7 ns (1.86x)  |        1468.8 ns (2.21x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.36 |        1.63 |     1.63 |      1.65 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1321.4 |      1328.8 |   1590.2 |    1346.1 |     1.329 |   752.6 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1292.5 |      1296.0 |   1475.2 |    1312.0 |     1.296 |   771.6 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 138502.4 |    140753.7 | 143738.2 |  140968.0 |     1.408 |   710.5 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 129150.6 |    131154.6 | 132249.2 |  130798.4 |     1.312 |   762.5 |     1.00x |
| native        | command_burst      |        - |     - |    199.0 |       200.9 |    201.8 |     201.6 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    664.7 |       665.8 |    709.1 |     670.2 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     6.05 |        6.62 |     7.60 |      6.68 |         - |       - |     4.07x |
| rust:release  | dense_query_update |     1000 |   256 |   1441.1 |      1445.1 |   1580.1 |    1455.5 |     1.445 |   692.0 |     1.09x |
| rust:release  | dense_query_update |     1000 |  4096 |   1360.8 |      1363.5 |   1509.4 |    1374.2 |     1.364 |   733.4 |     1.05x |
| rust:release  | dense_query_update |   100000 |   256 | 147644.0 |    151843.1 | 155902.0 |  152106.3 |     1.518 |   658.6 |     1.08x |
| rust:release  | dense_query_update |   100000 |  4096 | 130805.2 |    132527.6 | 133837.6 |  135257.3 |     1.325 |   754.6 |     1.01x |
| rust:release  | command_burst      |        - |     - |    557.3 |       558.2 |    609.0 |     563.8 |         - |       - |     2.78x |
| rust:release  | host_api_roundtrip |        - |     - |   1464.2 |      1468.8 |   1511.9 |    1477.8 |         - |       - |     2.21x |
| rust:debug    | noop               |        - |     - |    66.05 |       68.01 |    69.06 |     67.97 |         - |       - |    41.84x |
| rust:debug    | dense_query_update |     1000 |   256 |   8009.0 |      8049.5 |   8332.5 |    8114.1 |     8.049 |   124.2 |     6.06x |
| rust:debug    | dense_query_update |     1000 |  4096 |   6965.7 |      6991.1 |   7188.0 |    7066.6 |     6.991 |   143.0 |     5.39x |
| rust:debug    | dense_query_update |   100000 |   256 | 780067.2 |    782999.6 | 797917.6 |  788142.3 |     7.830 |   127.7 |     5.56x |
| rust:debug    | dense_query_update |   100000 |  4096 | 643553.8 |    645237.4 | 659311.2 |  646862.4 |     6.452 |   155.0 |     4.92x |
| rust:debug    | command_burst      |        - |     - |   2153.6 |      2161.8 |   2222.1 |    2177.0 |         - |       - |    10.76x |
| rust:debug    | host_api_roundtrip |        - |     - |  11721.6 |     12275.0 |  12854.8 |   12372.2 |         - |       - |    18.44x |
| c99:release   | noop               |        - |     - |     5.10 |        5.15 |     5.81 |      5.21 |         - |       - |     3.17x |
| c99:release   | dense_query_update |     1000 |   256 |    336.1 |       338.5 |    341.3 |     341.6 |     0.339 |  2954.2 |     0.25x |
| c99:release   | dense_query_update |     1000 |  4096 |    282.8 |       285.8 |    291.2 |     288.5 |     0.286 |  3498.7 |     0.22x |
| c99:release   | dense_query_update |   100000 |   256 |  49483.0 |     49993.8 |  51844.6 |   50456.2 |     0.500 |  2000.2 |     0.36x |
| c99:release   | dense_query_update |   100000 |  4096 |  35430.4 |     36823.4 |  40265.4 |   37254.5 |     0.368 |  2715.7 |     0.28x |
| c99:release   | command_burst      |        - |     - |    537.9 |       538.8 |    610.4 |     545.7 |         - |       - |     2.68x |
| c99:release   | host_api_roundtrip |        - |     - |   1266.7 |      1270.0 |   1384.1 |    1286.3 |         - |       - |     1.91x |
| c99:debug     | noop               |        - |     - |     6.17 |        6.44 |     7.30 |      6.47 |         - |       - |     3.96x |
| c99:debug     | dense_query_update |     1000 |   256 |   4981.4 |      4989.6 |   5134.0 |    5028.2 |     4.990 |   200.4 |     3.76x |
| c99:debug     | dense_query_update |     1000 |  4096 |   4905.0 |      4910.0 |   5107.1 |    4976.5 |     4.910 |   203.7 |     3.79x |
| c99:debug     | dense_query_update |   100000 |   256 | 507887.6 |    510335.2 | 538679.0 |  512844.6 |     5.103 |   195.9 |     3.63x |
| c99:debug     | dense_query_update |   100000 |  4096 | 495021.0 |    497181.0 | 508023.8 |  498318.1 |     4.972 |   201.1 |     3.79x |
| c99:debug     | command_burst      |        - |     - |    603.0 |       604.2 |    649.3 |     609.0 |         - |       - |     3.01x |
| c99:debug     | host_api_roundtrip |        - |     - |   1377.5 |      1383.3 |   1429.4 |    1392.1 |         - |       - |     2.08x |
| cpp17:release | noop               |        - |     - |     5.06 |        5.14 |     5.87 |      5.19 |         - |       - |     3.16x |
| cpp17:release | dense_query_update |     1000 |   256 |    346.3 |       350.5 |    373.2 |     354.2 |     0.351 |  2852.9 |     0.26x |
| cpp17:release | dense_query_update |     1000 |  4096 |    246.8 |       251.2 |    254.0 |     250.9 |     0.251 |  3981.5 |     0.19x |
| cpp17:release | dense_query_update |   100000 |   256 |  49164.6 |     49486.0 |  51339.8 |   49959.4 |     0.495 |  2020.8 |     0.35x |
| cpp17:release | dense_query_update |   100000 |  4096 |  35436.4 |     36355.7 |  39906.8 |   36711.5 |     0.364 |  2750.6 |     0.28x |
| cpp17:release | command_burst      |        - |     - |    575.4 |       576.1 |    624.9 |     580.5 |         - |       - |     2.87x |
| cpp17:release | host_api_roundtrip |        - |     - |   1238.5 |      1241.7 |   1289.3 |    1251.3 |         - |       - |     1.86x |
| cpp17:debug   | noop               |        - |     - |     6.39 |        6.45 |     7.12 |      6.50 |         - |       - |     3.97x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   5261.8 |      5271.7 |   5431.2 |    5309.3 |     5.272 |   189.7 |     3.97x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   5090.7 |      5099.0 |   5429.6 |    5168.9 |     5.099 |   196.1 |     3.93x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 523028.4 |    526302.0 | 537239.2 |  527720.0 |     5.263 |   190.0 |     3.74x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 509163.8 |    511179.7 | 518315.2 |  511501.8 |     5.112 |   195.6 |     3.90x |
| cpp17:debug   | command_burst      |        - |     - |   1214.1 |      1224.7 |   1320.6 |    1241.4 |         - |       - |     6.10x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1856.2 |      1865.5 |   1933.9 |    1879.2 |         - |       - |     2.80x |
| zig:release   | noop               |        - |     - |     5.91 |        5.93 |     6.86 |      6.02 |         - |       - |     3.65x |
| zig:release   | dense_query_update |     1000 |   256 |    414.6 |       422.7 |    432.6 |     428.3 |     0.423 |  2365.6 |     0.32x |
| zig:release   | dense_query_update |     1000 |  4096 |    324.7 |       325.5 |    326.5 |     328.1 |     0.325 |  3072.4 |     0.25x |
| zig:release   | dense_query_update |   100000 |   256 |  57931.6 |     58469.3 |  61210.4 |   59056.6 |     0.585 |  1710.3 |     0.42x |
| zig:release   | dense_query_update |   100000 |  4096 |  40457.8 |     40708.2 |  43147.8 |   41214.9 |     0.407 |  2456.5 |     0.31x |
| zig:release   | command_burst      |        - |     - |    626.6 |       627.3 |    682.9 |     632.8 |         - |       - |     3.12x |
| zig:release   | host_api_roundtrip |        - |     - |   1322.0 |      1322.5 |   1368.6 |    1332.2 |         - |       - |     1.99x |
| zig:debug     | noop               |        - |     - |     6.38 |        6.42 |     7.08 |      6.47 |         - |       - |     3.95x |
| zig:debug     | dense_query_update |     1000 |   256 |   3179.5 |      3189.7 |   3328.9 |    3209.9 |     3.190 |   313.5 |     2.40x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3076.3 |      3082.4 |   3238.8 |    3106.3 |     3.082 |   324.4 |     2.38x |
| zig:debug     | dense_query_update |   100000 |   256 | 317619.0 |    319903.4 | 325422.6 |  320024.1 |     3.199 |   312.6 |     2.27x |
| zig:debug     | dense_query_update |   100000 |  4096 | 304880.2 |    306675.8 | 313649.2 |  307096.9 |     3.067 |   326.1 |     2.34x |
| zig:debug     | command_burst      |        - |     - |   1993.3 |      2042.8 |   2101.8 |    2053.7 |         - |       - |    10.17x |
| zig:debug     | host_api_roundtrip |        - |     - |   1676.7 |      1680.6 |   1750.4 |    1698.2 |         - |       - |     2.52x |
<!-- BENCHMARKS_RESULTS_END -->
