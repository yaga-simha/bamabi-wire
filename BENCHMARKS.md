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
- **Commit:** `f08e904`
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
| noop                               |               1.54 | `c99:release`: 4.01 ns (2.60x)      |          4.32 ns (2.80x) |
| dense_query_update (1000, c256)    |             1723.3 | `cpp17:release`: 364.1 ns (0.21x)   |        1801.1 ns (1.05x) |
| dense_query_update (1000, c4096)   |             1697.2 | `cpp17:release`: 272.9 ns (0.16x)   |        1784.4 ns (1.05x) |
| dense_query_update (100000, c256)  |           183977.4 | `cpp17:release`: 73297.9 ns (0.40x) |      192690.8 ns (1.05x) |
| dense_query_update (100000, c4096) |           172088.3 | `c99:release`: 54683.2 ns (0.32x)   |      170083.6 ns (0.99x) |
| command_burst                      |              274.4 | `c99:release`: 698.4 ns (2.55x)     |         720.6 ns (2.63x) |
| host_api_roundtrip                 |              864.0 | `c99:release`: 1616.0 ns (1.87x)    |        1973.1 ns (2.28x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.54 |        1.54 |     1.56 |      1.57 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1718.0 |      1723.3 |   1897.9 |    1738.7 |     1.723 |   580.3 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1694.0 |      1697.2 |   1835.6 |    1710.1 |     1.697 |   589.2 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 181959.8 |    183977.4 | 186037.4 |  184257.2 |     1.840 |   543.5 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 170195.8 |    172088.3 | 174618.0 |  172273.7 |     1.721 |   581.1 |     1.00x |
| native        | command_burst      |        - |     - |    271.8 |       274.4 |    309.3 |     277.4 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    860.6 |       864.0 |    912.3 |     870.5 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     4.32 |        4.32 |     5.01 |      4.37 |         - |       - |     2.80x |
| rust:release  | dense_query_update |     1000 |   256 |   1796.2 |      1801.1 |   2004.7 |    1818.8 |     1.801 |   555.2 |     1.05x |
| rust:release  | dense_query_update |     1000 |  4096 |   1779.9 |      1784.4 |   1928.4 |    1803.2 |     1.784 |   560.4 |     1.05x |
| rust:release  | dense_query_update |   100000 |   256 | 190483.6 |    192690.8 | 197138.2 |  193099.4 |     1.927 |   519.0 |     1.05x |
| rust:release  | dense_query_update |   100000 |  4096 | 168023.8 |    170083.6 | 171215.6 |  170003.8 |     1.701 |   587.9 |     0.99x |
| rust:release  | command_burst      |        - |     - |    719.7 |       720.6 |    776.5 |     726.6 |         - |       - |     2.63x |
| rust:release  | host_api_roundtrip |        - |     - |   1929.7 |      1973.1 |   2055.2 |    1979.0 |         - |       - |     2.28x |
| rust:debug    | noop               |        - |     - |    82.40 |       83.79 |    84.94 |     83.78 |         - |       - |    54.31x |
| rust:debug    | dense_query_update |     1000 |   256 |   9657.4 |      9845.7 |  10175.0 |    9847.4 |     9.846 |   101.6 |     5.71x |
| rust:debug    | dense_query_update |     1000 |  4096 |   8210.1 |      8244.3 |   8476.6 |    8312.8 |     8.244 |   121.3 |     4.86x |
| rust:debug    | dense_query_update |   100000 |   256 | 947300.8 |    949750.6 | 958029.0 |  950750.4 |     9.498 |   105.3 |     5.16x |
| rust:debug    | dense_query_update |   100000 |  4096 | 765062.6 |    768187.5 | 786119.8 |  772552.6 |     7.682 |   130.2 |     4.46x |
| rust:debug    | command_burst      |        - |     - |   2682.3 |      2691.3 |   2751.4 |    2713.7 |         - |       - |     9.81x |
| rust:debug    | host_api_roundtrip |        - |     - |  14491.9 |     14524.9 |  14882.1 |   14578.0 |         - |       - |    16.81x |
| c99:release   | noop               |        - |     - |     4.01 |        4.01 |     4.76 |      4.07 |         - |       - |     2.60x |
| c99:release   | dense_query_update |     1000 |   256 |    369.5 |       375.9 |    378.9 |     378.8 |     0.376 |  2660.3 |     0.22x |
| c99:release   | dense_query_update |     1000 |  4096 |    348.7 |       353.7 |    358.1 |     357.1 |     0.354 |  2827.6 |     0.21x |
| c99:release   | dense_query_update |   100000 |   256 |  71670.0 |     73468.2 |  87531.4 |   75063.3 |     0.735 |  1361.1 |     0.40x |
| c99:release   | dense_query_update |   100000 |  4096 |  53838.6 |     54683.2 |  56713.8 |   54947.8 |     0.547 |  1828.7 |     0.32x |
| c99:release   | command_burst      |        - |     - |    696.5 |       698.4 |    754.2 |     706.6 |         - |       - |     2.55x |
| c99:release   | host_api_roundtrip |        - |     - |   1608.5 |      1616.0 |   1757.7 |    1647.4 |         - |       - |     1.87x |
| c99:debug     | noop               |        - |     - |     4.32 |        4.32 |     5.02 |      4.36 |         - |       - |     2.80x |
| c99:debug     | dense_query_update |     1000 |   256 |   6247.1 |      6259.5 |   6606.9 |    6329.1 |     6.259 |   159.8 |     3.63x |
| c99:debug     | dense_query_update |     1000 |  4096 |   6144.1 |      6153.3 |   6491.1 |    6224.6 |     6.153 |   162.5 |     3.63x |
| c99:debug     | dense_query_update |   100000 |   256 | 644080.6 |    645737.9 | 653925.0 |  646908.6 |     6.457 |   154.9 |     3.51x |
| c99:debug     | dense_query_update |   100000 |  4096 | 625626.2 |    626801.4 | 635148.0 |  627913.5 |     6.268 |   159.5 |     3.64x |
| c99:debug     | command_burst      |        - |     - |    745.7 |       748.2 |    805.6 |     757.0 |         - |       - |     2.73x |
| c99:debug     | host_api_roundtrip |        - |     - |   1816.5 |      1823.4 |   1882.0 |    1838.4 |         - |       - |     2.11x |
| cpp17:release | noop               |        - |     - |     4.01 |        4.01 |     4.02 |      4.03 |         - |       - |     2.60x |
| cpp17:release | dense_query_update |     1000 |   256 |    358.7 |       364.1 |    614.1 |     391.4 |     0.364 |  2746.6 |     0.21x |
| cpp17:release | dense_query_update |     1000 |  4096 |    271.1 |       272.9 |    275.3 |     276.4 |     0.273 |  3664.1 |     0.16x |
| cpp17:release | dense_query_update |   100000 |   256 |  71235.0 |     73297.9 |  82488.2 |   74409.3 |     0.733 |  1364.3 |     0.40x |
| cpp17:release | dense_query_update |   100000 |  4096 |  53676.2 |     54702.2 |  58200.8 |   55044.4 |     0.547 |  1828.1 |     0.32x |
| cpp17:release | command_burst      |        - |     - |    697.2 |       698.5 |    760.4 |     707.2 |         - |       - |     2.55x |
| cpp17:release | host_api_roundtrip |        - |     - |   1618.4 |      1628.7 |   1907.2 |    1671.5 |         - |       - |     1.89x |
| cpp17:debug   | noop               |        - |     - |     4.01 |        4.01 |     4.94 |      4.10 |         - |       - |     2.60x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   6068.9 |      6081.7 |   6250.7 |    6134.0 |     6.082 |   164.4 |     3.53x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   6058.9 |      6081.0 |   6238.9 |    6125.9 |     6.081 |   164.4 |     3.58x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 623308.0 |    624803.7 | 634753.4 |  626156.9 |     6.248 |   160.1 |     3.40x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 602200.6 |    603384.6 | 617158.4 |  604884.4 |     6.034 |   165.7 |     3.51x |
| cpp17:debug   | command_burst      |        - |     - |   1409.6 |      1417.1 |   1480.4 |    1430.7 |         - |       - |     5.16x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2287.3 |      2343.3 |   2395.8 |    2346.2 |         - |       - |     2.71x |
| zig:release   | noop               |        - |     - |     4.01 |        4.01 |     4.75 |      4.06 |         - |       - |     2.60x |
| zig:release   | dense_query_update |     1000 |   256 |    449.2 |       451.7 |    466.9 |     458.7 |     0.452 |  2213.7 |     0.26x |
| zig:release   | dense_query_update |     1000 |  4096 |    365.1 |       366.5 |    368.1 |     369.1 |     0.366 |  2728.7 |     0.22x |
| zig:release   | dense_query_update |   100000 |   256 |  74553.2 |     76908.7 |  83141.2 |   77261.1 |     0.769 |  1300.2 |     0.42x |
| zig:release   | dense_query_update |   100000 |  4096 |  54670.2 |     55174.0 |  60152.2 |   55973.3 |     0.552 |  1812.4 |     0.32x |
| zig:release   | command_burst      |        - |     - |    757.9 |       758.9 |    820.5 |     769.4 |         - |       - |     2.77x |
| zig:release   | host_api_roundtrip |        - |     - |   1712.7 |      1744.3 |   1800.4 |    1744.8 |         - |       - |     2.02x |
| zig:debug     | noop               |        - |     - |     4.32 |        4.32 |     5.04 |      4.37 |         - |       - |     2.80x |
| zig:debug     | dense_query_update |     1000 |   256 |   3610.3 |      3613.5 |   3765.0 |    3649.0 |     3.613 |   276.7 |     2.10x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3504.7 |      3507.0 |   3656.4 |    3540.1 |     3.507 |   285.1 |     2.07x |
| zig:debug     | dense_query_update |   100000 |   256 | 368708.6 |    370820.5 | 377036.0 |  371507.5 |     3.708 |   269.7 |     2.02x |
| zig:debug     | dense_query_update |   100000 |  4096 | 347551.0 |    349447.6 | 354409.8 |  349694.7 |     3.494 |   286.2 |     2.03x |
| zig:debug     | command_burst      |        - |     - |   2637.7 |      2694.8 |   2817.8 |    2700.4 |         - |       - |     9.82x |
| zig:debug     | host_api_roundtrip |        - |     - |   2611.1 |      2637.0 |   2795.2 |    2661.9 |         - |       - |     3.05x |
<!-- BENCHMARKS_RESULTS_END -->
