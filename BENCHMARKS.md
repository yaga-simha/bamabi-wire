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
- **Commit:** `f243873`
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
| noop                               |               1.16 | `zig:release`: 2.53 ns (2.18x)      |          4.55 ns (3.92x) |
| dense_query_update (1000, c256)    |              618.3 | `cpp17:release`: 234.2 ns (0.38x)   |         305.0 ns (0.49x) |
| dense_query_update (1000, c4096)   |              195.4 | `cpp17:release`: 236.7 ns (1.21x)   |         304.2 ns (1.56x) |
| dense_query_update (100000, c256)  |            38450.0 | `c99:release`: 41083.3 ns (1.07x)   |       42675.0 ns (1.11x) |
| dense_query_update (100000, c4096) |            33704.1 | `cpp17:release`: 33895.9 ns (1.01x) |       34033.3 ns (1.01x) |
| command_burst                      |              145.0 | `zig:release`: 419.0 ns (2.89x)     |         680.4 ns (4.69x) |
| host_api_roundtrip                 |              813.2 | `c99:release`: 1177.2 ns (1.45x)    |        1312.1 ns (1.61x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.15 |        1.16 |     1.18 |      1.17 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |    616.7 |       618.3 |    659.2 |     621.4 |     0.618 |  1617.3 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |    190.8 |       195.4 |    591.7 |     244.8 |     0.195 |  5117.2 |     1.00x |
| native        | dense_query_update |   100000 |   256 |  36741.6 |     38450.0 |  65400.0 |   43078.5 |     0.385 |  2600.8 |     1.00x |
| native        | dense_query_update |   100000 |  4096 |  33608.4 |     33704.1 |  47433.4 |   35618.6 |     0.337 |  2967.0 |     1.00x |
| native        | command_burst      |        - |     - |    121.9 |       145.0 |    178.3 |     146.7 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    739.2 |       813.2 |    915.8 |     826.8 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     4.53 |        4.55 |     5.26 |      4.68 |         - |       - |     3.92x |
| rust:release  | dense_query_update |     1000 |   256 |    270.8 |       305.0 |    377.5 |     318.5 |     0.305 |  3278.7 |     0.49x |
| rust:release  | dense_query_update |     1000 |  4096 |    300.0 |       304.2 |    307.5 |     304.1 |     0.304 |  3287.7 |     1.56x |
| rust:release  | dense_query_update |   100000 |   256 |  39766.8 |     42675.0 |  50000.0 |   43978.1 |     0.427 |  2343.3 |     1.11x |
| rust:release  | dense_query_update |   100000 |  4096 |  30308.4 |     34033.3 |  40250.0 |   34966.4 |     0.340 |  2938.3 |     1.01x |
| rust:release  | command_burst      |        - |     - |    611.4 |       680.4 |    740.0 |     686.8 |         - |       - |     4.69x |
| rust:release  | host_api_roundtrip |        - |     - |   1277.2 |      1312.1 |   1458.9 |    1324.6 |         - |       - |     1.61x |
| rust:debug    | noop               |        - |     - |    183.8 |       188.7 |    197.9 |     189.8 |         - |       - |   162.33x |
| rust:debug    | dense_query_update |     1000 |   256 |   6718.3 |      7349.2 |   7905.8 |    7412.1 |     7.349 |   136.1 |    11.89x |
| rust:debug    | dense_query_update |     1000 |  4096 |   6052.5 |      6425.0 |   6749.2 |    6452.4 |     6.425 |   155.6 |    32.88x |
| rust:debug    | dense_query_update |   100000 |   256 | 689250.0 |    711979.2 | 732166.8 |  712542.4 |     7.120 |   140.5 |    18.52x |
| rust:debug    | dense_query_update |   100000 |  4096 | 571391.6 |    592158.3 | 614025.0 |  593141.4 |     5.922 |   168.9 |    17.57x |
| rust:debug    | command_burst      |        - |     - |   4419.7 |      4831.9 |   5167.8 |    4829.6 |         - |       - |    33.32x |
| rust:debug    | host_api_roundtrip |        - |     - |  10677.5 |     10982.4 |  11364.4 |   11016.2 |         - |       - |    13.51x |
| c99:release   | noop               |        - |     - |     5.40 |        5.40 |     5.57 |      5.42 |         - |       - |     4.65x |
| c99:release   | dense_query_update |     1000 |   256 |    713.3 |       715.0 |    755.8 |     718.9 |     0.715 |  1398.6 |     1.16x |
| c99:release   | dense_query_update |     1000 |  4096 |    510.8 |       513.3 |    727.5 |     550.7 |     0.513 |  1948.0 |     2.63x |
| c99:release   | dense_query_update |   100000 |   256 |  37100.0 |     41083.3 |  70075.0 |   43535.1 |     0.411 |  2434.1 |     1.07x |
| c99:release   | dense_query_update |   100000 |  4096 |  32191.8 |     33900.0 |  47183.4 |   35792.8 |     0.339 |  2949.9 |     1.01x |
| c99:release   | command_burst      |        - |     - |    403.6 |       423.6 |    490.3 |     433.4 |         - |       - |     2.92x |
| c99:release   | host_api_roundtrip |        - |     - |   1116.4 |      1177.2 |   1427.5 |    1219.5 |         - |       - |     1.45x |
| c99:debug     | noop               |        - |     - |     6.17 |        6.17 |     6.24 |      6.19 |         - |       - |     5.31x |
| c99:debug     | dense_query_update |     1000 |   256 |   2241.7 |      2537.9 |   6554.2 |    2997.1 |     2.538 |   394.0 |     4.10x |
| c99:debug     | dense_query_update |     1000 |  4096 |   2357.5 |      2437.1 |   4026.7 |    2652.3 |     2.437 |   410.3 |    12.47x |
| c99:debug     | dense_query_update |   100000 |   256 | 260975.0 |    269229.3 | 321483.4 |  275323.3 |     2.692 |   371.4 |     7.00x |
| c99:debug     | dense_query_update |   100000 |  4096 | 251150.0 |    257833.4 | 273800.0 |  259161.8 |     2.578 |   387.8 |     7.65x |
| c99:debug     | command_burst      |        - |     - |    397.8 |       412.4 |    453.1 |     414.0 |         - |       - |     2.84x |
| c99:debug     | host_api_roundtrip |        - |     - |   1091.4 |      1247.5 |   1463.1 |    1264.3 |         - |       - |     1.53x |
| cpp17:release | noop               |        - |     - |     2.36 |        5.40 |     6.34 |      5.17 |         - |       - |     4.65x |
| cpp17:release | dense_query_update |     1000 |   256 |    207.5 |       234.2 |    292.5 |     237.6 |     0.234 |  4270.6 |     0.38x |
| cpp17:release | dense_query_update |     1000 |  4096 |    215.0 |       236.7 |    540.0 |     322.9 |     0.237 |  4225.5 |     1.21x |
| cpp17:release | dense_query_update |   100000 |   256 |  36175.0 |     41158.4 |  69316.8 |   44221.7 |     0.412 |  2429.6 |     1.07x |
| cpp17:release | dense_query_update |   100000 |  4096 |  31583.4 |     33895.9 |  52508.4 |   36628.1 |     0.339 |  2950.2 |     1.01x |
| cpp17:release | command_burst      |        - |     - |    423.6 |       462.1 |    502.5 |     464.6 |         - |       - |     3.19x |
| cpp17:release | host_api_roundtrip |        - |     - |   1173.1 |      1209.9 |   1619.7 |    1274.8 |         - |       - |     1.49x |
| cpp17:debug   | noop               |        - |     - |     6.17 |        6.17 |     6.26 |      6.19 |         - |       - |     5.31x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   2540.0 |      2560.4 |   2848.3 |    2609.6 |     2.560 |   390.6 |     4.14x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   2423.3 |      2475.0 |   4220.0 |    2673.0 |     2.475 |   404.0 |    12.67x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 268200.0 |    272750.0 | 287275.0 |  274962.9 |     2.728 |   366.6 |     7.09x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 252783.4 |    258433.3 | 270508.4 |  259996.4 |     2.584 |   386.9 |     7.67x |
| cpp17:debug   | command_burst      |        - |     - |    734.4 |       752.1 |    863.9 |     762.2 |         - |       - |     5.19x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1756.4 |      1847.5 |   2039.7 |    1863.0 |         - |       - |     2.27x |
| zig:release   | noop               |        - |     - |     2.52 |        2.53 |     2.54 |      2.53 |         - |       - |     2.18x |
| zig:release   | dense_query_update |     1000 |   256 |    332.5 |       333.3 |    377.5 |     338.5 |     0.333 |  2999.9 |     0.54x |
| zig:release   | dense_query_update |     1000 |  4096 |    270.8 |       275.0 |    414.2 |     295.1 |     0.275 |  3636.4 |     1.41x |
| zig:release   | dense_query_update |   100000 |   256 |  42333.4 |     43712.5 |  48858.2 |   44510.4 |     0.437 |  2287.7 |     1.14x |
| zig:release   | dense_query_update |   100000 |  4096 |  34966.6 |     35416.6 |  38991.6 |   35907.4 |     0.354 |  2823.5 |     1.05x |
| zig:release   | command_burst      |        - |     - |    399.7 |       419.0 |    440.3 |     419.8 |         - |       - |     2.89x |
| zig:release   | host_api_roundtrip |        - |     - |   1209.4 |      1227.9 |   1387.5 |    1249.1 |         - |       - |     1.51x |
| zig:debug     | noop               |        - |     - |     2.43 |        2.70 |     3.47 |      2.80 |         - |       - |     2.33x |
| zig:debug     | dense_query_update |     1000 |   256 |   2521.7 |      2614.2 |   3069.2 |    2655.8 |     2.614 |   382.5 |     4.23x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2446.7 |      2657.1 |   2964.2 |    2680.4 |     2.657 |   376.4 |    13.60x |
| zig:debug     | dense_query_update |   100000 |   256 | 246716.8 |    261870.8 | 283283.4 |  265162.1 |     2.619 |   381.9 |     6.81x |
| zig:debug     | dense_query_update |   100000 |  4096 | 236483.2 |    263491.6 | 286650.0 |  262576.7 |     2.635 |   379.5 |     7.82x |
| zig:debug     | command_burst      |        - |     - |   1051.7 |      1081.4 |   1205.0 |    1104.1 |         - |       - |     7.46x |
| zig:debug     | host_api_roundtrip |        - |     - |   2593.6 |      2640.7 |   2831.7 |    2665.4 |         - |       - |     3.25x |
<!-- BENCHMARKS_RESULTS_END -->
