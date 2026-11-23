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
- **Commit:** `fdef9a4`
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
| noop                               |               1.74 | `c99:release`: 6.60 ns (3.79x)      |          8.40 ns (4.82x) |
| dense_query_update (1000, c256)    |             1666.0 | `c99:release`: 427.6 ns (0.26x)     |        1835.6 ns (1.10x) |
| dense_query_update (1000, c4096)   |             1613.9 | `cpp17:release`: 318.2 ns (0.20x)   |        1678.0 ns (1.04x) |
| dense_query_update (100000, c256)  |           178153.6 | `cpp17:release`: 63367.4 ns (0.36x) |      189767.6 ns (1.07x) |
| dense_query_update (100000, c4096) |           163554.1 | `c99:release`: 46771.9 ns (0.29x)   |      167001.0 ns (1.02x) |
| command_burst                      |              262.2 | `c99:release`: 715.2 ns (2.73x)     |         719.0 ns (2.74x) |
| host_api_roundtrip                 |              863.3 | `cpp17:release`: 1635.7 ns (1.89x)  |        1922.5 ns (2.23x) |

#### Full Permutation Results

| Subject       | Workload           | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------------- | ------------------ | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native        | noop               |        - |     - |     1.74 |        1.74 |     1.75 |      1.77 |         - |       - |     1.00x |
| native        | dense_query_update |     1000 |   256 |   1649.0 |      1666.0 |   1988.3 |    1692.4 |     1.666 |   600.2 |     1.00x |
| native        | dense_query_update |     1000 |  4096 |   1608.0 |      1613.9 |   1933.3 |    1640.0 |     1.614 |   619.6 |     1.00x |
| native        | dense_query_update |   100000 |   256 | 174787.6 |    178153.6 | 185884.0 |  179054.1 |     1.782 |   561.3 |     1.00x |
| native        | dense_query_update |   100000 |  4096 | 161229.6 |    163554.1 | 166829.8 |  163767.8 |     1.636 |   611.4 |     1.00x |
| native        | command_burst      |        - |     - |    259.8 |       262.2 |    323.5 |     269.1 |         - |       - |     1.00x |
| native        | host_api_roundtrip |        - |     - |    862.1 |       863.3 |    992.1 |     876.5 |         - |       - |     1.00x |
| rust:release  | noop               |        - |     - |     7.89 |        8.40 |    10.04 |      8.87 |         - |       - |     4.82x |
| rust:release  | dense_query_update |     1000 |   256 |   1831.1 |      1835.6 |   2009.2 |    1855.0 |     1.836 |   544.8 |     1.10x |
| rust:release  | dense_query_update |     1000 |  4096 |   1674.9 |      1678.0 |   1851.3 |    1693.6 |     1.678 |   596.0 |     1.04x |
| rust:release  | dense_query_update |   100000 |   256 | 186887.4 |    189767.6 | 195602.2 |  190438.7 |     1.898 |   527.0 |     1.07x |
| rust:release  | dense_query_update |   100000 |  4096 | 164678.6 |    167001.0 | 171216.2 |  167452.2 |     1.670 |   598.8 |     1.02x |
| rust:release  | command_burst      |        - |     - |    717.1 |       719.0 |    778.0 |     726.1 |         - |       - |     2.74x |
| rust:release  | host_api_roundtrip |        - |     - |   1912.4 |      1922.5 |   1987.7 |    1941.9 |         - |       - |     2.23x |
| rust:debug    | noop               |        - |     - |    85.67 |       87.20 |    89.14 |     87.28 |         - |       - |    50.07x |
| rust:debug    | dense_query_update |     1000 |   256 |   9455.9 |      9538.8 |   9728.7 |    9579.4 |     9.539 |   104.8 |     5.73x |
| rust:debug    | dense_query_update |     1000 |  4096 |   8153.8 |      8205.2 |   8414.4 |    8258.9 |     8.205 |   121.9 |     5.08x |
| rust:debug    | dense_query_update |   100000 |   256 | 925622.0 |    930727.6 | 946144.4 |  933633.4 |     9.307 |   107.4 |     5.22x |
| rust:debug    | dense_query_update |   100000 |  4096 | 757115.8 |    760523.8 | 795748.6 |  764318.0 |     7.605 |   131.5 |     4.65x |
| rust:debug    | command_burst      |        - |     - |   2758.1 |      2781.8 |   2881.3 |    2797.8 |         - |       - |    10.61x |
| rust:debug    | host_api_roundtrip |        - |     - |  13844.0 |     13966.1 |  14108.3 |   13979.9 |         - |       - |    16.18x |
| c99:release   | noop               |        - |     - |     6.56 |        6.60 |     7.78 |      6.69 |         - |       - |     3.79x |
| c99:release   | dense_query_update |     1000 |   256 |    422.4 |       427.6 |    430.6 |     430.6 |     0.428 |  2338.5 |     0.26x |
| c99:release   | dense_query_update |     1000 |  4096 |    362.1 |       365.6 |    377.8 |     370.3 |     0.366 |  2734.9 |     0.23x |
| c99:release   | dense_query_update |   100000 |   256 |  63335.4 |     63727.9 |  67207.0 |   64592.6 |     0.637 |  1569.2 |     0.36x |
| c99:release   | dense_query_update |   100000 |  4096 |  45491.0 |     46771.9 |  50019.6 |   47269.4 |     0.468 |  2138.0 |     0.29x |
| c99:release   | command_burst      |        - |     - |    714.4 |       715.2 |    771.3 |     722.2 |         - |       - |     2.73x |
| c99:release   | host_api_roundtrip |        - |     - |   1662.4 |      1667.6 |   1800.1 |    1693.2 |         - |       - |     1.93x |
| c99:debug     | noop               |        - |     - |     7.92 |        8.19 |     9.14 |      8.24 |         - |       - |     4.70x |
| c99:debug     | dense_query_update |     1000 |   256 |   6517.2 |      6532.5 |   6720.1 |    6587.7 |     6.533 |   153.1 |     3.92x |
| c99:debug     | dense_query_update |     1000 |  4096 |   6305.5 |      6316.4 |   6521.8 |    6377.3 |     6.316 |   158.3 |     3.91x |
| c99:debug     | dense_query_update |   100000 |   256 | 655025.4 |    658160.0 | 679415.4 |  663613.8 |     6.582 |   151.9 |     3.69x |
| c99:debug     | dense_query_update |   100000 |  4096 | 637806.0 |    639122.8 | 643884.8 |  639883.3 |     6.391 |   156.5 |     3.91x |
| c99:debug     | command_burst      |        - |     - |    776.5 |       779.2 |    834.9 |     787.2 |         - |       - |     2.97x |
| c99:debug     | host_api_roundtrip |        - |     - |   1801.3 |      1809.7 |   1869.3 |    1823.6 |         - |       - |     2.10x |
| cpp17:release | noop               |        - |     - |     6.55 |        6.61 |     7.49 |      6.69 |         - |       - |     3.79x |
| cpp17:release | dense_query_update |     1000 |   256 |    464.3 |       469.9 |    494.5 |     479.7 |     0.470 |  2128.2 |     0.28x |
| cpp17:release | dense_query_update |     1000 |  4096 |    314.7 |       318.2 |    320.9 |     322.6 |     0.318 |  3143.0 |     0.20x |
| cpp17:release | dense_query_update |   100000 |   256 |  62694.4 |     63367.4 |  67389.4 |   64177.8 |     0.634 |  1578.1 |     0.36x |
| cpp17:release | dense_query_update |   100000 |  4096 |  44669.8 |     47289.7 |  50957.2 |   47165.3 |     0.473 |  2114.6 |     0.29x |
| cpp17:release | command_burst      |        - |     - |    735.0 |       735.8 |    790.0 |     743.9 |         - |       - |     2.81x |
| cpp17:release | host_api_roundtrip |        - |     - |   1626.9 |      1635.7 |   1699.3 |    1649.9 |         - |       - |     1.89x |
| cpp17:debug   | noop               |        - |     - |     7.95 |        8.19 |     9.15 |      8.24 |         - |       - |     4.70x |
| cpp17:debug   | dense_query_update |     1000 |   256 |   6986.9 |      7010.4 |   7217.8 |    7070.7 |     7.010 |   142.6 |     4.21x |
| cpp17:debug   | dense_query_update |     1000 |  4096 |   6526.0 |      6536.7 |   6724.1 |    6591.8 |     6.537 |   153.0 |     4.05x |
| cpp17:debug   | dense_query_update |   100000 |   256 | 676278.6 |    679844.9 | 687048.6 |  680322.3 |     6.798 |   147.1 |     3.82x |
| cpp17:debug   | dense_query_update |   100000 |  4096 | 658418.2 |    660326.1 | 668319.0 |  662023.6 |     6.603 |   151.4 |     4.04x |
| cpp17:debug   | command_burst      |        - |     - |   1586.9 |      1612.7 |   1687.9 |    1625.9 |         - |       - |     6.15x |
| cpp17:debug   | host_api_roundtrip |        - |     - |   2243.6 |      2267.0 |   2324.3 |    2282.0 |         - |       - |     2.63x |
| zig:release   | noop               |        - |     - |     7.59 |        7.61 |     8.58 |      7.70 |         - |       - |     4.37x |
| zig:release   | dense_query_update |     1000 |   256 |    529.0 |       540.5 |    548.6 |     543.1 |     0.540 |  1850.1 |     0.32x |
| zig:release   | dense_query_update |     1000 |  4096 |    422.4 |       423.8 |    424.8 |     427.2 |     0.424 |  2359.5 |     0.26x |
| zig:release   | dense_query_update |   100000 |   256 |  74053.2 |     74565.9 |  78399.6 |   75494.9 |     0.746 |  1341.1 |     0.42x |
| zig:release   | dense_query_update |   100000 |  4096 |  51856.4 |     52254.0 |  55656.0 |   53107.9 |     0.523 |  1913.7 |     0.32x |
| zig:release   | command_burst      |        - |     - |    783.3 |       784.4 |    841.0 |     792.9 |         - |       - |     2.99x |
| zig:release   | host_api_roundtrip |        - |     - |   1707.4 |      1710.0 |   1768.7 |    1724.1 |         - |       - |     1.98x |
| zig:debug     | noop               |        - |     - |     8.24 |        8.26 |     9.25 |      8.35 |         - |       - |     4.74x |
| zig:debug     | dense_query_update |     1000 |   256 |   4193.0 |      4219.0 |   4672.5 |    4274.1 |     4.219 |   237.0 |     2.53x |
| zig:debug     | dense_query_update |     1000 |  4096 |   3960.6 |      3965.9 |   4183.8 |    4013.3 |     3.966 |   252.1 |     2.46x |
| zig:debug     | dense_query_update |   100000 |   256 | 408634.2 |    410977.7 | 416584.0 |  411681.1 |     4.110 |   243.3 |     2.31x |
| zig:debug     | dense_query_update |   100000 |  4096 | 390924.2 |    393814.4 | 397315.6 |  394026.8 |     3.938 |   253.9 |     2.41x |
| zig:debug     | command_burst      |        - |     - |   2450.2 |      2486.2 |   2565.6 |    2503.0 |         - |       - |     9.48x |
| zig:debug     | host_api_roundtrip |        - |     - |   2146.2 |      2155.9 |   2212.2 |    2171.8 |         - |       - |     2.50x |
<!-- BENCHMARKS_RESULTS_END -->
