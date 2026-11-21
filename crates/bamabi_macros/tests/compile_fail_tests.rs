#![allow(unreachable_pub)]

//! `trybuild` UI compile-fail and positive-control test suite for `#[derive(AbiPod)]`,
//! `#[derive(ModComponent)]`, and `#[derive(GuestComponent)]`.
//!
//! ## UI Fixture Scenario Catalog (`tests/ui/*.rs`)
//!
//! Note: individual `.rs` fixtures under `tests/ui/` are intentionally kept unmodified so
//! `trybuild` `.stderr` line numbers and column spans remain exact. All 38 UI scenarios are
//! documented below:
//!
//! ### Positive Controls (`pass_*.rs`)
//! - `pass_abi_pod_const_generic.rs`: Verifies `#[derive(AbiPod)]` accepts `const` generic
//!   structs (`FixedBuf<const N: usize>`, defaulted const params, `#[repr(transparent)]`, and
//!   multi-const-generic `Grid<const ROWS: usize, const COLS: usize>`).
//! - `pass_valid_abi_pod_and_mod_component.rs`: Verifies valid `#[repr(C)]` `AbiPod` structs
//!   (explicit padding, primitives, nested `AbiPod`) and `#[derive(ModComponent)]` structs/enums
//!   compile cleanly and satisfy `assert_abi_pod` / `assert_raw_insertable`.
//! - `pass_guest_component.rs`: Verifies `#[derive(GuestComponent)]` with default and custom
//!   `name`, `namespace`, `uuid`, `sparse_set`, `immutable`, `summary_tick`, and `no_schema_hash`
//!   options compiles and produces expected `GuestComponent` constants.
//!
//! ### `#[derive(AbiPod)]` Compile-Fail Scenarios (`abi_pod_*.rs`)
//! - `abi_pod_bool_field.rs`: Rejects Rust `bool` field (non-universal bit validity; use `FfiBool`).
//! - `abi_pod_const_generic_conditional_padding.rs`: Rejects const-generic struct that is unpadded at `N = 0` but padded at `N = 1`.
//! - `abi_pod_const_generic_padded.rs`: Rejects const-generic struct with implicit interior padding.
//! - `abi_pod_direct_drop.rs`: Rejects struct implementing `Drop` (`!needs_drop` violation).
//! - `abi_pod_enum.rs`: Rejects plain Rust enums (discriminant/tag layout not POD).
//! - `abi_pod_generic.rs`: Rejects type-generic structs (only `const` generics are supported).
//! - `abi_pod_implicit_padding.rs`: Rejects interior compiler padding (`size_of != sum(fields)`).
//! - `abi_pod_lifetime_generic.rs`: Rejects lifetime-generic structs (only `const` generics are supported).
//! - `abi_pod_nested_drop_field.rs`: Rejects struct containing a nested field with `Drop` glue.
//! - `abi_pod_no_repr_c.rs`: Rejects default `#[repr(Rust)]` struct without `#[repr(C)]`/`transparent`.
//! - `abi_pod_repr_c_packed.rs`: Rejects `#[repr(C, packed)]` (unaligned field references are UB).
//! - `abi_pod_string_field.rs`: Rejects heap-allocated `String` field.
//! - `abi_pod_tagged_enum.rs`: Rejects tagged `#[repr(C, u32)]` enum in `AbiPod`.
//! - `abi_pod_trailing_padding.rs`: Rejects trailing compiler padding (`size_of != sum(fields)`).
//! - `abi_pod_u128_field.rs`: Rejects `u128` field (divergent C/LLVM alignment across compilers).
//! - `abi_pod_usize_field.rs`: Rejects `usize` field (pointer-width dependent, not cross-arch POD).
//! - `abi_pod_vec_u8_field.rs`: Rejects heap-allocated `Vec<u8>` field.
//!
//! ### `#[derive(ModComponent)]` Compile-Fail Scenarios (`mod_component_*.rs`)
//! - `mod_component_bool_not_abi_pod.rs`: Verifies `ModComponent` containing `bool` does NOT impl `AbiPod`.
//! - `mod_component_bool_not_raw_insertable.rs`: Verifies `ModComponent` containing `bool` does NOT impl `RawInsertable`.
//! - `mod_component_enum_not_abi_pod.rs`: Verifies `ModComponent` enum does NOT impl `AbiPod`.
//! - `mod_component_ffi_option_bool_rejected.rs`: Rejects `FfiOption<bool>` (inner type must be `AbiPod`).
//! - `mod_component_guest_non_pod.rs`: Rejects `#[mod_component(guest)]` on a struct with non-POD `String` (`GuestComponent: AbiPod`).
//! - `mod_component_implicit_padding.rs`: Rejects implicit padding in a `#[repr(C)]` POD `ModComponent`.
//! - `mod_component_invalid_uuid_rejected.rs`: Rejects malformed `#[mod_component(uuid = "...")]` hex literal.
//! - `mod_component_no_pod_suppresses_abi_pod.rs`: Verifies `#[mod_component(no_pod)]` suppresses `AbiPod` impl.
//! - `mod_component_non_opaque_nested_string_rejected.rs`: Rejects non-opaque nested `String` without `ModComponent` VTable.
//! - `mod_component_spoofed_string_rejected.rs`: Rejects user-defined `struct String` attempting to spoof `std::string::String`.
//! - `mod_component_spoofed_vec_rejected.rs`: Rejects user-defined `struct Vec<T>` attempting to spoof `std::vec::Vec<T>`.
//! - `mod_component_string_not_abi_pod.rs`: Verifies `ModComponent` with `String` does NOT impl `AbiPod`.
//! - `mod_component_string_not_raw_insertable.rs`: Verifies `ModComponent` with `String` does NOT impl `RawInsertable`.
//! - `mod_component_vec_of_bool_comp_rejected.rs`: Rejects `Vec<bool>` field (element must be `AbiPod`).
//! - `mod_component_vec_of_repr_rust_rejected.rs`: Rejects `Vec<ReprRust>` field (element must be `AbiPod`).
//! - `mod_component_vec_of_string_comp_rejected.rs`: Rejects `Vec<String>` field (element must be `AbiPod`).
//!
//! ### `#[derive(GuestComponent)]` Compile-Fail Scenarios (`guest_component_*.rs`)
//! - `guest_component_enum_rejected.rs`: Rejects applying `#[derive(GuestComponent)]` to an enum.
//! - `guest_component_sparse_summary_tick.rs`: Rejects combining `sparse_set` and `summary_tick` flags.

use bamabi_macros::{AbiPod, ModComponent};
use bamabi_sys::ModComponent as _;

/// Minimal `AbiPod` struct linking `bamabi_macros` and `bamabi_sys` into the `trybuild` target.
#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
struct LinkAnchorPod {
    x: f32,
}

/// Minimal `ModComponent` struct wrapping [`LinkAnchorPod`] to keep derive traits live in test builds.
#[repr(C)]
#[derive(Copy, Clone, ModComponent)]
struct LinkAnchorComp {
    val: LinkAnchorPod,
}

/// Test scenario: verifies that `LinkAnchorPod` and `LinkAnchorComp` satisfy `AbiPod`,
/// `RawInsertable`, and canonical `ModComponent::CANONICAL_NAME` derivation at runtime.
#[test]
fn link_anchor_traits_are_live() {
    bamabi_sys::assert_abi_pod::<LinkAnchorPod>();
    bamabi_sys::assert_abi_pod::<LinkAnchorComp>();
    bamabi_sys::assert_raw_insertable::<LinkAnchorComp>();
    assert_eq!(
        LinkAnchorComp::CANONICAL_NAME,
        "compile_fail_tests::LinkAnchorComp"
    );
}

/// Test scenario: executes the full 34-fixture `trybuild` UI test matrix (`pass_*.rs`,
/// `abi_pod_*.rs`, `mod_component_*.rs`, and `guest_component_*.rs`) verifying exact compiler
/// acceptance and diagnostic rejection for every macro safety and layout rule.
#[test]
#[cfg_attr(miri, ignore)]
fn ui_compile_fail_and_pass_suite() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_*.rs");
    t.compile_fail("tests/ui/abi_pod_*.rs");
    t.compile_fail("tests/ui/mod_component_*.rs");
    t.compile_fail("tests/ui/guest_component_*.rs");
}
