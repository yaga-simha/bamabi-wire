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
  - `noop` isolates per-system entry overhead: raw C99/C++17/Zig callbacks cross only a function pointer (`~2–5 ns`), whereas `rust:release` pays for safe `catch_unwind` + `ReentrantCell` + `sys_commands` setup (`~20 ns`).
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
- **Commit:** `d60148f`
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

| Workload                           | Native Median (ns) | Fastest Release Guest             | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | --------------------------------- | -----------------------: |
| noop                               |               0.68 | `zig:release`: 2.50 ns (3.68x)    |        21.93 ns (32.28x) |
| dense_query_update (1000, c256)    |              187.9 | `zig:release`: 336.7 ns (1.79x)   |         370.8 ns (1.97x) |
| dense_query_update (1000, c4096)   |              193.3 | `rust:release`: 249.2 ns (1.29x)  |         249.2 ns (1.29x) |
| dense_query_update (100000, c256)  |            37366.6 | `c99:release`: 40058.4 ns (1.07x) |       51187.5 ns (1.37x) |
| dense_query_update (100000, c4096) |            33650.0 | `c99:release`: 33783.4 ns (1.00x) |       34691.7 ns (1.03x) |
| command_burst                      |              121.1 | `c99:release`: 394.2 ns (3.25x)   |         664.3 ns (5.49x) |
| host_api_roundtrip                 |              815.8 | `c99:release`: 1164.2 ns (1.43x)  |        1957.1 ns (2.40x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     0.68 |        0.68 |     0.69 |      0.68 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |    168.3 |       187.9 |    190.0 |     186.5 |     0.188 |  5321.7 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |    174.2 |       193.3 |    194.2 |     192.1 |     0.193 |  5172.2 |     1.00x |
| native        | dense_query_update |   100000 |   256 |  37016.8 |     37366.6 |  40908.4 |   37919.5 |     0.374 |  2676.2 |     1.00x |
| native        | dense_query_update |   100000 |  4096 |  33566.6 |     33650.0 |  37450.0 |   34228.1 |     0.337 |  2971.8 |     1.00x |
| native        | command_burst      |        - |     - |    120.0 |       121.1 |    145.8 |     124.5 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    814.2 |       815.8 |    903.9 |     830.9 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |    21.48 |       21.93 |    23.61 |     22.21 |         - |       - |    32.28x |
| rust:release  | dense_query_update |     1000 |   256 |    367.5 |       370.8 |    373.3 |     371.2 |     0.371 |  2696.6 |     1.97x |
| rust:release  | dense_query_update |     1000 |  4096 |    246.7 |       249.2 |    251.7 |     249.6 |     0.249 |  4013.5 |     1.29x |
| rust:release  | dense_query_update |   100000 |   256 |  50900.0 |     51187.5 |  53833.2 |   51819.6 |     0.512 |  1953.6 |     1.37x |
| rust:release  | dense_query_update |   100000 |  4096 |  34533.4 |     34691.7 |  37016.6 |   35117.4 |     0.347 |  2882.5 |     1.03x |
| rust:release  | command_burst      |        - |     - |    654.7 |       664.3 |    756.9 |     675.9 |         - |       - |     5.49x |
| rust:release  | host_api_roundtrip |        - |     - |   1918.6 |      1957.1 |   2105.0 |    1972.1 |         - |       - |     2.40x |
| rust:debug    | noop               |        - |     - |    133.9 |       136.4 |    139.3 |     136.6 |         - |       - |   200.83x |
| rust:debug    | dense_query_update |     1000 |   256 |   8195.0 |      8279.6 |   8785.8 |    8364.3 |     8.280 |   120.8 |    44.06x |
| rust:debug    | dense_query_update |     1000 |  4096 |   6414.2 |      6487.1 |   7005.8 |    6572.9 |     6.487 |   154.2 |    33.55x |
| rust:debug    | dense_query_update |   100000 |   256 | 760441.6 |    805987.5 | 833800.0 |  808396.8 |     8.060 |   124.1 |    21.57x |
| rust:debug    | dense_query_update |   100000 |  4096 | 545750.0 |    578929.2 | 588466.6 |  578373.6 |     5.789 |   172.7 |    17.20x |
| rust:debug    | command_burst      |        - |     - |   4623.9 |      4740.3 |   4860.0 |    4747.5 |         - |       - |    39.14x |
| rust:debug    | host_api_roundtrip |        - |     - |  17263.6 |     17516.5 |  18193.6 |   17615.9 |         - |       - |    21.47x |
| c99:release   | noop               |        - |     - |     5.40 |        5.41 |     6.59 |      5.58 |         - |       - |     7.96x |
| c99:release   | dense_query_update |     1000 |   256 |    713.3 |       715.0 |    937.5 |     738.0 |     0.715 |  1398.6 |     3.81x |
| c99:release   | dense_query_update |     1000 |  4096 |    210.0 |       620.0 |    631.7 |     621.2 |     0.620 |  1612.9 |     3.21x |
| c99:release   | dense_query_update |   100000 |   256 |  36508.4 |     40058.4 |  78400.0 |   43110.3 |     0.401 |  2496.4 |     1.07x |
| c99:release   | dense_query_update |   100000 |  4096 |  33725.0 |     33783.4 |  35850.0 |   34168.3 |     0.338 |  2960.0 |     1.00x |
| c99:release   | command_burst      |        - |     - |    377.5 |       394.2 |    418.1 |     394.5 |         - |       - |     3.25x |
| c99:release   | host_api_roundtrip |        - |     - |   1153.6 |      1164.2 |   1220.0 |    1171.8 |         - |       - |     1.43x |
| c99:debug     | noop               |        - |     - |     6.17 |        6.17 |     6.73 |      6.22 |         - |       - |     9.09x |
| c99:debug     | dense_query_update |     1000 |   256 |   2351.7 |      2496.7 |   2714.2 |    2524.5 |     2.497 |   400.5 |    13.29x |
| c99:debug     | dense_query_update |     1000 |  4096 |   2420.8 |      2432.5 |   2609.2 |    2455.4 |     2.433 |   411.1 |    12.58x |
| c99:debug     | dense_query_update |   100000 |   256 | 262008.4 |    264191.7 | 268441.6 |  264675.0 |     2.642 |   378.5 |     7.07x |
| c99:debug     | dense_query_update |   100000 |  4096 | 249683.4 |    258108.3 | 272866.6 |  259706.5 |     2.581 |   387.4 |     7.67x |
| c99:debug     | command_burst      |        - |     - |    405.0 |       423.1 |    465.0 |     428.4 |         - |       - |     3.49x |
| c99:debug     | host_api_roundtrip |        - |     - |   1202.8 |      1206.9 |   1281.4 |    1222.1 |         - |       - |     1.48x |
| cpp17:release | noop               |        - |     - |     5.40 |        5.41 |     6.22 |      5.53 |         - |       - |     7.96x |
| cpp17:release | dense_query_update |     1000 |   256 |    698.3 |       700.0 |    905.0 |     714.4 |     0.700 |  1428.6 |     3.73x |
| cpp17:release | dense_query_update |     1000 |  4096 |    618.3 |       620.8 |    627.5 |     623.5 |     0.621 |  1610.8 |     3.21x |
| cpp17:release | dense_query_update |   100000 |   256 |  36333.4 |     40216.6 |  66825.0 |   42401.3 |     0.402 |  2486.5 |     1.08x |
| cpp17:release | dense_query_update |   100000 |  4096 |  33750.0 |     33850.0 |  35608.4 |   34235.6 |     0.339 |  2954.2 |     1.01x |
| cpp17:release | command_burst      |        - |     - |    440.0 |       468.3 |    508.9 |     471.7 |         - |       - |     3.87x |
| cpp17:release | host_api_roundtrip |        - |     - |   1210.8 |      1226.4 |   1343.1 |    1248.7 |         - |       - |     1.50x |
| cpp17:debug   | noop               |        - |     - |     6.17 |        6.18 |     8.53 |      6.44 |         - |       - |     9.10x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   2293.3 |      2546.7 |   4262.5 |    2764.0 |     2.547 |   392.7 |    13.55x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   2340.8 |      2463.3 |   2672.5 |    2485.2 |     2.463 |   406.0 |    12.74x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 250691.6 |    266629.1 | 280733.2 |  267578.9 |     2.666 |   375.1 |     7.14x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 251141.8 |    257458.3 | 264591.8 |  258048.8 |     2.575 |   388.4 |     7.65x |
| cpp17:debug   | command_burst      |        - |     - |    748.3 |       760.6 |    817.8 |     768.8 |         - |       - |     6.28x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1790.6 |      1841.4 |   1958.1 |    1844.7 |         - |       - |     2.26x |
| zig:release   | noop               |        - |     - |     2.26 |        2.50 |     3.09 |      2.56 |         - |       - |     3.68x |
| zig:release   | dense_query_update |     1000 |   256 |    307.5 |       336.7 |    345.8 |     337.6 |     0.337 |  2970.3 |     1.79x |
| zig:release   | dense_query_update |     1000 |  4096 |    274.2 |       275.0 |    310.0 |     281.0 |     0.275 |  3636.4 |     1.42x |
| zig:release   | dense_query_update |   100000 |   256 |  41766.6 |     44100.1 |  49683.4 |   45135.7 |     0.441 |  2267.6 |     1.18x |
| zig:release   | dense_query_update |   100000 |  4096 |  34941.6 |     36637.5 |  43058.4 |   37366.7 |     0.366 |  2729.4 |     1.09x |
| zig:release   | command_burst      |        - |     - |    368.1 |       448.6 |    505.0 |     446.0 |         - |       - |     3.70x |
| zig:release   | host_api_roundtrip |        - |     - |   1198.3 |      1234.4 |   1360.8 |    1250.8 |         - |       - |     1.51x |
| zig:debug     | noop               |        - |     - |     2.48 |        2.69 |     4.10 |      2.90 |         - |       - |     3.96x |
| zig:debug     | dense_query_update |     1000 |   256 |   2638.3 |      2792.1 |   3041.7 |    2834.7 |     2.792 |   358.2 |    14.86x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2359.2 |      2761.2 |   3152.5 |    2786.3 |     2.761 |   362.2 |    14.28x |
| zig:debug     | dense_query_update |   100000 |   256 | 247741.6 |    257345.8 | 272916.6 |  258861.4 |     2.573 |   388.6 |     6.89x |
| zig:debug     | dense_query_update |   100000 |  4096 | 277016.8 |    285670.9 | 298066.8 |  286673.1 |     2.857 |   350.1 |     8.49x |
| zig:debug     | command_burst      |        - |     - |   1061.7 |      1073.3 |   1154.7 |    1085.9 |         - |       - |     8.86x |
| zig:debug     | host_api_roundtrip |        - |     - |   2564.7 |      2601.5 |   2693.6 |    2617.6 |         - |       - |     3.19x |
<!-- BENCHMARKS_RESULTS_END -->
