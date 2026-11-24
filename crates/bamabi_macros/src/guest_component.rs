//! `#[derive(GuestComponent)]` zero-overhead guest component metadata generator.
//!
//! Synthesizes `bamabi_guest::GuestComponent` implementations with compile-time SipHash-2-4-128
//! field-schema hashing and storage flags (`sparse_set`, `immutable`, `summary_tick`).

use std::ffi::CString;

use crate::attrs::FieldAttrOverride;
use crate::attrs::{
    parse_guest_component_attrs, parse_repr_flags, resolve_guest_crate_path, resolve_sys_crate_path,
};
use crate::classify::{canonical_bam_field_kind_ident, canonical_field_type_tag, classify_type};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Index, LitCStr};

/// Entry point for `#[derive(GuestComponent)]` proc-macro expansion.
///
/// Intended purpose: generates a zero-runtime-allocation `GuestComponent` impl for an `AbiPod`
/// guest struct so it can be registered and queried through `bamabi_guest` without manual
/// `ComponentDesc` boilerplate.
///
/// What it does: delegates to [`expand_guest_component_inner`] using crate paths resolved from
/// `Cargo.toml` or explicit `#[guest_component(crate = ..., guest_crate = ...)]` overrides.
pub(crate) fn expand_guest_component(input: &DeriveInput) -> syn::Result<TokenStream> {
    expand_guest_component_inner(input, None, None)
}

/// Test-only entry point for `#[derive(GuestComponent)]` with injected crate paths.
///
/// Intended purpose: allows unit tests in `src/tests.rs` to inspect expanded token streams
/// deterministically without depending on `proc-macro-crate` manifest lookups.
///
/// What it does: forwards explicit `sys_crate` and `guest_crate` path tokens into
/// [`expand_guest_component_inner`].
#[cfg(test)]
pub(crate) fn expand_guest_component_with_crates(
    input: &DeriveInput,
    sys_crate: &TokenStream,
    guest_crate: &TokenStream,
) -> syn::Result<TokenStream> {
    expand_guest_component_inner(input, Some(sys_crate.clone()), Some(guest_crate.clone()))
}

/// Core implementation of `#[derive(GuestComponent)]`.
///
/// Intended purpose: enforces that guest-defined components have deterministic non-packed C
/// layouts and emit stable canonical C-string names, field-schema hashes, and storage flags.
///
/// What it does: rejects generics, enums/unions, `#[repr(packed)]`, and missing
/// `#[repr(C)]`/`#[repr(transparent)]`; validates `#[guest_component(...)]` attributes
/// (including rejecting `sparse_set` combined with `summary_tick`); builds the `NAME` `CStr`
/// literal, `FIELD_SCHEMA_HASH` chain over `offset_of!`, bitmask `FLAGS`, and optional explicit
/// `UUID`; and emits `impl GuestComponent for <Struct>`.
fn expand_guest_component_inner(
    input: &DeriveInput,
    sys_crate_override: Option<TokenStream>,
    guest_crate_override: Option<TokenStream>,
) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`#[derive(GuestComponent)]` does not support generic structs",
        ));
    }

    let fields = match &input.data {
        Data::Struct(data_struct) => &data_struct.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(GuestComponent)]` can only be applied to structs (`GuestComponent` requires `AbiPod`)",
            ));
        }
    };

    let attrs = parse_guest_component_attrs(input)?;
    let repr_flags = parse_repr_flags(&input.attrs)?;
    if repr_flags.has_packed {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(GuestComponent)]` cannot be applied to `#[repr(packed)]` structs because `GuestComponent` requires `AbiPod`",
        ));
    }
    if !repr_flags.has_c && !repr_flags.has_transparent {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(GuestComponent)]` requires `#[repr(C)]` or `#[repr(transparent)]` to guarantee deterministic cross-language memory layout",
        ));
    }

    let sys_crate =
        sys_crate_override.unwrap_or_else(|| resolve_sys_crate_path(attrs.crate_override.as_ref()));
    let guest_crate = guest_crate_override
        .unwrap_or_else(|| resolve_guest_crate_path(attrs.guest_crate_override.as_ref()));

    let struct_ident = &input.ident;
    let base_name = attrs
        .custom_name
        .unwrap_or_else(|| struct_ident.to_string());
    let full_name = match attrs.custom_namespace {
        Some(ns) => format!("{ns}::{base_name}"),
        None => base_name,
    };
    let name_cstr = CString::new(full_name.as_bytes()).map_err(|_| {
        syn::Error::new_spanned(
            struct_ident,
            "component name must not contain interior NUL bytes",
        )
    })?;
    let name_lit = LitCStr::new(&name_cstr, struct_ident.span());

    let mut bam_field_entries = Vec::new();
    let mut schema_fp_updates = Vec::new();
    let mut schema_hash_expr = quote! { 0u64 };

    let mut push_field = |field_name: String, member_toks: TokenStream, field: &syn::Field| {
        let field_ty = &field.ty;
        let field_tag = canonical_field_type_tag(field_ty);
        let kind_ident_str = match classify_type(field_ty, FieldAttrOverride::None) {
            Ok(ref kind) => canonical_bam_field_kind_ident(kind, field_ty),
            Err(_) => "NESTED",
        };
        let kind_ident = format_ident!("{}", kind_ident_str);

        if !attrs.no_schema_hash {
            schema_hash_expr = quote! {
                #sys_crate::ComponentUuid::hash_field_schema(
                    #schema_hash_expr,
                    #field_name,
                    #field_tag,
                    ::core::mem::offset_of!(#struct_ident, #member_toks),
                )
            };
        }

        bam_field_entries.push(quote! {
            #sys_crate::BamFieldDesc::new(
                #sys_crate::BamFieldKind::#kind_ident,
                #sys_crate::StrView::from_str(#field_name),
                #sys_crate::StrView::from_str(#field_tag),
                ::core::mem::offset_of!(#struct_ident, #member_toks),
                ::core::mem::size_of::<#field_ty>(),
                ::core::mem::align_of::<#field_ty>(),
            )
        });
        schema_fp_updates.push(quote! {
            h = #sys_crate::fnv1a_64_update_u64(h, (#sys_crate::BamFieldKind::#kind_ident).0 as u64);
            h = #sys_crate::fnv1a_64_update_bytes(h, #field_name.as_bytes());
            h = #sys_crate::fnv1a_64_update_bytes(h, #field_tag.as_bytes());
            h = #sys_crate::fnv1a_64_update_u64(
                h,
                ::core::mem::offset_of!(#struct_ident, #member_toks) as u64,
            );
            h = #sys_crate::fnv1a_64_update_u64(h, ::core::mem::size_of::<#field_ty>() as u64);
            h = #sys_crate::fnv1a_64_update_u64(h, ::core::mem::align_of::<#field_ty>() as u64);
        });
    };

    match fields {
        Fields::Named(named) => {
            for field in &named.named {
                let Some(ident) = field.ident.as_ref() else {
                    continue;
                };
                push_field(ident.to_string(), quote! { #ident }, field);
            }
        }
        Fields::Unnamed(unnamed) => {
            for (idx, field) in unnamed.unnamed.iter().enumerate() {
                let index = Index::from(idx);
                push_field(idx.to_string(), quote! { #index }, field);
            }
        }
        Fields::Unit => {}
    }

    let schema_expr = if attrs.no_schema_hash {
        quote! { 0u64 }
    } else {
        schema_hash_expr
    };

    let mut flag_terms = Vec::new();
    if attrs.sparse_set {
        flag_terms.push(quote! { #sys_crate::BAM_COMPONENT_FLAG_SPARSE_SET });
    }
    if attrs.immutable {
        flag_terms.push(quote! { #sys_crate::BAM_COMPONENT_FLAG_IMMUTABLE });
    }
    if attrs.summary_tick {
        flag_terms.push(quote! { #sys_crate::BAM_COMPONENT_FLAG_SUMMARY_TICK });
    }
    let flags_expr = if flag_terms.is_empty() {
        quote! { 0u32 }
    } else {
        quote! { #(#flag_terms)|* }
    };

    let uuid_item = match attrs.custom_uuid.as_deref() {
        Some(hex_str) => {
            let bytes = bamabi_schema::try_parse_uuid_hex(hex_str).ok_or_else(|| {
                syn::Error::new_spanned(
                    &input.ident,
                    "invalid `uuid` literal in `#[guest_component]`",
                )
            })?;
            quote! {
                const UUID: #sys_crate::ComponentUuid =
                    #sys_crate::ComponentUuid::from_bytes([#(#bytes),*]);
            }
        }
        None => quote! {},
    };

    let fingerprint_expr = match attrs.custom_schema_fingerprint {
        Some(custom_fp) => quote! { #custom_fp },
        None => quote! {
            {
                let mut h = #sys_crate::fnv1a_64_init();
                h = #sys_crate::fnv1a_64_update_bytes(h, #full_name.as_bytes());
                h = #sys_crate::fnv1a_64_update_u64(h, ::core::mem::size_of::<#struct_ident>() as u64);
                h = #sys_crate::fnv1a_64_update_u64(h, ::core::mem::align_of::<#struct_ident>() as u64);
                h = #sys_crate::fnv1a_64_update_u64(h, (#flags_expr) as u64);
                #(#schema_fp_updates)*
                h
            }
        },
    };

    Ok(quote! {
        const _: () = {
            static GUEST_BAM_FIELDS: &[#sys_crate::BamFieldDesc] = &[
                #(#bam_field_entries),*
            ];

            const GUEST_SCHEMA_FINGERPRINT: u64 = #fingerprint_expr;

            impl #guest_crate::GuestComponent for #struct_ident {
                const NAME: &'static ::core::ffi::CStr = #name_lit;
                const FIELD_SCHEMA_HASH: u64 = #schema_expr;
                const FLAGS: u32 = #flags_expr;
                #uuid_item
                const SCHEMA_FINGERPRINT: u64 = GUEST_SCHEMA_FINGERPRINT;
                const BAM_FIELDS: &'static [#sys_crate::BamFieldDesc] = GUEST_BAM_FIELDS;
            }
        };
    })
}
