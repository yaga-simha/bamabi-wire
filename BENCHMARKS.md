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
- **Commit:** `18f8152`
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
| noop                               |               1.63 | `c99:release`: 5.12 ns (3.15x)      |          6.68 ns (4.11x) |
| dense_query_update (1000, c256)    |             1329.0 | `c99:release`: 339.4 ns (0.26x)     |        1439.0 ns (1.08x) |
| dense_query_update (1000, c4096)   |             1291.6 | `cpp17:release`: 247.0 ns (0.19x)   |        1345.8 ns (1.04x) |
| dense_query_update (100000, c256)  |           141436.6 | `cpp17:release`: 49355.5 ns (0.35x) |      149860.3 ns (1.06x) |
| dense_query_update (100000, c4096) |           131198.0 | `c99:release`: 36484.1 ns (0.28x)   |      132460.0 ns (1.01x) |
| command_burst                      |              200.6 | `c99:release`: 540.2 ns (2.69x)     |         557.8 ns (2.78x) |
| host_api_roundtrip                 |              651.9 | `cpp17:release`: 1241.2 ns (1.90x)  |        1480.6 ns (2.27x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.37 |        1.63 |     1.63 |      1.64 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1320.8 |      1329.0 |   1588.6 |    1346.1 |     1.329 |   752.4 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1286.7 |      1291.6 |   1473.2 |    1309.2 |     1.292 |   774.3 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 139628.8 |    141436.6 | 145661.8 |  141687.3 |     1.414 |   707.0 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 129543.6 |    131198.0 | 135079.8 |  131353.3 |     1.312 |   762.2 |     1.00x |
| native        | command_burst      |        - |     - |    199.4 |       200.6 |    201.7 |     201.4 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    651.1 |       651.9 |    697.3 |     656.6 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     6.27 |        6.68 |     8.16 |      6.98 |         - |       - |     4.11x |
| rust:release  | dense_query_update |     1000 |   256 |   1436.0 |      1439.0 |   1569.4 |    1449.1 |     1.439 |   694.9 |     1.08x |
| rust:release  | dense_query_update |     1000 |  4096 |   1340.8 |      1345.8 |   1470.4 |    1355.0 |     1.346 |   743.0 |     1.04x |
| rust:release  | dense_query_update |   100000 |   256 | 147653.0 |    149860.3 | 151675.0 |  149871.1 |     1.499 |   667.3 |     1.06x |
| rust:release  | dense_query_update |   100000 |  4096 | 130667.2 |    132460.0 | 136287.8 |  132413.6 |     1.325 |   754.9 |     1.01x |
| rust:release  | command_burst      |        - |     - |    556.7 |       557.8 |    599.8 |     561.7 |         - |       - |     2.78x |
| rust:release  | host_api_roundtrip |        - |     - |   1475.4 |      1480.6 |   1552.2 |    1492.6 |         - |       - |     2.27x |
| rust:debug    | noop               |        - |     - |    65.77 |       68.02 |    71.20 |     68.91 |         - |       - |    41.85x |
| rust:debug    | dense_query_update |     1000 |   256 |   8014.1 |      8060.0 |   8280.5 |    8112.1 |     8.060 |   124.1 |     6.06x |
| rust:debug    | dense_query_update |     1000 |  4096 |   6912.2 |      6930.6 |   7104.1 |    6980.2 |     6.931 |   144.3 |     5.37x |
| rust:debug    | dense_query_update |   100000 |   256 | 785914.4 |    788465.2 | 797221.4 |  789773.7 |     7.885 |   126.8 |     5.57x |
| rust:debug    | dense_query_update |   100000 |  4096 | 644066.2 |    645607.5 | 653700.8 |  646943.6 |     6.456 |   154.9 |     4.92x |
| rust:debug    | command_burst      |        - |     - |   2157.0 |      2193.4 |   2252.7 |    2192.4 |         - |       - |    10.93x |
| rust:debug    | host_api_roundtrip |        - |     - |  11858.9 |     12045.8 |  12259.8 |   12060.1 |         - |       - |    18.48x |
| c99:release   | noop               |        - |     - |     5.09 |        5.12 |     5.77 |      5.17 |         - |       - |     3.15x |
| c99:release   | dense_query_update |     1000 |   256 |    336.3 |       339.4 |    342.5 |     345.2 |     0.339 |  2946.3 |     0.26x |
| c99:release   | dense_query_update |     1000 |  4096 |    282.2 |       285.0 |    298.3 |     291.0 |     0.285 |  3508.3 |     0.22x |
| c99:release   | dense_query_update |   100000 |   256 |  49180.2 |     49551.8 |  53128.0 |   50161.9 |     0.496 |  2018.1 |     0.35x |
| c99:release   | dense_query_update |   100000 |  4096 |  35387.4 |     36484.1 |  39525.6 |   36834.1 |     0.365 |  2740.9 |     0.28x |
| c99:release   | command_burst      |        - |     - |    539.2 |       540.2 |    582.0 |     544.3 |         - |       - |     2.69x |
| c99:release   | host_api_roundtrip |        - |     - |   1265.0 |      1269.5 |   1315.4 |    1277.5 |         - |       - |     1.95x |
| c99:debug     | noop               |        - |     - |     6.39 |        6.45 |     7.14 |      6.54 |         - |       - |     3.97x |
| c99:debug     | dense_query_update |     1000 |   256 |   5058.6 |      5075.9 |   5231.5 |    5110.5 |     5.076 |   197.0 |     3.82x |
| c99:debug     | dense_query_update |     1000 |  4096 |   4904.4 |      4911.8 |   5064.6 |    4945.0 |     4.912 |   203.6 |     3.80x |
| c99:debug     | dense_query_update |   100000 |   256 | 509413.0 |    513327.8 | 521431.0 |  514037.9 |     5.133 |   194.8 |     3.63x |
| c99:debug     | dense_query_update |   100000 |  4096 | 494602.4 |    496307.1 | 501308.6 |  496679.1 |     4.963 |   201.5 |     3.78x |
| c99:debug     | command_burst      |        - |     - |    592.4 |       601.8 |    653.1 |     607.1 |         - |       - |     3.00x |
| c99:debug     | host_api_roundtrip |        - |     - |   1367.7 |      1370.8 |   1450.3 |    1383.4 |         - |       - |     2.10x |
| cpp17:release | noop               |        - |     - |     5.07 |        5.13 |     5.89 |      5.19 |         - |       - |     3.16x |
| cpp17:release | dense_query_update |     1000 |   256 |    345.3 |       349.3 |    358.1 |     353.5 |     0.349 |  2862.7 |     0.26x |
| cpp17:release | dense_query_update |     1000 |  4096 |    244.6 |       247.0 |    251.2 |     250.4 |     0.247 |  4048.9 |     0.19x |
| cpp17:release | dense_query_update |   100000 |   256 |  48893.8 |     49355.5 |  51191.4 |   49844.3 |     0.494 |  2026.1 |     0.35x |
| cpp17:release | dense_query_update |   100000 |  4096 |  35639.8 |     36605.2 |  39253.2 |   36966.0 |     0.366 |  2731.9 |     0.28x |
| cpp17:release | command_burst      |        - |     - |    574.9 |       575.5 |    619.5 |     580.1 |         - |       - |     2.87x |
| cpp17:release | host_api_roundtrip |        - |     - |   1238.3 |      1241.2 |   1351.7 |    1257.8 |         - |       - |     1.90x |
| cpp17:debug   | noop               |        - |     - |     6.42 |        6.46 |     7.26 |      6.53 |         - |       - |     3.98x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   5152.0 |      5164.2 |   5439.8 |    5218.4 |     5.164 |   193.6 |     3.89x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   5091.1 |      5096.4 |   5256.9 |    5150.3 |     5.096 |   196.2 |     3.95x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 524583.8 |    527238.8 | 533124.6 |  527925.7 |     5.272 |   189.7 |     3.73x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 509499.0 |    511439.8 | 517981.8 |  511817.2 |     5.114 |   195.5 |     3.90x |
| cpp17:debug   | command_burst      |        - |     - |   1277.4 |      1327.1 |   1370.7 |    1325.5 |         - |       - |     6.61x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   1846.3 |      1855.6 |   1904.6 |    1866.6 |         - |       - |     2.85x |
| zig:release   | noop               |        - |     - |     5.89 |        5.92 |     6.63 |      5.97 |         - |       - |     3.64x |
| zig:release   | dense_query_update |     1000 |   256 |    413.8 |       422.0 |    483.3 |     429.3 |     0.422 |  2369.4 |     0.32x |
| zig:release   | dense_query_update |     1000 |  4096 |    324.9 |       325.7 |    327.3 |     328.4 |     0.326 |  3070.3 |     0.25x |
| zig:release   | dense_query_update |   100000 |   256 |  57991.4 |     58422.2 |  60527.4 |   58864.8 |     0.584 |  1711.7 |     0.41x |
| zig:release   | dense_query_update |   100000 |  4096 |  41014.0 |     41342.4 |  45246.4 |   42041.3 |     0.413 |  2418.8 |     0.32x |
| zig:release   | command_burst      |        - |     - |    590.2 |       590.8 |    635.0 |     595.4 |         - |       - |     2.94x |
| zig:release   | host_api_roundtrip |        - |     - |   1304.7 |      1305.7 |   1351.5 |    1314.8 |         - |       - |     2.00x |
| zig:debug     | noop               |        - |     - |     6.38 |        6.43 |     7.11 |      6.48 |         - |       - |     3.96x |
| zig:debug     | dense_query_update |     1000 |   256 |   3130.1 |      3132.9 |   3297.4 |    3158.3 |     3.133 |   319.2 |     2.36x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3074.2 |      3077.9 |   3220.7 |    3099.8 |     3.078 |   324.9 |     2.38x |
| zig:debug     | dense_query_update |   100000 |   256 | 316960.4 |    319408.0 | 325523.2 |  319911.0 |     3.194 |   313.1 |     2.26x |
| zig:debug     | dense_query_update |   100000 |  4096 | 304105.0 |    305883.6 | 310424.6 |  306112.1 |     3.059 |   326.9 |     2.33x |
| zig:debug     | command_burst      |        - |     - |   1916.7 |      1948.4 |   2039.5 |    1969.3 |         - |       - |     9.71x |
| zig:debug     | host_api_roundtrip |        - |     - |   1673.8 |      1680.2 |   1730.9 |    1690.7 |         - |       - |     2.58x |
<!-- BENCHMARKS_RESULTS_END -->
