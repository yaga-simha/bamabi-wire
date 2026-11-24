//! Codegen accessor specification for host-owned `std::string::String` fields.

use crate::codegen::{AccessorSpec, ReceiverKind, SlotSpec};
use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Type;

/// Builds the [`AccessorSpec`] (`get_<field>`, `set_<field>`) for a host-owned `String` field.
///
/// Intended purpose: enforces Bamabi's strict allocator isolation invariant—guest mods read UTF-8
/// via `StrView` and mutate via alias-safe host-allocator replacement without touching `String` internals.
///
/// What it does: emits a `__BamStdString` anti-spoofing guard, marks the parent struct non-POD
/// and non-`RawInsertable`, and generates `get` (`-> StrView`) and `set` (`StrView -> FfiResult`) slots.
pub(crate) fn spec(
    sys_crate: &TokenStream,
    member_tokens: &TokenStream,
    str_ty: &Type,
    field_span: Span,
) -> AccessorSpec {
    let assertion = quote_spanned! { field_span =>
        const _: fn() = || {
            fn assert_std_string_field<T: __BamStdString>() {}
            assert_std_string_field::<#str_ty>();
        };
    };

    AccessorSpec {
        collision_label: "accessor",
        allows_raw_insertable: false,
        allows_abi_pod: false,
        assertions: vec![assertion],
        pod_assertion: None,
        slots: vec![
            SlotSpec {
                prefix: "get",
                receiver: ReceiverKind::Const {
                    on_invalid: quote! { #sys_crate::FfiResult::NULL_POINTER },
                },
                extra_params_fn: quote! { , out_view: *mut #sys_crate::StrView },
                extra_params_vt: quote! { , *mut #sys_crate::StrView },
                ret_ty: quote! { #sys_crate::FfiResult },
                valid_body: quote! {
                    if out_view.is_null() {
                        return #sys_crate::FfiResult::NULL_POINTER;
                    }
                    if !out_view.is_aligned() {
                        return #sys_crate::FfiResult::INVALID_ARGUMENT;
                    }
                    let view = unsafe {
                        __bam_read_string::<#str_ty>(
                            ::core::ptr::addr_of!((*typed_ptr).#member_tokens),
                        )
                    };
                    unsafe { ::core::ptr::write(out_view, view) };
                    #sys_crate::FfiResult::OK
                },
            },
            SlotSpec {
                prefix: "set",
                receiver: ReceiverKind::Mut {
                    on_null: quote! { #sys_crate::FfiResult::NULL_POINTER },
                    pre_align_check: Some(quote! {
                        if view.is_null() {
                            return #sys_crate::FfiResult::NULL_POINTER;
                        }
                        if !view.is_aligned() {
                            return #sys_crate::FfiResult::INVALID_ARGUMENT;
                        }
                    }),
                    on_unaligned: quote! { #sys_crate::FfiResult::INVALID_ARGUMENT },
                },
                extra_params_fn: quote! { , view: *const #sys_crate::StrView },
                extra_params_vt: quote! { , *const #sys_crate::StrView },
                ret_ty: quote! { #sys_crate::FfiResult },
                valid_body: quote! {
                    let view_val = unsafe { ::core::ptr::read(view) };
                    unsafe {
                        __bam_write_string::<#str_ty>(
                            ::core::ptr::addr_of_mut!((*typed_ptr).#member_tokens),
                            view_val,
                        )
                    }
                },
            },
        ],
    }
}
