//! Codegen accessor specification for Rust `bool` fields exposed via `FfiBool`.

use crate::codegen::{AccessorSpec, ReceiverKind, SlotSpec};
use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Type;

/// Builds the [`AccessorSpec`] (`get_<field>`, `set_<field>`) for a Rust `bool` field.
///
/// Intended purpose: prevents invalid bit-pattern UB (`2..=255`) across `extern "C"` by
/// marshaling Rust `bool` fields through `FfiBool` (`u8`) and excluding `RawInsertable`/`AbiPod`.
///
/// What it does: emits a `__BamPrimBool` compile-time type guard (preventing spoofed `bool`
/// aliases) and generates `get` (`-> FfiBool`) and `set` (`FfiBool -> FfiResult`) VTable slots.
pub(crate) fn spec(
    sys_crate: &TokenStream,
    member_tokens: &TokenStream,
    bool_ty: &Type,
    field_span: Span,
) -> AccessorSpec {
    let assertion = quote_spanned! { field_span =>
        const _: fn() = || {
            fn assert_bool_field<T: __BamPrimBool>() {}
            assert_bool_field::<#bool_ty>();
        };
    };

    AccessorSpec {
        collision_label: "accessor",
        // Rust `bool` has invalid bit patterns (`2..=255`), so structs containing `bool` omit
        // `RawInsertable` and `AbiPod` and cross the C-ABI via `FfiBool` (`u8`).
        allows_raw_insertable: false,
        allows_abi_pod: false,
        assertions: vec![assertion],
        pod_assertion: None,
        slots: vec![
            SlotSpec {
                prefix: "get",
                receiver: ReceiverKind::Const {
                    on_invalid: quote! { #sys_crate::FfiBool::FALSE },
                },
                extra_params_fn: quote! {},
                extra_params_vt: quote! {},
                ret_ty: quote! { #sys_crate::FfiBool },
                valid_body: quote! {
                    unsafe {
                        __bam_read_bool::<#bool_ty>(
                            ::core::ptr::addr_of!((*typed_ptr).#member_tokens),
                        )
                    }
                },
            },
            SlotSpec {
                prefix: "set",
                receiver: ReceiverKind::Mut {
                    on_null: quote! { #sys_crate::FfiResult::NULL_POINTER },
                    pre_align_check: None,
                    on_unaligned: quote! { #sys_crate::FfiResult::INVALID_ARGUMENT },
                },
                extra_params_fn: quote! { , val: #sys_crate::FfiBool },
                extra_params_vt: quote! { , #sys_crate::FfiBool },
                ret_ty: quote! { #sys_crate::FfiResult },
                valid_body: quote! {
                    unsafe {
                        __bam_write_bool::<#bool_ty>(
                            ::core::ptr::addr_of_mut!((*typed_ptr).#member_tokens),
                            val,
                        )
                    }
                },
            },
        ],
    }
}
