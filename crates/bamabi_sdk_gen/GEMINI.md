# Agent Guide: `bamabi_sdk_gen`

Crate guide for **`bamabi_sdk_gen`** (`v0.3.0`, ABI `0x0003_0000`) — deterministic C99, C++17, Zig, and component header generator.

---

## 1. Crate Identity
- **Name:** `bamabi_sdk_gen` (`version = "0.3.0"`, `edition = "2024"`, MSRV `1.95`)
- **Role:** Generates the canonical ISO C99 ABI header (`include/bamabi.h`), C++17 RAII wrapper (`bamabi.hpp`), Zig module bindings (`bamabi.zig`), and `ModComponent::C_DECL` component headers directly from `bamabi_sys::offsets` and ABI constants.

---

## 2. Safety & Dependency Boundaries
- **Safety:** **`#![forbid(unsafe_code)]`** at crate root.
- **Zero Bevy Dependencies:** Depends strictly on `bamabi_sys` ONLY. **NEVER** add any `bevy*` dependency or build-time toolchain subprocess invocations inside library code.

---

## 3. Owned Surface
- **C99 Header Generator (`header_gen.rs`):** `render_c99_header` and `write_or_check_header` emitting deterministic `include/bamabi.h` (`BAM_EXPORT`, `BAM_RESTRICT`, all 73 `BAM_HOST_API_OFFSET_*` constants, `BAM_RESULT_CYCLE_DETECTED`, `BAM_RELATION_FLAG_*`, `BAM_SCHEDULE_SET_FLAG_*`, `bam_uuid_from_parts`, inline helpers, and 64-bit/32-bit `BAM_STATIC_ASSERT` layout guards).
- **Polyglot SDK Bundle (`sdk.rs`):** `export_sdk` writing `bamabi.h`, `bamabi.hpp` (C++17 RAII + `bam::offsets::*` + `copy_column_restricted`), and `bamabi.zig` (`offsets.*`, `BAM_RESTRICT_QUALIFIER = "noalias"`, `copyColumnNoAlias`) used by `cargo bam export-sdk`.
- **Component Headers (`component_header.rs`):** `render_component_header` and `section_for` rendering C99 component layout assertions from `ModComponent::C_DECL`.

---

## 4. Strict Engineering Rules
1. **Single Source of Truth:** All generated offsets, struct sizes, flags, and error codes MUST come directly from `bamabi_sys` (`offsets::OFF64_*`, `offsets::OFF32_*`, `SLOT_COUNT = 73`) — never hardcode duplicate wire numbers.
2. **Deterministic Byte-Identical Output:** Generated headers must contain zero timestamps, machine paths, or randomized ordering so `--check` modes in CI and `cargo bam` are deterministic.
3. **Strict Standard Conformance:** C99 output must compile cleanly under strict `-std=c99 -Wall -Wextra -Werror -pedantic`; C++17 output must compile under `-std=c++17`.

---

## 5. Standalone Verification Commands

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo tree | sed 's/ (.*//' | grep -v bamabi | grep -i bevy   # MUST print NOTHING
typos
```

---

## 6. Header Synchronization & Downstream Coordination
1. **In-Repo Byte-Parity Gate:** `crates/bamabi_sdk_gen/tests/header_parity.rs` verifies that `render_c99_header()` matches `../../include/bamabi.h` byte-for-byte, and `bamabi-wire` CI compiles both `tests/standalone_c_test.c` and `include/generated/testbed_components.h` under strict `-std=c99 -Wall -Wextra -Werror -pedantic`.
2. **Downstream Consumers:** `bamabi_sdk_gen` is consumed by `cargo bam export-sdk` (`bamabi-tools`) and `cargo xtask header` / `gen-component-headers` (`bamabi-showcase`).
