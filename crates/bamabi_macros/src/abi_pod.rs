//! `#[derive(AbiPod)]` compile-time layout and safety verifier.
//!
//! Enforces Bamabi's cross-language Plain-Old-Data invariant so components can be mapped
//! zero-copy across C99, C++17, Zig, and Rust guest boundaries without serialization.

use crate::attrs::{parse_abi_pod_crate_override, parse_repr_flags, resolve_sys_crate_path};
use crate::classify::{is_bool_type, is_char_type};
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{ConstParam, Data, DeriveInput, GenericParam, Type, spanned::Spanned};

/// Returns `(zero_sample, one_sample)` compile-time argument literals for a `const` generic
/// parameter so `check_layout` can verify zero implicit padding across all affine/bilinear
/// combinations of const-generic dimensions.
fn const_param_sample_pair(ty: &Type) -> (TokenStream, TokenStream) {
    if is_bool_type(ty) {
        (quote! { false }, quote! { true })
    } else if is_char_type(ty) {
        (quote! { '\0' }, quote! { '\x01' })
    } else {
        (quote! { 0 }, quote! { 1 })
    }
}

/// Expands `#[derive(AbiPod)]` into compile-time layout assertions and `RawInsertable`/`AbiPod` impls.
///
/// Intended purpose: guarantees at compile time that a struct has a deterministic C ABI layout
/// with zero implicit compiler padding, zero drop glue, and only cross-language POD fields
/// (excluding `bool`, `char`, `u128`/`i128`, `usize`/`isize`, heap types, and enums).
///
/// What it does: rejects type/lifetime generic parameters (while allowing `const` generic
/// parameters such as `const N: usize`), non-structs, `#[repr(packed)]`, and missing
/// `#[repr(C)]`/`#[repr(transparent)]` attributes, then emits a `const _: ()` block that asserts
/// `Copy + 'static`, `!needs_drop::<Self>()`, `size_of::<Self>() == sum(size_of::<Field>())`,
/// and recursive `assert_abi_pod::<Field>()` before emitting `unsafe impl RawInsertable` and
/// `unsafe impl AbiPod`.
pub(crate) fn expand_abi_pod(input: &DeriveInput) -> syn::Result<TokenStream> {
    let mut const_params: Vec<&ConstParam> = Vec::new();
    for param in &input.generics.params {
        match param {
            GenericParam::Const(cp) => const_params.push(cp),
            GenericParam::Type(_) | GenericParam::Lifetime(_) => {
                return Err(syn::Error::new_spanned(
                    param,
                    "`#[derive(AbiPod)]` does not support type or lifetime generic parameters (only `const` generic parameters are supported)",
                ));
            }
        }
    }

    let fields = match &input.data {
        Data::Struct(data_struct) => &data_struct.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(AbiPod)]` can only be applied to structs",
            ));
        }
    };

    let crate_override = parse_abi_pod_crate_override(input)?;
    let sys_crate = resolve_sys_crate_path(crate_override.as_ref());

    let repr_flags = parse_repr_flags(&input.attrs)?;
    if repr_flags.has_packed {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(AbiPod)]` cannot be applied to `#[repr(packed)]` structs because unaligned field references are undefined behavior",
        ));
    }
    if !repr_flags.has_c && !repr_flags.has_transparent {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(AbiPod)]` requires `#[repr(C)]` or `#[repr(transparent)]` to guarantee deterministic cross-language memory layout",
        ));
    }

    let struct_ident = &input.ident;
    let mut field_assertions = Vec::new();
    let mut field_size_sum = quote! { 0usize };
    for field in fields.iter() {
        let field_ty = &field.ty;
        let span = field_ty.span();
        field_assertions.push(quote_spanned! { span =>
            #sys_crate::assert_abi_pod::<#field_ty>();
        });
        field_size_sum = quote! { #field_size_sum + ::core::mem::size_of::<#field_ty>() };
    }

    if const_params.is_empty() {
        return Ok(quote! {
            const _: () = {
                fn assert_self_copy<T: ::core::marker::Copy + 'static>() {}
                fn check_all() {
                    assert_self_copy::<#struct_ident>();
                    #(#field_assertions)*
                }
                ::core::assert!(!::core::mem::needs_drop::<#struct_ident>());
                ::core::assert!(
                    ::core::mem::size_of::<#struct_ident>() == (#field_size_sum),
                    "`#[derive(AbiPod)]` requires zero implicit compiler padding; insert explicit `_pad` fields"
                );

                // SAFETY: Verified `#[repr(C)]` / `#[repr(transparent)]` non-packed struct, `Copy + 'static`,
                // zero drop glue, zero implicit compiler padding, and every field verified `AbiPod` via `assert_abi_pod` above.
                #[allow(clippy::undocumented_unsafe_blocks)]
                unsafe impl #sys_crate::RawInsertable for #struct_ident {}

                // SAFETY: All cross-language `AbiPod` layout and bit-validity invariants are verified above
                // (`u128`, `i128`, `usize`, `isize`, `bool`, `char`, heap types, and enums are rejected).
                #[allow(clippy::undocumented_unsafe_blocks)]
                unsafe impl #sys_crate::AbiPod for #struct_ident {}
            };
        });
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let samples: Vec<(TokenStream, TokenStream)> = const_params
        .iter()
        .map(|cp| const_param_sample_pair(&cp.ty))
        .collect();

    let mut layout_check_calls: Vec<TokenStream> = Vec::with_capacity(samples.len() + 2);
    let all_zeros: Vec<&TokenStream> = samples.iter().map(|(z, _)| z).collect();
    layout_check_calls.push(quote! {
        check_layout::<#(#all_zeros),*>();
    });
    for idx in 0..samples.len() {
        let args: Vec<&TokenStream> = samples
            .iter()
            .enumerate()
            .map(|(j, (z, o))| if j == idx { o } else { z })
            .collect();
        layout_check_calls.push(quote! {
            check_layout::<#(#args),*>();
        });
    }
    if samples.len() >= 2 {
        let all_ones: Vec<&TokenStream> = samples.iter().map(|(_, o)| o).collect();
        layout_check_calls.push(quote! {
            check_layout::<#(#all_ones),*>();
        });
    }

    Ok(quote! {
        const _: () = {
            fn assert_self_copy<T: ::core::marker::Copy + 'static>() {}
            fn check_all #impl_generics () #where_clause {
                assert_self_copy::<#struct_ident #ty_generics>();
                #(#field_assertions)*
            }
            const fn check_layout #impl_generics () #where_clause {
                ::core::assert!(!::core::mem::needs_drop::<#struct_ident #ty_generics>());
                ::core::assert!(
                    ::core::mem::size_of::<#struct_ident #ty_generics>() == (#field_size_sum),
                    "`#[derive(AbiPod)]` requires zero implicit compiler padding; insert explicit `_pad` fields"
                );
            }
            #(#layout_check_calls)*

            // SAFETY: Verified `#[repr(C)]` / `#[repr(transparent)]` non-packed struct, `Copy + 'static`,
            // zero drop glue, zero implicit compiler padding across all const-generic dimensions,
            // and every field verified `AbiPod` via `assert_abi_pod` above.
            #[allow(clippy::undocumented_unsafe_blocks)]
            unsafe impl #impl_generics #sys_crate::RawInsertable for #struct_ident #ty_generics #where_clause {}

            // SAFETY: All cross-language `AbiPod` layout and bit-validity invariants are verified above
            // (`u128`, `i128`, `usize`, `isize`, `bool`, `char`, heap types, and enums are rejected).
            #[allow(clippy::undocumented_unsafe_blocks)]
            unsafe impl #impl_generics #sys_crate::AbiPod for #struct_ident #ty_generics #where_clause {}
        };
    })
}
