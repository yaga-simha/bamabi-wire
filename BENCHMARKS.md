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
- **Commit:** `efa0012`
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
| noop                               |               1.11 | `zig:release`: 3.91 ns (3.51x)     |         10.62 ns (9.52x) |
| dense_query_update (1000, c256)    |             2086.5 | `cpp17:release`: 241.7 ns (0.12x)  |        2160.9 ns (1.04x) |
| dense_query_update (1000, c4096)   |             2087.6 | `cpp17:release`: 269.1 ns (0.13x)  |        2127.2 ns (1.02x) |
| dense_query_update (100000, c256)  |           228619.9 | `zig:release`: 115380.5 ns (0.50x) |      235516.5 ns (1.03x) |
| dense_query_update (100000, c4096) |           212125.1 | `zig:release`: 95834.0 ns (0.45x)  |      214533.7 ns (1.01x) |
| command_burst                      |              257.3 | `c99:release`: 469.6 ns (1.82x)    |         489.2 ns (1.90x) |
| host_api_roundtrip                 |             1443.7 | `cpp17:release`: 2215.7 ns (1.53x) |        2399.8 ns (1.66x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.11 |        1.11 |     1.12 |      1.15 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   2086.2 |      2086.5 |   2471.6 |    2120.6 |     2.087 |   479.3 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   2087.3 |      2087.6 |   2197.5 |    2098.2 |     2.088 |   479.0 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 228403.8 |    228619.9 | 232428.6 |  229321.0 |     2.286 |   437.4 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 211964.2 |    212125.1 | 216639.4 |  212546.8 |     2.121 |   471.4 |     1.00x |
| native        | command_burst      |        - |     - |    252.5 |       257.3 |    287.5 |     258.7 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |   1443.6 |      1443.7 |   1578.5 |    1469.1 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |    10.59 |       10.62 |    11.14 |     10.68 |         - |       - |     9.52x |
| rust:release  | dense_query_update |     1000 |   256 |   2159.1 |      2160.9 |   2261.6 |    2173.7 |     2.161 |   462.8 |     1.04x |
| rust:release  | dense_query_update |     1000 |  4096 |   2125.5 |      2127.2 |   2229.2 |    2141.7 |     2.127 |   470.1 |     1.02x |
| rust:release  | dense_query_update |   100000 |   256 | 235245.0 |    235516.5 | 241380.8 |  236305.2 |     2.355 |   424.6 |     1.03x |
| rust:release  | dense_query_update |   100000 |  4096 | 214373.4 |    214533.7 | 217147.8 |  214913.0 |     2.145 |   466.1 |     1.01x |
| rust:release  | command_burst      |        - |     - |    487.5 |       489.2 |    523.7 |     492.1 |         - |       - |     1.90x |
| rust:release  | host_api_roundtrip |        - |     - |   2394.2 |      2399.8 |   2448.0 |    2410.7 |         - |       - |     1.66x |
| rust:debug    | noop               |        - |     - |    55.78 |       56.56 |    57.37 |     56.57 |         - |       - |    50.73x |
| rust:debug    | dense_query_update |     1000 |   256 |   6243.0 |      7393.6 |   7527.0 |    7366.4 |     7.394 |   135.3 |     3.54x |
| rust:debug    | dense_query_update |     1000 |  4096 |   5551.6 |      6402.3 |   6519.0 |    6407.7 |     6.402 |   156.2 |     3.07x |
| rust:debug    | dense_query_update |   100000 |   256 | 613689.6 |    654769.1 | 731613.8 |  671919.8 |     6.548 |   152.7 |     2.86x |
| rust:debug    | dense_query_update |   100000 |  4096 | 495610.2 |    530596.0 | 547253.0 |  529625.2 |     5.306 |   188.5 |     2.50x |
| rust:debug    | command_burst      |        - |     - |   1933.0 |      2031.1 |   2122.4 |    2030.2 |         - |       - |     7.89x |
| rust:debug    | host_api_roundtrip |        - |     - |   9999.8 |     10227.8 |  10442.5 |   10253.0 |         - |       - |     7.08x |
| c99:release   | noop               |        - |     - |     3.68 |        4.18 |     4.73 |      4.14 |         - |       - |     3.75x |
| c99:release   | dense_query_update |     1000 |   256 |    281.8 |       284.0 |    301.9 |     286.8 |     0.284 |  3521.6 |     0.14x |
| c99:release   | dense_query_update |     1000 |  4096 |    287.7 |       288.3 |    289.0 |     291.0 |     0.288 |  3468.8 |     0.14x |
| c99:release   | dense_query_update |   100000 |   256 | 117402.4 |    119295.7 | 122006.0 |  118948.1 |     1.193 |   838.3 |     0.52x |
| c99:release   | dense_query_update |   100000 |  4096 |  96485.6 |     97610.1 | 101166.6 |   97957.2 |     0.976 |  1024.5 |     0.46x |
| c99:release   | command_burst      |        - |     - |    468.8 |       469.6 |    506.2 |     472.5 |         - |       - |     1.82x |
| c99:release   | host_api_roundtrip |        - |     - |   2214.3 |      2217.4 |   2352.4 |    2239.2 |         - |       - |     1.54x |
| c99:debug     | noop               |        - |     - |     3.65 |        4.22 |     5.01 |      4.24 |         - |       - |     3.78x |
| c99:debug     | dense_query_update |     1000 |   256 |   4125.9 |      4148.5 |   4283.9 |    4178.5 |     4.149 |   241.0 |     1.99x |
| c99:debug     | dense_query_update |     1000 |  4096 |   3959.3 |      3991.6 |   4098.8 |    4004.9 |     3.992 |   250.5 |     1.91x |
| c99:debug     | dense_query_update |   100000 |   256 | 434273.8 |    437137.1 | 442356.0 |  437693.9 |     4.371 |   228.8 |     1.91x |
| c99:debug     | dense_query_update |   100000 |  4096 | 403770.4 |    406424.3 | 412354.0 |  406857.2 |     4.064 |   246.0 |     1.92x |
| c99:debug     | command_burst      |        - |     - |    523.0 |       526.7 |    586.3 |     535.4 |         - |       - |     2.05x |
| c99:debug     | host_api_roundtrip |        - |     - |   2304.8 |      2309.1 |   2340.5 |    2321.3 |         - |       - |     1.60x |
| cpp17:release | noop               |        - |     - |     3.63 |        4.18 |     4.73 |      4.16 |         - |       - |     3.75x |
| cpp17:release | dense_query_update |     1000 |   256 |    239.9 |       241.7 |    275.8 |     255.2 |     0.242 |  4137.9 |     0.12x |
| cpp17:release | dense_query_update |     1000 |  4096 |    268.5 |       269.1 |    269.6 |     272.0 |     0.269 |  3716.5 |     0.13x |
| cpp17:release | dense_query_update |   100000 |   256 | 116711.8 |    118759.8 | 123213.2 |  118714.3 |     1.188 |   842.0 |     0.52x |
| cpp17:release | dense_query_update |   100000 |  4096 |  97412.6 |     97756.2 | 101357.8 |   98639.6 |     0.978 |  1023.0 |     0.46x |
| cpp17:release | command_burst      |        - |     - |    468.2 |       470.4 |    504.2 |     475.4 |         - |       - |     1.83x |
| cpp17:release | host_api_roundtrip |        - |     - |   2214.4 |      2215.7 |   2264.8 |    2232.4 |         - |       - |     1.53x |
| cpp17:debug   | noop               |        - |     - |     3.90 |        4.18 |     4.71 |      4.11 |         - |       - |     3.74x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   4343.7 |      4415.3 |   4543.3 |    4433.6 |     4.415 |   226.5 |     2.12x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   4186.8 |      4226.9 |   4345.6 |    4245.5 |     4.227 |   236.6 |     2.02x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 453577.6 |    456801.7 | 462952.4 |  457346.7 |     4.568 |   218.9 |     2.00x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 419986.6 |    422965.8 | 437411.0 |  424122.0 |     4.230 |   236.4 |     1.99x |
| cpp17:debug   | command_burst      |        - |     - |   1137.5 |      1155.6 |   1192.4 |    1158.8 |         - |       - |     4.49x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2512.6 |      2530.0 |   3057.2 |    2580.4 |         - |       - |     1.75x |
| zig:release   | noop               |        - |     - |     3.90 |        3.91 |     4.83 |      4.07 |         - |       - |     3.51x |
| zig:release   | dense_query_update |     1000 |   256 |    379.2 |       380.2 |    382.0 |     383.3 |     0.380 |  2630.0 |     0.18x |
| zig:release   | dense_query_update |     1000 |  4096 |    345.9 |       347.3 |    348.8 |     349.8 |     0.347 |  2879.4 |     0.17x |
| zig:release   | dense_query_update |   100000 |   256 | 113360.0 |    115380.5 | 117082.6 |  114976.6 |     1.154 |   866.7 |     0.50x |
| zig:release   | dense_query_update |   100000 |  4096 |  94522.0 |     95834.0 |  97847.8 |   95797.2 |     0.958 |  1043.5 |     0.45x |
| zig:release   | command_burst      |        - |     - |    556.7 |       557.7 |    597.3 |     565.6 |         - |       - |     2.17x |
| zig:release   | host_api_roundtrip |        - |     - |   2269.5 |      2284.9 |   2316.4 |    2289.8 |         - |       - |     1.58x |
| zig:debug     | noop               |        - |     - |     3.90 |        4.17 |     4.88 |      4.29 |         - |       - |     3.74x |
| zig:debug     | dense_query_update |     1000 |   256 |   3230.9 |      3248.9 |   3361.9 |    3264.2 |     3.249 |   307.8 |     1.56x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3142.4 |      3154.3 |   3256.7 |    3173.5 |     3.154 |   317.0 |     1.51x |
| zig:debug     | dense_query_update |   100000 |   256 | 340292.0 |    343663.7 | 348147.4 |  343813.5 |     3.437 |   291.0 |     1.50x |
| zig:debug     | dense_query_update |   100000 |  4096 | 313967.0 |    316156.0 | 318828.0 |  315981.4 |     3.162 |   316.3 |     1.49x |
| zig:debug     | command_burst      |        - |     - |   1449.9 |      1496.9 |   1618.1 |    1508.8 |         - |       - |     5.82x |
| zig:debug     | host_api_roundtrip |        - |     - |  10765.1 |     10855.3 |  10978.6 |   10868.1 |         - |       - |     7.52x |
<!-- BENCHMARKS_RESULTS_END -->
