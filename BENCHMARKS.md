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
- **Commit:** `428b2b8`
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
| noop                               |               0.75 | `cpp17:release`: 2.33 ns (3.10x)    |          4.66 ns (6.21x) |
| dense_query_update (1000, c256)    |              185.8 | `cpp17:release`: 226.7 ns (1.22x)   |         308.8 ns (1.66x) |
| dense_query_update (1000, c4096)   |              193.3 | `cpp17:release`: 191.7 ns (0.99x)   |         260.0 ns (1.34x) |
| dense_query_update (100000, c256)  |            38287.5 | `cpp17:release`: 41033.4 ns (1.07x) |       41608.3 ns (1.09x) |
| dense_query_update (100000, c4096) |            33745.8 | `cpp17:release`: 33854.1 ns (1.00x) |       34425.1 ns (1.02x) |
| command_burst                      |              122.2 | `cpp17:release`: 447.5 ns (3.66x)   |         571.2 ns (4.67x) |
| host_api_roundtrip                 |              814.0 | `c99:release`: 1208.9 ns (1.49x)    |        1294.0 ns (1.59x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     0.75 |        0.75 |     0.76 |      0.75 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |    184.2 |       185.8 |    210.8 |     188.5 |     0.186 |  5381.0 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |    192.5 |       193.3 |    194.2 |     193.3 |     0.193 |  5172.2 |     1.00x |
| native        | dense_query_update |   100000 |   256 |  38050.0 |     38287.5 |  43008.2 |   39164.2 |     0.383 |  2611.8 |     1.00x |
| native        | dense_query_update |   100000 |  4096 |  33625.0 |     33745.8 |  39258.4 |   34749.0 |     0.337 |  2963.3 |     1.00x |
| native        | command_burst      |        - |     - |    121.4 |       122.2 |    127.8 |     123.4 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    811.7 |       814.0 |    896.1 |     834.3 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     4.52 |        4.66 |     5.99 |      4.86 |         - |       - |     6.21x |
| rust:release  | dense_query_update |     1000 |   256 |    303.3 |       308.8 |    312.5 |     310.5 |     0.309 |  3238.9 |     1.66x |
| rust:release  | dense_query_update |     1000 |  4096 |    252.5 |       260.0 |    266.7 |     261.1 |     0.260 |  3846.2 |     1.34x |
| rust:release  | dense_query_update |   100000 |   256 |  40958.4 |     41608.3 |  47108.4 |   42427.8 |     0.416 |  2403.4 |     1.09x |
| rust:release  | dense_query_update |   100000 |  4096 |  33933.2 |     34425.1 |  40175.0 |   35250.8 |     0.344 |  2904.9 |     1.02x |
| rust:release  | command_burst      |        - |     - |    563.3 |       571.2 |    617.5 |     576.2 |         - |       - |     4.67x |
| rust:release  | host_api_roundtrip |        - |     - |   1292.2 |      1294.0 |   1444.2 |    1318.8 |         - |       - |     1.59x |
| rust:debug    | noop               |        - |     - |    186.3 |       191.4 |    200.2 |     192.1 |         - |       - |   255.25x |
| rust:debug    | dense_query_update |     1000 |   256 |   7034.2 |      7337.5 |   8393.3 |    7480.3 |     7.338 |   136.3 |    39.48x |
| rust:debug    | dense_query_update |     1000 |  4096 |   6046.7 |      6441.2 |   7070.8 |    6477.0 |     6.441 |   155.2 |    33.32x |
| rust:debug    | dense_query_update |   100000 |   256 | 701416.8 |    723350.1 | 762900.0 |  723816.1 |     7.234 |   138.2 |    18.89x |
| rust:debug    | dense_query_update |   100000 |  4096 | 551975.0 |    585387.6 | 621850.0 |  587107.6 |     5.854 |   170.8 |    17.35x |
| rust:debug    | command_burst      |        - |     - |   1593.3 |      1698.1 |   1820.8 |    1708.2 |         - |       - |    13.89x |
| rust:debug    | host_api_roundtrip |        - |     - |  10588.9 |     11171.2 |  11633.1 |   11073.0 |         - |       - |    13.72x |
| c99:release   | noop               |        - |     - |     2.40 |        2.45 |     2.46 |      2.45 |         - |       - |     3.26x |
| c99:release   | dense_query_update |     1000 |   256 |    233.3 |       237.5 |    294.2 |     242.6 |     0.237 |  4210.5 |     1.28x |
| c99:release   | dense_query_update |     1000 |  4096 |    197.5 |       200.0 |    262.5 |     205.6 |     0.200 |  5000.0 |     1.03x |
| c99:release   | dense_query_update |   100000 |   256 |  39325.0 |     42883.3 |  45391.8 |   43103.3 |     0.429 |  2331.9 |     1.12x |
| c99:release   | dense_query_update |   100000 |  4096 |  33475.0 |     34075.0 |  43458.4 |   35737.0 |     0.341 |  2934.7 |     1.01x |
| c99:release   | command_burst      |        - |     - |    427.5 |       457.4 |    469.7 |     456.0 |         - |       - |     3.74x |
| c99:release   | host_api_roundtrip |        - |     - |   1186.4 |      1208.9 |   1272.5 |    1217.3 |         - |       - |     1.49x |
| c99:debug     | noop               |        - |     - |     6.17 |        6.18 |     7.40 |      6.34 |         - |       - |     8.24x |
| c99:debug     | dense_query_update |     1000 |   256 |   2493.3 |      2505.0 |   2788.3 |    2545.5 |     2.505 |   399.2 |    13.48x |
| c99:debug     | dense_query_update |     1000 |  4096 |   2420.8 |      2439.6 |   2795.8 |    2489.7 |     2.440 |   409.9 |    12.62x |
| c99:debug     | dense_query_update |   100000 |   256 | 262666.6 |    267991.7 | 287650.0 |  270410.7 |     2.680 |   373.1 |     7.00x |
| c99:debug     | dense_query_update |   100000 |  4096 | 255191.8 |    258312.6 | 278925.0 |  261841.1 |     2.583 |   387.1 |     7.65x |
| c99:debug     | command_burst      |        - |     - |    404.4 |       424.6 |    526.4 |     439.9 |         - |       - |     3.47x |
| c99:debug     | host_api_roundtrip |        - |     - |   1218.3 |      1256.1 |   1352.5 |    1267.2 |         - |       - |     1.54x |
| cpp17:release | noop               |        - |     - |     2.32 |        2.33 |     2.33 |      2.32 |         - |       - |     3.10x |
| cpp17:release | dense_query_update |     1000 |   256 |    223.3 |       226.7 |    264.2 |     229.9 |     0.227 |  4411.5 |     1.22x |
| cpp17:release | dense_query_update |     1000 |  4096 |    190.8 |       191.7 |    192.5 |     191.6 |     0.192 |  5217.6 |     0.99x |
| cpp17:release | dense_query_update |   100000 |   256 |  40750.0 |     41033.4 |  44916.6 |   41753.9 |     0.410 |  2437.0 |     1.07x |
| cpp17:release | dense_query_update |   100000 |  4096 |  33791.6 |     33854.1 |  38066.6 |   34676.0 |     0.339 |  2953.9 |     1.00x |
| cpp17:release | command_burst      |        - |     - |    413.3 |       447.5 |    531.1 |     460.4 |         - |       - |     3.66x |
| cpp17:release | host_api_roundtrip |        - |     - |   1191.7 |      1234.7 |   1376.7 |    1249.4 |         - |       - |     1.52x |
| cpp17:debug   | noop               |        - |     - |     2.46 |        2.47 |     2.81 |      2.51 |         - |       - |     3.29x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   2453.3 |      2585.4 |   5701.7 |    3201.6 |     2.585 |   386.8 |    13.91x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   2449.2 |      2505.0 |   4213.3 |    2725.2 |     2.505 |   399.2 |    12.96x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 268308.4 |    271258.2 | 294000.0 |  275678.3 |     2.713 |   368.7 |     7.08x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 255316.6 |    258533.3 | 274958.4 |  260740.4 |     2.585 |   386.8 |     7.66x |
| cpp17:debug   | command_burst      |        - |     - |    738.6 |       757.9 |    844.7 |     769.8 |         - |       - |     6.20x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1862.8 |      1936.0 |   2077.8 |    1944.6 |         - |       - |     2.38x |
| zig:release   | noop               |        - |     - |     2.50 |        2.54 |     3.17 |      2.64 |         - |       - |     3.39x |
| zig:release   | dense_query_update |     1000 |   256 |    332.5 |       335.4 |    446.7 |     350.2 |     0.335 |  2981.3 |     1.80x |
| zig:release   | dense_query_update |     1000 |  4096 |    272.5 |       275.0 |    310.8 |     279.7 |     0.275 |  3636.4 |     1.42x |
| zig:release   | dense_query_update |   100000 |   256 |  43791.6 |     44504.2 |  49333.4 |   45350.1 |     0.445 |  2247.0 |     1.16x |
| zig:release   | dense_query_update |   100000 |  4096 |  35391.8 |     35533.3 |  40475.0 |   36434.2 |     0.355 |  2814.3 |     1.05x |
| zig:release   | command_burst      |        - |     - |    454.2 |       513.2 |    546.7 |     508.4 |         - |       - |     4.20x |
| zig:release   | host_api_roundtrip |        - |     - |   1222.2 |      1223.6 |   1330.3 |    1248.8 |         - |       - |     1.50x |
| zig:debug     | noop               |        - |     - |     2.69 |        2.76 |     3.43 |      2.88 |         - |       - |     3.68x |
| zig:debug     | dense_query_update |     1000 |   256 |   2562.5 |      2695.4 |   2983.3 |    2722.9 |     2.695 |   371.0 |    14.50x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2273.3 |      2443.3 |   2741.7 |    2470.1 |     2.443 |   409.3 |    12.64x |
| zig:debug     | dense_query_update |   100000 |   256 | 257575.0 |    263083.4 | 285391.8 |  266274.6 |     2.631 |   380.1 |     6.87x |
| zig:debug     | dense_query_update |   100000 |  4096 | 244733.4 |    257516.8 | 270783.4 |  257503.9 |     2.575 |   388.3 |     7.63x |
| zig:debug     | command_burst      |        - |     - |   1062.8 |      1077.4 |   1184.7 |    1090.7 |         - |       - |     8.81x |
| zig:debug     | host_api_roundtrip |        - |     - |   2591.1 |      2668.6 |   2909.2 |    2687.7 |         - |       - |     3.28x |
<!-- BENCHMARKS_RESULTS_END -->
