#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Deterministic, `#![forbid(unsafe_code)]`, zero-Bevy SDK generator for `bamabi`.
//!
//! Emits the canonical strict ISO C99 ABI header (`include/bamabi.h`), `#[derive(ModComponent)]`
//! C99/C++17 component headers, ISO C++17 RAII wrappers (`bamabi.hpp`), and Zig `addTranslateC`
//! (`"bam_c"` module) bindings (`bamabi.zig`) directly from `bamabi_sys` layout constants.

pub mod component_header;
pub mod header_gen;
pub mod sdk;

pub use component_header::{render_component_header, section_for};
pub use header_gen::{render_c99_header, write_or_check_header};
pub use sdk::{CPP17_HEADER, ZIG_SDK, export_sdk};
