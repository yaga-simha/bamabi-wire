# `bamabi_sdk_gen`

Deterministic C99, C++17, Zig, and component header generator (`#![forbid(unsafe_code)]`, zero Bevy dependencies) for `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

## Responsibilities

- **Single-Source C99 Header (`bamabi.h`):** Generates [`include/bamabi.h`](../../include/bamabi.h) directly from [`bamabi_sys::offsets`](../bamabi_sys/README.md) metadata, including `BAM_EXPORT`, `BAM_RESTRICT` (`__restrict` on MSVC, `restrict` in C99/C11), `BAM_CHILD_OF_NAME`/`BAM_CHILDREN_NAME`/`BAM_DISABLED_NAME`, all 68 `BAM_HOST_API_OFFSET_*` slot offsets, `BAM_RESULT_CYCLE_DETECTED`, `BAM_RELATION_FLAG_*`, `BAM_SCHEDULE_SET_FLAG_*`, `bam_*` inline helpers (`bam_disabled_uuid`, `bam_asset_load_state_is_*`, `bam_event_kind_is_lifecycle`, `bam_schedule_is_valid`, `bam_field_kind_is_valid`), `bam_uuid_from_parts` (`SipHash-2-4-128`), and 64-bit/32-bit `BAM_STATIC_ASSERT` layout assertions.
- **Polyglot SDK Bundle (`sdk::export_sdk`):** Exports `bamabi.h`, `bamabi.hpp` (ISO C++17 68/68 slot view wrappers `HostContextView`/`LoadContextView`/`ChunkView`/`CommandsView`/`SystemContextView`, RAII `ScopedTransaction`, `bam::offsets::*`, `BAM_RESTRICT`, `copy_column_restricted`), and `bamabi.zig` (Zig `0.16` `b.addTranslateC` `"bam_c"` 68/68 slot views, lossless 18-code `BamError`/`checkResult`/`mapError`, `offsets.*`, `BAM_HOST_API_OFFSET_*`, `BAM_RESTRICT_QUALIFIER = "noalias"`, `copyColumnNoAlias`) for `cargo-bam` (in `bamabi-tools`).
- **Component Header Emission (`component_header`):** Renders C99/C++17 component declaration headers with `BAM_STATIC_ASSERT` size/offset checks from `ModComponent::C_DECL` strings.

## Documentation

- **Crate Architecture & Implementation:** [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Test & Verification Matrix:** [`TESTS.md`](TESTS.md)
- **Workspace Documentation:** [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) · [`../../TESTING.md`](../../TESTING.md)

## Setup & Building

- Rust toolchain pinned in [`../../rust-toolchain.toml`](../../rust-toolchain.toml) (`edition = "2024"`, MSRV `1.95`).
- Optional polyglot toolchains for full integration verification: `cc` / `clang` (C99 + ASan/UBSan), `c++` (ISO C++17), and `zig` (`0.16.0`).

```bash
# Build bamabi_sdk_gen and verify zero Bevy dependencies
cargo build -p bamabi_sdk_gen --all-features
cargo tree -p bamabi_sdk_gen | sed 's/ (.*//' | grep -v bamabi | grep -i bevy || true
```

## Running Codegen & Exporting SDKs

### In-Repo Header Regeneration & Parity Checks

```bash
# Regenerate or verify include/bamabi.h
cargo xtask header
cargo xtask header --check

# Regenerate or verify include/generated/testbed_components.h
cargo xtask gen-component-headers
cargo xtask gen-component-headers --check
```

### Standalone Library Usage (`export_sdk`)

```toml
[dependencies]
bamabi_sdk_gen = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main" }
```

```rust,no_run
use std::path::Path;

let c99 = bamabi_sdk_gen::render_c99_header().expect("valid header");
assert!(c99.contains("BAM_ABI_VERSION"));
bamabi_sdk_gen::export_sdk(Path::new("target/sdk")).expect("sdk export");
```

Downstream C99/C++17/Zig guests consume the exported bundle via `cargo bam export-sdk` (`bamabi-tools`) or direct compiler flags (`-std=c99 -Wall -Wextra -Werror -pedantic`, `-std=c++17 -Wall -Wextra -Wpedantic -Werror`, or Zig `b.addTranslateC` `"bam_c"`).

## Testing

```bash
cargo fmt -p bamabi_sdk_gen -- --check
cargo clippy -p bamabi_sdk_gen --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sdk_gen --no-deps --all-features
cargo test -p bamabi_sdk_gen --all-features
cargo test --doc -p bamabi_sdk_gen --all-features
cargo test -p bamabi_sdk_gen --test header_parity
cargo xtask c-test
```

See [`TESTS.md`](TESTS.md) for every unit, parity, C++17/Zig compiler, and C99 ASan/UBSan test and the invariants each proves.

## Benchmarking

Layer 0 (`bamabi-wire`) contains **no Criterion benchmarks**; `bamabi_sdk_gen` performs deterministic string synthesis and compile-time layout checks rather than frame-loop execution. Runtime FFI dispatch and ECS query throughput benchmarks live in Layer 4 (`bamabi-showcase`).

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
