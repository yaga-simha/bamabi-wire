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
- **Commit:** `9a0cea8`
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

| Workload                           | Native Median (ns) | Fastest Release Guest              | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | ---------------------------------- | -----------------------: |
| noop                               |               1.85 | `c99:release`: 6.17 ns (3.33x)     |          7.40 ns (4.00x) |
| dense_query_update (1000, c256)    |             1697.2 | `cpp17:release`: 388.9 ns (0.23x)  |        1825.6 ns (1.08x) |
| dense_query_update (1000, c4096)   |             1656.6 | `cpp17:release`: 283.9 ns (0.17x)  |        1724.0 ns (1.04x) |
| dense_query_update (100000, c256)  |           181792.5 | `c99:release`: 74413.1 ns (0.41x)  |      193213.7 ns (1.06x) |
| dense_query_update (100000, c4096) |           168948.7 | `c99:release`: 43193.2 ns (0.26x)  |      169858.3 ns (1.01x) |
| command_burst                      |              251.1 | `rust:release`: 704.9 ns (2.81x)   |         704.9 ns (2.81x) |
| host_api_roundtrip                 |              873.0 | `cpp17:release`: 1635.9 ns (1.87x) |        1947.8 ns (2.23x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.85 |        1.85 |     1.93 |      1.89 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1690.3 |      1697.2 |   2001.5 |    1723.4 |     1.697 |   589.2 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1653.7 |      1656.6 |   1808.3 |    1675.7 |     1.657 |   603.7 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 179332.8 |    181792.5 | 203742.4 |  185443.8 |     1.818 |   550.1 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 166986.0 |    168948.7 | 175165.2 |  169462.6 |     1.689 |   591.9 |     1.00x |
| native        | command_burst      |        - |     - |    246.4 |       251.1 |    309.4 |     255.2 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    866.1 |       873.0 |   1041.0 |     892.3 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     7.40 |        7.40 |     8.53 |      7.59 |         - |       - |     4.00x |
| rust:release  | dense_query_update |     1000 |   256 |   1821.4 |      1825.6 |   1969.5 |    1840.5 |     1.826 |   547.8 |     1.08x |
| rust:release  | dense_query_update |     1000 |  4096 |   1720.8 |      1724.0 |   1880.1 |    1746.6 |     1.724 |   580.0 |     1.04x |
| rust:release  | dense_query_update |   100000 |   256 | 190848.2 |    193213.7 | 205497.4 |  194931.9 |     1.932 |   517.6 |     1.06x |
| rust:release  | dense_query_update |   100000 |  4096 | 167649.2 |    169858.3 | 173908.8 |  170400.7 |     1.699 |   588.7 |     1.01x |
| rust:release  | command_burst      |        - |     - |    703.0 |       704.9 |    761.3 |     711.6 |         - |       - |     2.81x |
| rust:release  | host_api_roundtrip |        - |     - |   1928.3 |      1947.8 |   2085.1 |    1966.5 |         - |       - |     2.23x |
| rust:debug    | noop               |        - |     - |    86.37 |       87.54 |    89.38 |     87.70 |         - |       - |    47.28x |
| rust:debug    | dense_query_update |     1000 |   256 |   9670.7 |      9783.6 |   9872.7 |    9767.6 |     9.784 |   102.2 |     5.76x |
| rust:debug    | dense_query_update |     1000 |  4096 |   8278.4 |      8298.2 |   8479.2 |    8359.8 |     8.298 |   120.5 |     5.01x |
| rust:debug    | dense_query_update |   100000 |   256 | 941065.0 |    946507.1 | 965462.4 |  949508.7 |     9.465 |   105.7 |     5.21x |
| rust:debug    | dense_query_update |   100000 |  4096 | 765683.4 |    769529.5 | 784143.8 |  771320.0 |     7.695 |   129.9 |     4.55x |
| rust:debug    | command_burst      |        - |     - |   2674.8 |      2682.3 |   2829.1 |    2718.1 |         - |       - |    10.68x |
| rust:debug    | host_api_roundtrip |        - |     - |  13807.8 |     13858.9 |  14144.1 |   13901.9 |         - |       - |    15.88x |
| c99:release   | noop               |        - |     - |     6.17 |        6.17 |     7.04 |      6.26 |         - |       - |     3.33x |
| c99:release   | dense_query_update |     1000 |   256 |    425.6 |       435.5 |    444.8 |     437.7 |     0.435 |  2296.2 |     0.26x |
| c99:release   | dense_query_update |     1000 |  4096 |    317.0 |       319.6 |    322.2 |     322.1 |     0.320 |  3128.9 |     0.19x |
| c99:release   | dense_query_update |   100000 |   256 |  71801.2 |     74413.1 |  84989.8 |   75731.4 |     0.744 |  1343.8 |     0.41x |
| c99:release   | dense_query_update |   100000 |  4096 |  42306.6 |     43193.2 |  62870.8 |   46916.7 |     0.432 |  2315.2 |     0.26x |
| c99:release   | command_burst      |        - |     - |    719.1 |       720.4 |    781.5 |     731.5 |         - |       - |     2.87x |
| c99:release   | host_api_roundtrip |        - |     - |   1625.7 |      1658.6 |   1775.1 |    1671.7 |         - |       - |     1.90x |
| c99:debug     | noop               |        - |     - |     6.50 |        6.54 |     7.32 |      6.60 |         - |       - |     3.53x |
| c99:debug     | dense_query_update |     1000 |   256 |   6238.0 |      6276.6 |   6486.7 |    6328.8 |     6.277 |   159.3 |     3.70x |
| c99:debug     | dense_query_update |     1000 |  4096 |   6166.6 |      6175.4 |   6320.3 |    6220.7 |     6.175 |   161.9 |     3.73x |
| c99:debug     | dense_query_update |   100000 |   256 | 646416.2 |    650226.2 | 663073.0 |  651638.5 |     6.502 |   153.8 |     3.58x |
| c99:debug     | dense_query_update |   100000 |  4096 | 626547.2 |    628725.2 | 639371.0 |  629645.2 |     6.287 |   159.1 |     3.72x |
| c99:debug     | command_burst      |        - |     - |    764.1 |       765.7 |    821.5 |     772.5 |         - |       - |     3.05x |
| c99:debug     | host_api_roundtrip |        - |     - |   1828.6 |      1849.9 |   1901.3 |    1862.4 |         - |       - |     2.12x |
| cpp17:release | noop               |        - |     - |     6.17 |        6.17 |     7.25 |      6.27 |         - |       - |     3.33x |
| cpp17:release | dense_query_update |     1000 |   256 |    385.1 |       388.9 |    392.1 |     394.1 |     0.389 |  2571.3 |     0.23x |
| cpp17:release | dense_query_update |     1000 |  4096 |    279.9 |       283.9 |    287.7 |     286.4 |     0.284 |  3522.1 |     0.17x |
| cpp17:release | dense_query_update |   100000 |   256 |  71805.2 |     78387.5 |  87252.0 |   79128.3 |     0.784 |  1275.7 |     0.43x |
| cpp17:release | dense_query_update |   100000 |  4096 |  54419.2 |     56139.2 |  62095.2 |   56980.7 |     0.561 |  1781.3 |     0.33x |
| cpp17:release | command_burst      |        - |     - |    718.0 |       719.0 |    776.6 |     725.2 |         - |       - |     2.86x |
| cpp17:release | host_api_roundtrip |        - |     - |   1625.0 |      1635.9 |   1689.3 |    1648.9 |         - |       - |     1.87x |
| cpp17:debug   | noop               |        - |     - |     6.51 |        6.54 |     7.37 |      6.62 |         - |       - |     3.53x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   6046.4 |      6055.8 |   6260.4 |    6108.4 |     6.056 |   165.1 |     3.57x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   6069.5 |      6248.1 |   9467.4 |    6617.5 |     6.248 |   160.0 |     3.77x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 624277.0 |    627795.5 | 642871.6 |  629732.6 |     6.278 |   159.3 |     3.45x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 601945.4 |    603485.4 | 615208.0 |  604918.0 |     6.035 |   165.7 |     3.57x |
| cpp17:debug   | command_burst      |        - |     - |   1431.5 |      1439.9 |   1521.1 |    1453.1 |         - |       - |     5.73x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2597.1 |      2812.5 |   2977.0 |    2792.7 |         - |       - |     3.22x |
| zig:release   | noop               |        - |     - |     6.17 |        6.21 |     7.06 |      6.30 |         - |       - |     3.36x |
| zig:release   | dense_query_update |     1000 |   256 |    455.6 |       459.8 |    463.9 |     462.4 |     0.460 |  2175.0 |     0.27x |
| zig:release   | dense_query_update |     1000 |  4096 |    370.5 |       372.1 |    374.3 |     374.8 |     0.372 |  2687.6 |     0.22x |
| zig:release   | dense_query_update |   100000 |   256 |  69915.6 |     76995.9 |  99909.4 |   79986.9 |     0.770 |  1298.8 |     0.42x |
| zig:release   | dense_query_update |   100000 |  4096 |  46235.8 |     56580.0 |  62508.0 |   56185.7 |     0.566 |  1767.4 |     0.33x |
| zig:release   | command_burst      |        - |     - |    778.4 |       779.6 |    841.8 |     787.7 |         - |       - |     3.10x |
| zig:release   | host_api_roundtrip |        - |     - |   1741.4 |      1769.7 |   1841.9 |    1777.4 |         - |       - |     2.03x |
| zig:debug     | noop               |        - |     - |     6.60 |        6.62 |     7.39 |      6.74 |         - |       - |     3.58x |
| zig:debug     | dense_query_update |     1000 |   256 |   3632.7 |      3636.8 |   4082.0 |    3700.0 |     3.637 |   275.0 |     2.14x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3508.1 |      3511.1 |   3735.1 |    3563.7 |     3.511 |   284.8 |     2.12x |
| zig:debug     | dense_query_update |   100000 |   256 | 368492.2 |    375382.8 | 495189.4 |  389312.7 |     3.754 |   266.4 |     2.06x |
| zig:debug     | dense_query_update |   100000 |  4096 | 347597.4 |    351856.3 | 398674.2 |  360384.8 |     3.519 |   284.2 |     2.08x |
| zig:debug     | command_burst      |        - |     - |   2515.5 |      2551.9 |   2697.3 |    2567.1 |         - |       - |    10.16x |
| zig:debug     | host_api_roundtrip |        - |     - |   2565.5 |      2617.6 |   2815.0 |    2638.5 |         - |       - |     3.00x |
<!-- BENCHMARKS_RESULTS_END -->
