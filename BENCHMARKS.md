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
- **Commit:** `885e972`
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

| Workload                           | Native Median (ns) | Fastest Release Guest              | Safe Rust (rust:release) |
| ---------------------------------- | -----------------: | ---------------------------------- | -----------------------: |
| noop                               |               1.16 | `zig:release`: 2.50 ns (2.15x)     |          4.62 ns (3.97x) |
| dense_query_update (1000, c256)    |              186.2 | `c99:release`: 227.5 ns (1.22x)    |         300.0 ns (1.61x) |
| dense_query_update (1000, c4096)   |              193.3 | `cpp17:release`: 191.7 ns (0.99x)  |         240.8 ns (1.25x) |
| dense_query_update (100000, c256)  |            38479.1 | `c99:release`: 41437.5 ns (1.08x)  |       43054.1 ns (1.12x) |
| dense_query_update (100000, c4096) |            33641.8 | `c99:release`: 33900.0 ns (1.01x)  |       34137.5 ns (1.01x) |
| command_burst                      |              122.5 | `c99:release`: 434.9 ns (3.55x)    |         679.2 ns (5.54x) |
| host_api_roundtrip                 |             1043.3 | `cpp17:release`: 1179.6 ns (1.13x) |        1292.5 ns (1.24x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.15 |        1.16 |     2.17 |      1.28 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |    185.0 |       186.2 |    212.5 |     190.1 |     0.186 |  5369.1 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |    192.5 |       193.3 |    249.2 |     203.7 |     0.193 |  5172.2 |     1.00x |
| native        | dense_query_update |   100000 |   256 |  38241.6 |     38479.1 |  54058.4 |   41193.6 |     0.385 |  2598.8 |     1.00x |
| native        | dense_query_update |   100000 |  4096 |  33575.0 |     33641.8 |  51350.0 |   35620.4 |     0.336 |  2972.5 |     1.00x |
| native        | command_burst      |        - |     - |    109.7 |       122.5 |    192.8 |     127.0 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    798.6 |      1043.3 |   1122.8 |     989.7 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     4.60 |        4.62 |     4.92 |      4.66 |         - |       - |     3.97x |
| rust:release  | dense_query_update |     1000 |   256 |    299.2 |       300.0 |    301.7 |     300.3 |     0.300 |  3333.3 |     1.61x |
| rust:release  | dense_query_update |     1000 |  4096 |    239.2 |       240.8 |    269.2 |     244.4 |     0.241 |  4152.1 |     1.25x |
| rust:release  | dense_query_update |   100000 |   256 |  41633.2 |     43054.1 |  50791.6 |   44647.5 |     0.431 |  2322.7 |     1.12x |
| rust:release  | dense_query_update |   100000 |  4096 |  34025.0 |     34137.5 |  39108.4 |   35028.2 |     0.341 |  2929.3 |     1.01x |
| rust:release  | command_burst      |        - |     - |    660.6 |       679.2 |    733.9 |     682.6 |         - |       - |     5.54x |
| rust:release  | host_api_roundtrip |        - |     - |   1271.9 |      1292.5 |   1501.9 |    1321.1 |         - |       - |     1.24x |
| rust:debug    | noop               |        - |     - |    177.8 |       182.8 |    191.2 |     183.2 |         - |       - |   157.25x |
| rust:debug    | dense_query_update |     1000 |   256 |   7119.2 |      7202.5 |   7967.5 |    7316.9 |     7.202 |   138.8 |    38.67x |
| rust:debug    | dense_query_update |     1000 |  4096 |   5855.8 |      6260.8 |   6564.2 |    6284.1 |     6.261 |   159.7 |    32.38x |
| rust:debug    | dense_query_update |   100000 |   256 | 668725.0 |    697254.2 | 725491.6 |  699122.1 |     6.973 |   143.4 |    18.12x |
| rust:debug    | dense_query_update |   100000 |  4096 | 544066.6 |    564362.5 | 588125.0 |  565817.5 |     5.644 |   177.2 |    16.78x |
| rust:debug    | command_burst      |        - |     - |   4722.5 |      4847.6 |   5168.1 |    4882.5 |         - |       - |    39.57x |
| rust:debug    | host_api_roundtrip |        - |     - |  10250.6 |     10785.7 |  11160.6 |   10794.0 |         - |       - |    10.34x |
| c99:release   | noop               |        - |     - |     5.40 |        5.41 |    11.00 |      6.13 |         - |       - |     4.65x |
| c99:release   | dense_query_update |     1000 |   256 |    206.7 |       227.5 |    267.5 |     229.2 |     0.228 |  4395.6 |     1.22x |
| c99:release   | dense_query_update |     1000 |  4096 |    189.2 |       197.5 |    247.5 |     208.3 |     0.198 |  5063.3 |     1.02x |
| c99:release   | dense_query_update |   100000 |   256 |  40725.0 |     41437.5 |  60300.0 |   43499.6 |     0.414 |  2413.3 |     1.08x |
| c99:release   | dense_query_update |   100000 |  4096 |  33408.4 |     33900.0 |  51700.0 |   35952.7 |     0.339 |  2949.9 |     1.01x |
| c99:release   | command_burst      |        - |     - |    382.5 |       434.9 |    637.8 |     471.0 |         - |       - |     3.55x |
| c99:release   | host_api_roundtrip |        - |     - |   1149.2 |      1217.5 |   1522.2 |    1286.1 |         - |       - |     1.17x |
| c99:debug     | noop               |        - |     - |     6.17 |        6.18 |     8.91 |      6.80 |         - |       - |     5.32x |
| c99:debug     | dense_query_update |     1000 |   256 |   2411.7 |      2502.5 |   2746.7 |    2536.9 |     2.502 |   399.6 |    13.44x |
| c99:debug     | dense_query_update |     1000 |  4096 |   2420.8 |      2438.3 |   2861.7 |    2495.5 |     2.438 |   410.1 |    12.61x |
| c99:debug     | dense_query_update |   100000 |   256 | 262166.6 |    264950.0 | 288308.4 |  269347.1 |     2.650 |   377.4 |     6.89x |
| c99:debug     | dense_query_update |   100000 |  4096 | 251041.6 |    259925.0 | 271875.0 |  261324.2 |     2.599 |   384.7 |     7.73x |
| c99:debug     | command_burst      |        - |     - |    416.7 |       431.0 |    438.1 |     432.0 |         - |       - |     3.52x |
| c99:debug     | host_api_roundtrip |        - |     - |   1104.2 |      1236.0 |   1282.5 |    1228.9 |         - |       - |     1.18x |
| cpp17:release | noop               |        - |     - |     5.40 |        5.41 |     6.20 |      5.50 |         - |       - |     4.65x |
| cpp17:release | dense_query_update |     1000 |   256 |    206.7 |       230.0 |    260.0 |     230.8 |     0.230 |  4347.8 |     1.23x |
| cpp17:release | dense_query_update |     1000 |  4096 |    172.5 |       191.7 |    193.3 |     189.6 |     0.192 |  5217.6 |     0.99x |
| cpp17:release | dense_query_update |   100000 |   256 |  38591.6 |     42179.2 |  77875.0 |   52177.1 |     0.422 |  2370.8 |     1.10x |
| cpp17:release | dense_query_update |   100000 |  4096 |  30850.0 |     33941.8 |  52183.4 |   36900.8 |     0.339 |  2946.2 |     1.01x |
| cpp17:release | command_burst      |        - |     - |    430.8 |       449.3 |    573.6 |     466.9 |         - |       - |     3.67x |
| cpp17:release | host_api_roundtrip |        - |     - |   1178.1 |      1179.6 |   1428.6 |    1220.9 |         - |       - |     1.13x |
| cpp17:debug   | noop               |        - |     - |     2.47 |        2.55 |     7.07 |      3.08 |         - |       - |     2.20x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   2526.7 |      2544.6 |   3238.3 |    2677.9 |     2.545 |   393.0 |    13.66x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   2170.8 |      3757.9 |   4197.5 |    3342.2 |     3.758 |   266.1 |    19.44x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 261516.6 |    269945.9 | 296391.8 |  273083.1 |     2.699 |   370.4 |     7.02x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 247300.0 |    257866.7 | 274575.0 |  259330.8 |     2.579 |   387.8 |     7.67x |
| cpp17:debug   | command_burst      |        - |     - |    748.9 |       767.6 |    850.8 |     780.3 |         - |       - |     6.27x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1823.9 |      1854.2 |   2001.9 |    1875.6 |         - |       - |     1.78x |
| zig:release   | noop               |        - |     - |     2.50 |        2.50 |     2.66 |      2.53 |         - |       - |     2.15x |
| zig:release   | dense_query_update |     1000 |   256 |    329.2 |       335.0 |    427.5 |     349.6 |     0.335 |  2985.1 |     1.80x |
| zig:release   | dense_query_update |     1000 |  4096 |    269.2 |       270.8 |    319.2 |     279.2 |     0.271 |  3692.2 |     1.40x |
| zig:release   | dense_query_update |   100000 |   256 |  41900.0 |     42500.0 |  50158.4 |   43431.8 |     0.425 |  2352.9 |     1.10x |
| zig:release   | dense_query_update |   100000 |  4096 |  35000.0 |     35450.0 |  40433.2 |   36196.4 |     0.354 |  2820.9 |     1.05x |
| zig:release   | command_burst      |        - |     - |    438.9 |       469.6 |    494.4 |     468.1 |         - |       - |     3.83x |
| zig:release   | host_api_roundtrip |        - |     - |   1186.4 |      1212.2 |   1348.9 |    1237.2 |         - |       - |     1.16x |
| zig:debug     | noop               |        - |     - |     2.69 |        2.70 |     2.70 |      2.70 |         - |       - |     2.32x |
| zig:debug     | dense_query_update |     1000 |   256 |   2380.8 |      2793.8 |   2987.5 |    2790.5 |     2.794 |   357.9 |    15.00x |
| zig:debug     | dense_query_update |     1000 |  4096 |   2459.2 |      2603.8 |   2806.7 |    2613.7 |     2.604 |   384.1 |    13.47x |
| zig:debug     | dense_query_update |   100000 |   256 | 250125.0 |    258912.5 | 286241.8 |  264229.6 |     2.589 |   386.2 |     6.73x |
| zig:debug     | dense_query_update |   100000 |  4096 | 268100.0 |    284287.5 | 295066.6 |  283267.2 |     2.843 |   351.8 |     8.45x |
| zig:debug     | command_burst      |        - |     - |   1068.6 |      1083.8 |   1167.5 |    1096.5 |         - |       - |     8.85x |
| zig:debug     | host_api_roundtrip |        - |     - |   2327.2 |      2633.3 |   2908.3 |    2660.7 |         - |       - |     2.52x |
<!-- BENCHMARKS_RESULTS_END -->
