# `bamabi_sdk_gen`

Deterministic C99, C++17, Zig, and component header generator (`#![forbid(unsafe_code)]`, zero Bevy dependencies) for `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

## Responsibilities

- **Single-Source C99 Header (`bamabi.h`):** Generates [`include/bamabi.h`](../../include/bamabi.h) directly from [`bamabi_sys::offsets`](../bamabi_sys) metadata, including `BAM_EXPORT`, `BAM_RESTRICT` (`__restrict` on MSVC, `restrict` in C99/C11), all 68 `BAM_HOST_API_OFFSET_*` slot offsets, `BAM_RESULT_CYCLE_DETECTED`, `BAM_RELATION_FLAG_*`, `BAM_SCHEDULE_SET_FLAG_*`, `bam_*` inline helpers, `bam_uuid_from_parts` (`SipHash-2-4-128`), and 64-bit/32-bit `BAM_STATIC_ASSERT` layout assertions.
- **Polyglot SDK Bundle (`sdk::export_sdk`):** Exports `bamabi.h`, `bamabi.hpp` (C++17 RAII wrappers, `bam::offsets::*`, `BAM_RESTRICT`, `copy_column_restricted`), and `bamabi.zig` (Zig `offsets.*`, `BAM_HOST_API_OFFSET_*`, `BAM_RESTRICT_QUALIFIER = "noalias"`, `copyColumnNoAlias`) for `cargo-bam` (in `bamabi-tools`).
- **Component Header Emission (`component_header`):** Renders C99 component declaration headers with `BAM_STATIC_ASSERT` size/offset checks from `ModComponent::C_DECL` strings.

## Standalone Usage

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

## Verification

```bash
cargo clippy -p bamabi_sdk_gen --all-targets --all-features -- -D warnings
cargo test -p bamabi_sdk_gen --all-features
cargo test -p bamabi_sdk_gen --test header_parity
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sdk_gen --no-deps --all-features
```

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
