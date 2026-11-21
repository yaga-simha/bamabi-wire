#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Deterministic, `#![forbid(unsafe_code)]`, zero-Bevy polyglot SDK and component header generator
//! for `bamabi` (`ABI 0.3.0`, `0x0003_0000`).
//!
//! `bamabi_sdk_gen` sits in Layer 0 (`bamabi-wire`) and translates the Rust ABI definitions in
//! [`bamabi_sys`] into byte-identical C99, C++17, Zig, and component declaration headers without
//! depending on `bevy_*` or spawning compiler subprocesses at library runtime.
//!
//! ## Workspace & Crate Documentation
//!
//! - **Crate guides:** [`docs::architecture`] (`crates/bamabi_sdk_gen/ARCHITECTURE.md`) · [`docs::tests`] (`crates/bamabi_sdk_gen/TESTS.md`)
//! - **Workspace guides:** [`bamabi_sys::docs::workspace`](../bamabi_sys/docs/workspace/index.html) · [`bamabi_guest::docs::workspace`](../bamabi_guest/docs/workspace/index.html)
//! - **Sibling crates:** [`bamabi_sys`](../bamabi_sys/index.html) · [`bamabi_macros`](../bamabi_macros/index.html) · [`bamabi_guest_sys`](../bamabi_guest_sys/index.html) · [`bamabi_guest`](../bamabi_guest/index.html) · [`xtask`](../xtask/index.html)
//!
//! # Learning Path
//!
//! ## Step 1: Role — Deterministic Polyglot SDK Emitter with Zero Bevy Dependencies
//!
//! Every non-Rust guest `.cdylib` mod (C99, C++17, or Zig) and every C-ABI verification tool
//! consumes headers emitted by `bamabi_sdk_gen`:
//!
//! - **Safety & dependency firewall:** `#![forbid(unsafe_code)]` at the crate root and `bamabi_sys`
//!   as the sole dependency (`zero` Bevy crates, `zero` procedural macros, `zero` runtime
//!   subprocesses).
//! - **Deterministic output:** Emitted headers contain no timestamps, host paths, or randomized
//!   iteration orders, enabling strict byte-for-byte parity gates (`cargo xtask header --check`,
//!   `cargo xtask gen-component-headers --check`, and `tests/header_parity.rs`).
//! - **Four emitted artifacts:**
//!   1. `include/bamabi.h` — canonical ISO C99 wire header (`header_gen`).
//!   2. `bamabi.hpp` — ISO C++17 RAII + typed view wrapper header (`sdk::CPP17_HEADER`).
//!   3. `bamabi.zig` — Zig `0.16` `b.addTranslateC` (`"bam_c"`) module bindings (`sdk::ZIG_SDK`).
//!   4. `include/generated/*.h` — C99/C++17 component declaration headers (`component_header`).
//!
//! ## Step 2: How `bamabi_sys::offsets` Drives Zero-Drift Header Generation
//!
//! Rather than duplicating wire constants by hand, [`header_gen`] reads ABI constants, flags,
//! error discriminants, and the [`bamabi_sys::offsets`] table (`OFF64_*`, `OFF32_*`,
//! `SIZE_64 = 552`, `SIZE_32 = 280`, `SLOT_COUNT = 68`) directly from [`bamabi_sys`].
//!
//! Before emitting C99 source text, [`render_c99_header`] constructs a unified 20-struct layout
//! specification table (`wire_struct_specs`) and runs two pre-emission checks:
//!
//! 1. **Rust layout verification (`verify_host_rust_layouts`):** Compares `size_of`, `align_of`,
//!    and `offset_of!` of every primitive newtype and all 20 wire structs on the current host
//!    (`64`-bit or `32`-bit) against the frozen ABI `0.3.0` layout table.
//! 2. **VTable slot count check:** Asserts that `BamHostApi` declares exactly
//!    `BAM_HOST_API_SLOT_COUNT` (`68`) function-pointer slots after `(struct_size, abi_version)`.
//!
//! The generated C99 header then emits `BAM_STATIC_ASSERT` guards for both `64`-bit
//! (`UINTPTR_MAX == 0xffffffffffffffffULL`) and `32`-bit (`UINTPTR_MAX == 0xffffffffUL`) targets,
//! handling `uint64_t` alignment differences across `32`-bit ABIs (`4`-byte on `i686` vs `8`-byte
//! on `ARM32`/`Wasm32`) via `BAM_ALIGNOF(uint64_t)`.
//!
//! ## Step 3: Generating Core Headers & Language SDKs
//!
//! - [`render_c99_header`] synthesizes the 6-section ISO C99 header (`BAM_EXPORT`, `BAM_UNUSED`,
//!   `BAM_RESTRICT`, `BAM_ALIGNOF`, `BAM_STATIC_ASSERT`, all 68 `BAM_HOST_API_OFFSET_*` macros,
//!   prefix-extensibility `BAM_*_MIN_V0_2_SIZE` / `BAM_*_MIN_V0_3_SIZE` bounds, and `bam_*`
//!   inline helpers including `SipHash-2-4-128` `bam_uuid_from_parts`, `bam_host_api_covers_slot`,
//!   and `bam_host_api_copy_bounded`).
//! - [`write_or_check_header`] powers `cargo xtask header` and `cargo xtask header --check`,
//!   normalizing CRLF to LF on read and skipping disk writes when bytes already match.
//! - [`export_sdk`] writes `bamabi.h`, `bamabi.hpp` ([`CPP17_HEADER`]), and `bamabi.zig`
//!   ([`ZIG_SDK`]) into an output directory for `cargo bam export-sdk` and polyglot consumers:
//!   - [`CPP17_HEADER`] adds `bam::offsets::*` compile-time constants, `copy_column_restricted`
//!     using `BAM_RESTRICT`, `guard_boundary`, 68-slot views (`HostContextView`, `LoadContextView`,
//!     `ChunkView`, `CommandsView`, `SystemContextView`), and RAII [`ScopedTransaction`][cpp_tx].
//!   - [`ZIG_SDK`] targets Zig `0.16` `b.addTranslateC` (`@import("bam_c")`), providing
//!     `BAM_RESTRICT_QUALIFIER = "noalias"`, `copyColumnNoAlias`, `offsets.*` /
//!     `BAM_HOST_API_OFFSET_*`, lossless 18-code `BamError` (`checkResult` / `mapError`), and
//!     68-slot view structs.
//!
//! [cpp_tx]: sdk::CPP17_HEADER
//!
//! ```rust
//! use bamabi_sdk_gen::{export_sdk, render_c99_header};
//!
//! let c99 = render_c99_header().expect("bamabi_sys layouts must be valid");
//! assert!(c99.contains("#define BAM_ABI_VERSION 0x00030000u"));
//! assert!(c99.contains("#define BAM_HOST_API_SLOT_COUNT    68u"));
//! assert!(c99.contains("#define BAM_HOST_API_OFFSET_MIGRATE_COMPONENT_COLUMN"));
//!
//! let out_dir = std::env::temp_dir().join(format!("bamabi_sdk_doctest_{}", std::process::id()));
//! let files = export_sdk(&out_dir).expect("export_sdk writes all three SDK files");
//! assert_eq!(files.len(), 3);
//! let _ = std::fs::remove_dir_all(&out_dir);
//! ```
//!
//! ## Step 4: Component Header Generation (`render_component_header`, `section_for`)
//!
//! Host crates define shared ECS components using `#[derive(ModComponent)]` (from
//! `bamabi_macros`), which emits a compile-time `ModComponent::C_DECL` string per type.
//! [`section_for`] extracts `(CANONICAL_NAME, IS_ABI_POD, C_DECL)` at compile time, and
//! [`render_component_header`] wraps those sections in include guards, `#include "<abi_header>"`,
//! and `extern "C"` blocks (used by `cargo xtask gen-component-headers` to emit
//! `include/generated/testbed_components.h`).
//!
//! ```rust
//! use bamabi_sdk_gen::render_component_header;
//!
//! // In host crates, `#[derive(ModComponent)]` generates `ModComponent::C_DECL` and
//! // `section_for::<MyComponent>()` supplies `(CANONICAL_NAME, IS_ABI_POD, C_DECL)`.
//! let sections = [(
//!     "example::Health",
//!     true,
//!     "typedef struct ExampleHealth { int32_t current; } ExampleHealth;\n",
//! )];
//!
//! let header = render_component_header(
//!     "EXAMPLE_COMPONENTS_H",
//!     "bamabi.h",
//!     &sections,
//! );
//! assert!(header.contains("#ifndef EXAMPLE_COMPONENTS_H"));
//! assert!(header.contains("#include \"bamabi.h\""));
//! assert!(header.contains("example::Health"));
//! ```
//!
//! ## Step 5: Module Map & Further Reading
//!
//! | Module / Document | Purpose |
//! |---|---|
//! | [`header_gen`] | Single-source ISO C99 header synthesizer (`render_c99_header`, `write_or_check_header`) |
//! | [`sdk`] | Polyglot SDK bundle (`CPP17_HEADER`, `ZIG_SDK`, `c99_header`, `export_sdk`) |
//! | [`component_header`] | `ModComponent::C_DECL` header renderer (`render_component_header`, `section_for`) |
//! | `crates/bamabi_sdk_gen/ARCHITECTURE.md` | Deep dive into layout specs, 32/64-bit assertions, C++17 RAII, Zig `translate-c`, and update workflows |
//! | `crates/bamabi_sdk_gen/TESTS.md` | Complete catalog of parity, compiler, Zig, and C99 ASan/UBSan tests |
//! | `crates/bamabi_sdk_gen/README.md` | Setup, build, codegen, test, and benchmarking quick reference |
//!
//! ## Step 6: Setup, Build, Running Codegen & Parity Testing
//!
//! Run all verification gates standalone inside `bamabi-wire`:
//!
//! ```bash
//! cargo fmt -p bamabi_sdk_gen -- --check
//! cargo clippy -p bamabi_sdk_gen --all-targets --all-features -- -D warnings
//! RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sdk_gen --no-deps --all-features
//! cargo test -p bamabi_sdk_gen --all-features
//! cargo test --doc -p bamabi_sdk_gen --all-features
//! cargo xtask header --check
//! cargo xtask gen-component-headers --check
//! cargo xtask c-test
//! ```

pub mod component_header;
#[cfg(doc)]
pub mod docs {
    //! Embedded architecture and test documentation for `bamabi_sdk_gen`.

    /// Crate architecture and C99/C++17/Zig emitter internals (`crates/bamabi_sdk_gen/ARCHITECTURE.md`).
    pub mod architecture {
        #![doc = include_str!("../ARCHITECTURE.md")]
    }

    /// Crate parity, C/C++/Zig compiler, and sanitizer test catalog (`crates/bamabi_sdk_gen/TESTS.md`).
    pub mod tests {
        #![doc = include_str!("../TESTS.md")]
    }
}
pub mod header_gen;
pub mod sdk;

pub use component_header::{render_component_header, section_for};
pub use header_gen::{render_c99_header, write_or_check_header};
pub use sdk::{CPP17_HEADER, ZIG_SDK, export_sdk};
