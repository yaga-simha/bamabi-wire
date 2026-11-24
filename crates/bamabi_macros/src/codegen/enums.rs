//! `#[derive(ModComponent)]` code generation for fieldless and data-carrying enums.

use crate::attrs::{IntRepr, ReprFlags, parse_field_attr, to_snake_case};
use crate::classify::{
    FieldKind, canonical_field_type_tag, canonical_kind_type_tag, classify_type,
};
use crate::codegen::c_decl::{DeclEnumInfo, DeclEnumVariant, DeclEnumVariantField, DeclFieldKind};
use crate::codegen::witness::{
    WitnessEnumVariant, WitnessEnumVariantField, WitnessFieldKind, emit_enum_witnesses,
};
use crate::codegen::{CodegenState, vtable};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{DataEnum, DeriveInput, Expr, Fields, Ident, Lit, Type, UnOp, spanned::Spanned as _};

/// Evaluates a literal or negated literal enum discriminant expression into an `i128`.
///
/// Intended purpose: enforces deterministic compile-time discriminants for C99/Zig header
/// emission and `IntRepr` range checks without evaluating arbitrary Rust const expressions.
fn parse_discriminant_i128(expr: &Expr) -> syn::Result<i128> {
    match expr {
        Expr::Group(g) => parse_discriminant_i128(&g.expr),
        Expr::Paren(p) => parse_discriminant_i128(&p.expr),
        Expr::Lit(expr_lit) => {
            if let Lit::Int(lit_int) = &expr_lit.lit {
                let val = lit_int.base10_parse::<i128>().map_err(|e| {
                    syn::Error::new_spanned(
                        lit_int,
                        format!("integer discriminant out of range for i128: {e}"),
                    )
                })?;
                Ok(val)
            } else {
                Err(syn::Error::new_spanned(
                    expr,
                    "enum discriminants in `#[derive(ModComponent)]` must be integer literals",
                ))
            }
        }
        Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => {
            if let Expr::Lit(expr_lit) = &*unary.expr
                && let Lit::Int(lit_int) = &expr_lit.lit
            {
                let mag = lit_int.base10_parse::<u128>().map_err(|e| {
                    syn::Error::new_spanned(lit_int, format!("invalid integer discriminant: {e}"))
                })?;
                if mag == (i128::MAX as u128) + 1 {
                    return Ok(i128::MIN);
                }
                if mag > i128::MAX as u128 {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "negated enum discriminant out of i128 range",
                    ));
                }
                return Ok(-(mag as i128));
            }
            Err(syn::Error::new_spanned(
                expr,
                "enum discriminants in `#[derive(ModComponent)]` must be integer literals",
            ))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "enum discriminants in `#[derive(ModComponent)]` must be integer literals for deterministic C99/Zig header generation",
        )),
    }
}

/// Formats an `i128` discriminant value with its concrete `IntRepr` integer suffix (e.g. `42u32`).
fn format_discriminant_tokens(
    val: i128,
    int_repr: IntRepr,
    span: Span,
) -> syn::Result<TokenStream> {
    let suffix = int_repr.rust_ty_str();
    let lit_str = format!("{val}{suffix}");
    lit_str.parse::<TokenStream>().map_err(|_| {
        syn::Error::new(
            span,
            format!("internal error: invalid integer discriminant `{lit_str}`"),
        )
    })
}

/// Classified payload field metadata for a single field within a data-carrying enum variant.
struct EnumFieldInfo {
    field_name: String,
    rust_ident: Option<Ident>,
    kind: FieldKind,
    raw_ty: Type,
    span: Span,
}

/// Parsed variant representation produced before emitting enum VTable slots and witnesses.
struct ParsedEnumVariant<'a> {
    variant: &'a syn::Variant,
    disc_val: i128,
    is_unit: bool,
    is_named: bool,
    has_skipped: bool,
    fields_info: Vec<EnumFieldInfo>,
}

/// Validates enum `#[repr(...)]` constraints and parses all variant discriminants and fields.
fn parse_enum_variants<'a>(
    input: &DeriveInput,
    data_enum: &'a DataEnum,
    repr_flags: &ReprFlags,
    is_fieldless: bool,
) -> syn::Result<Vec<ParsedEnumVariant<'a>>> {
    if is_fieldless {
        if !repr_flags.has_c && !repr_flags.int_repr.is_some() {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(ModComponent)]` on fieldless enums requires `#[repr(u8|u16|u32|u64|i8|i16|i32|i64)]` or `#[repr(C)]`",
            ));
        }
    } else if !repr_flags.has_c || !repr_flags.int_repr.is_some() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(ModComponent)]` on data-carrying enums requires `#[repr(C, u8|u16|u32|u64|i8|i16|i32|i64)]`",
        ));
    }

    let int_repr = repr_flags.int_repr;
    let mut prev_disc: Option<i128> = None;
    let mut parsed_variants = Vec::with_capacity(data_enum.variants.len());

    for variant in &data_enum.variants {
        let disc_val = match &variant.discriminant {
            Some((_, expr)) => parse_discriminant_i128(expr)?,
            None => match prev_disc {
                Some(prev) => prev.checked_add(1).ok_or_else(|| {
                    syn::Error::new_spanned(&variant.ident, "enum discriminant overflow")
                })?,
                None => 0,
            },
        };
        if !int_repr.fits_i128(disc_val) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                format!(
                    "enum discriminant `{disc_val}` does not fit in `#[repr({})]`",
                    int_repr.rust_ty_str()
                ),
            ));
        }
        prev_disc = Some(disc_val);

        let mut fields_info = Vec::new();
        let mut has_skipped = false;
        let is_named = matches!(variant.fields, Fields::Named(_));
        let is_unit = matches!(variant.fields, Fields::Unit);

        for (idx, f) in variant.fields.iter().enumerate() {
            let attr_override = parse_field_attr(&f.attrs)?;
            let kind = classify_type(&f.ty, attr_override)?;
            let field_name = match &f.ident {
                Some(id) => id.to_string(),
                None => idx.to_string(),
            };
            if matches!(kind, FieldKind::Skip) {
                has_skipped = true;
            }
            fields_info.push(EnumFieldInfo {
                field_name,
                rust_ident: f.ident.clone(),
                kind,
                raw_ty: f.ty.clone(),
                span: f.ty.span(),
            });
        }

        parsed_variants.push(ParsedEnumVariant {
            variant,
            disc_val,
            is_unit,
            is_named,
            has_skipped,
            fields_info,
        });
    }

    Ok(parsed_variants)
}

/// Emits compile-time discriminant/size assertions and the C-ABI `tag` slot for an enum.
fn emit_enum_tag_slot(
    state: &mut CodegenState,
    parsed_variants: &[ParsedEnumVariant<'_>],
    is_fieldless: bool,
    int_repr: IntRepr,
) -> syn::Result<()> {
    let enum_ident = state.struct_ident.clone();
    let sys_crate = state.sys_crate.clone();
    let tag_ty_ident = Ident::new(int_repr.rust_ty_str(), Span::call_site());

    if is_fieldless {
        let mut disc_asserts = Vec::new();
        for pv in parsed_variants {
            let v_ident = &pv.variant.ident;
            let disc_tok = format_discriminant_tokens(pv.disc_val, int_repr, v_ident.span())?;
            disc_asserts.push(quote! {
                ::core::assert!((#enum_ident::#v_ident as #tag_ty_ident) == #disc_tok);
            });
        }
        state.layout_assertions.push(quote! {
            const _: () = {
                ::core::assert!(::core::mem::size_of::<#enum_ident>() == ::core::mem::size_of::<#tag_ty_ident>());
                #(#disc_asserts)*
            };
        });
    } else {
        state.layout_assertions.push(quote! {
            const _: () = {
                ::core::assert!(::core::mem::size_of::<#enum_ident>() >= ::core::mem::size_of::<#tag_ty_ident>());
            };
        });
    }

    let mut tag_arms = Vec::new();
    for pv in parsed_variants {
        let v_ident = &pv.variant.ident;
        let disc_tok = format_discriminant_tokens(pv.disc_val, int_repr, v_ident.span())?;
        let pat = if pv.is_unit {
            quote! { #enum_ident::#v_ident }
        } else if pv.is_named {
            quote! { #enum_ident::#v_ident { .. } }
        } else {
            quote! { #enum_ident::#v_ident(..) }
        };
        tag_arms.push(quote! { #pat => #disc_tok, });
    }

    state.used_vtable_fields.insert("tag".to_owned());
    let tag_export_name = format!("{}_tag", state.export_fn_prefix);
    state.used_export_names.insert(tag_export_name.clone());
    let enum_snake = crate::attrs::to_snake_case(&enum_ident.to_string());
    let tag_fn_ident = format_ident!("__bam_{}_tag", enum_snake);
    let tag_export_attr = if state.export_symbols {
        quote! {
            #[allow(unreachable_pub)]
            #[unsafe(export_name = #tag_export_name)]
            pub
        }
    } else {
        quote! {}
    };
    let tag_default = format_discriminant_tokens(0, int_repr, enum_ident.span())?;
    state.extern_fns.push(quote! {
        #[allow(clippy::undocumented_unsafe_blocks)]
        #tag_export_attr
        unsafe extern "C" fn #tag_fn_ident(ptr: *const ::core::ffi::c_void) -> #tag_ty_ident {
            if ptr.is_null() {
                return #tag_default;
            }
            let typed_ptr = ptr.cast::<#enum_ident>();
            if !typed_ptr.is_aligned() {
                return #tag_default;
            }
            match unsafe { &*typed_ptr } {
                #(#tag_arms)*
            }
        }
    });
    let tag_doc = format!("Generated C-ABI `tag` discriminant getter slot for `{enum_ident}`.");
    state.vtable_fields.push(quote! {
        #[doc = #tag_doc]
        pub tag: ::core::option::Option<
            unsafe extern "C" fn(*const ::core::ffi::c_void) -> #tag_ty_ident
        >,
    });
    state.vtable_inits.push(quote! {
        tag: ::core::option::Option::Some(#tag_fn_ident),
    });
    let tag_sym_expr = state.canonical_name.symbol_name_expr("tag");
    state.symbol_entries.push(quote! {
        #sys_crate::SymbolEntry {
            name: #tag_sym_expr,
            ptr: #tag_fn_ident as *const ::core::ffi::c_void,
        }
    });
    Ok(())
}

/// Emits field type assertions and collects decl/schema/witness metadata for one enum variant.
fn collect_variant_field_metadata(
    state: &mut CodegenState,
    fields_info: &[EnumFieldInfo],
) -> syn::Result<(
    Vec<DeclEnumVariantField>,
    Vec<(String, String)>,
    Vec<WitnessEnumVariantField>,
)> {
    let sys_crate = state.sys_crate.clone();
    let mut decl_fields = Vec::new();
    let mut schema_variant_fields = Vec::new();
    let mut witness_fields = Vec::new();

    for info in fields_info {
        let f_span = info.span;
        let f_ty = &info.raw_ty;
        match &info.kind {
            FieldKind::Skip => continue,
            FieldKind::Bool(_) => {
                state.layout_assertions.push(quote_spanned! { f_span =>
                    const _: fn() = || {
                        fn assert_bool_field<T: __BamPrimBool>() {}
                        assert_bool_field::<#f_ty>();
                    };
                });
            }
            FieldKind::Primitive { is_abi_pod, .. } => {
                if *is_abi_pod {
                    state.layout_assertions.push(quote_spanned! { f_span =>
                        const _: fn() = || {
                            #sys_crate::assert_abi_pod::<#f_ty>();
                        };
                    });
                } else {
                    state.layout_assertions.push(quote_spanned! { f_span =>
                        const _: fn() = || {
                            #sys_crate::assert_raw_insertable::<#f_ty>();
                        };
                    });
                }
            }
            FieldKind::HostString(_) => {
                state.layout_assertions.push(quote_spanned! { f_span =>
                    const _: fn() = || {
                        fn assert_str_field<T: __BamStdString>() {}
                        assert_str_field::<#f_ty>();
                    };
                });
            }
            FieldKind::HostVec { elem_ty, .. } => {
                let elem_span = elem_ty.span();
                state.layout_assertions.push(quote_spanned! { f_span =>
                    const _: fn() = || {
                        fn assert_vec_field<Elem, V: __BamStdVec<Elem>>() {}
                        assert_vec_field::<#elem_ty, #f_ty>();
                    };
                });
                state.layout_assertions.push(quote_spanned! { elem_span =>
                    const _: fn() = || {
                        #sys_crate::assert_abi_pod::<#elem_ty>();
                    };
                });
            }
            FieldKind::FfiOption { inner_ty, .. } => {
                let inner_span = inner_ty.span();
                state.layout_assertions.push(quote_spanned! { f_span =>
                    const _: fn() = || {
                        fn assert_opt_field<Inner, Opt: __BamFfiOption<Inner>>() {}
                        assert_opt_field::<#inner_ty, #f_ty>();
                    };
                });
                state.layout_assertions.push(quote_spanned! { inner_span =>
                    const _: fn() = || {
                        #sys_crate::assert_raw_insertable::<#inner_ty>();
                    };
                });
            }
            FieldKind::Pad(_) => {
                return Err(syn::Error::new_spanned(
                    f_ty,
                    "`#[mod_field(pad)]` is only supported on `#[repr(C)]` struct fields",
                ));
            }
            FieldKind::Nested { .. } => {
                state.layout_assertions.push(quote_spanned! { f_span =>
                    const _: fn() = || {
                        fn assert_nested_clone<T: ::core::clone::Clone + 'static>() {}
                        assert_nested_clone::<#f_ty>();
                    };
                });
            }
        }

        schema_variant_fields.push((
            info.field_name.clone(),
            canonical_field_type_tag(&info.raw_ty),
        ));

        let decl_kind = match &info.kind {
            FieldKind::Skip | FieldKind::Pad(_) => {
                return Err(syn::Error::new_spanned(
                    f_ty,
                    "internal error: unexpected skipped or padding field in enum variant decl",
                ));
            }
            FieldKind::Bool(_) => DeclFieldKind::Bool,
            FieldKind::Primitive { .. } => DeclFieldKind::Primitive,
            FieldKind::HostString(_) => DeclFieldKind::HostString,
            FieldKind::HostVec { elem_ty, .. } => DeclFieldKind::HostVec {
                elem_ty: elem_ty.clone(),
            },
            FieldKind::FfiOption { inner_ty, .. } => DeclFieldKind::FfiOption {
                inner_ty: inner_ty.clone(),
            },
            FieldKind::Nested { .. } => DeclFieldKind::Nested,
        };

        decl_fields.push(DeclEnumVariantField {
            field_name: info.field_name.clone(),
            c_field_ident: if info
                .field_name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
            {
                format!("_{}", info.field_name)
            } else {
                info.field_name.clone()
            },
            kind: decl_kind,
            raw_ty: info.raw_ty.clone(),
        });

        let witness_kind = match &info.kind {
            FieldKind::Skip | FieldKind::Pad(_) => {
                return Err(syn::Error::new_spanned(
                    f_ty,
                    "internal error: unexpected skipped or padding field in enum variant witness",
                ));
            }
            FieldKind::Bool(_) => WitnessFieldKind::Bool,
            FieldKind::Primitive { .. } => WitnessFieldKind::Primitive,
            FieldKind::HostString(_) => WitnessFieldKind::HostString,
            FieldKind::HostVec { elem_ty, .. } => WitnessFieldKind::HostVec {
                elem_ty: elem_ty.clone(),
            },
            FieldKind::FfiOption { inner_ty, .. } => WitnessFieldKind::FfiOption {
                inner_ty: inner_ty.clone(),
            },
            FieldKind::Nested { .. } => WitnessFieldKind::Nested,
        };
        witness_fields.push(WitnessEnumVariantField {
            field_name: info.field_name.clone(),
            type_tag: canonical_kind_type_tag(&info.raw_ty, &info.kind),
            rust_ident: info.rust_ident.clone(),
            kind: witness_kind,
            raw_ty: info.raw_ty.clone(),
        });
    }

    Ok((decl_fields, schema_variant_fields, witness_fields))
}

/// Emits `set_<var_snake>` setter slot for an enum variant with no skipped fields.
fn emit_enum_variant_setter_slot(
    state: &mut CodegenState,
    pv: &ParsedEnumVariant<'_>,
    v_snake: &str,
) -> syn::Result<()> {
    let enum_ident = state.struct_ident.clone();
    let enum_snake = crate::attrs::to_snake_case(&enum_ident.to_string());
    let sys_crate = state.sys_crate.clone();
    let v_ident = &pv.variant.ident;
    let v_name = v_ident.to_string();

    let set_slot_str = format!("set_{v_snake}");
    let set_export_str = format!("{}_{set_slot_str}", state.export_fn_prefix);
    if !state.used_vtable_fields.insert(set_slot_str.clone())
        || !state.used_export_names.insert(set_export_str.clone())
    {
        return Err(syn::Error::new_spanned(
            v_ident,
            format!("enum variant `{v_name}` produces colliding VTable slot `{set_slot_str}`"),
        ));
    }

    let mut param_fn_toks = Vec::new();
    let mut param_vt_toks = Vec::new();
    let mut prep_toks = Vec::new();
    let mut ctor_fields = Vec::new();

    for (idx, info) in pv.fields_info.iter().enumerate() {
        let param_ident = format_ident!("arg_{idx}");
        let local_ident = format_ident!("local_{idx}");
        let f_ty = &info.raw_ty;

        match &info.kind {
            FieldKind::Skip => {
                return Err(syn::Error::new_spanned(
                    f_ty,
                    "internal error: skipped field encountered in non-skipped enum variant setter",
                ));
            }
            FieldKind::Bool(_) => {
                param_fn_toks.push(quote! { , #param_ident: #sys_crate::FfiBool });
                param_vt_toks.push(quote! { , #sys_crate::FfiBool });
                prep_toks.push(quote! {
                    let #local_ident: #f_ty = #param_ident.get();
                });
            }
            FieldKind::Primitive { .. } => {
                param_fn_toks.push(quote! { , #param_ident: #f_ty });
                param_vt_toks.push(quote! { , #f_ty });
                prep_toks.push(quote! {
                    let #local_ident: #f_ty = #param_ident;
                });
            }
            FieldKind::FfiOption { inner_ty, .. } => {
                param_fn_toks.push(quote! { , #param_ident: #sys_crate::FfiOption<#inner_ty> });
                param_vt_toks.push(quote! { , #sys_crate::FfiOption<#inner_ty> });
                prep_toks.push(quote! {
                    let #local_ident: #sys_crate::FfiOption<#inner_ty> = if #param_ident.is_some() {
                        #sys_crate::FfiOption::some(#param_ident.value)
                    } else {
                        #sys_crate::FfiOption::none()
                    };
                });
            }
            FieldKind::HostString(_) => {
                param_fn_toks.push(quote! { , #param_ident: *const #sys_crate::StrView });
                param_vt_toks.push(quote! { , *const #sys_crate::StrView });
                prep_toks.push(quote! {
                    let #local_ident: ::std::string::String = {
                        if #param_ident.is_null() {
                            return #sys_crate::FfiResult::NULL_POINTER;
                        }
                        if !#param_ident.is_aligned() {
                            return #sys_crate::FfiResult::INVALID_ARGUMENT;
                        }
                        let view_val = unsafe { ::core::ptr::read(#param_ident) };
                        let status = view_val.validate();
                        if status.is_err() {
                            return status;
                        }
                        if view_val.len == 0 {
                            ::std::string::String::new()
                        } else {
                            let bytes = unsafe { view_val.as_slice() };
                            match ::core::str::from_utf8(bytes) {
                                ::core::result::Result::Ok(s) => ::std::string::String::from(s),
                                ::core::result::Result::Err(_) => return #sys_crate::FfiResult::INVALID_UTF8,
                            }
                        }
                    };
                });
            }
            FieldKind::HostVec { elem_ty, .. } => {
                param_fn_toks
                    .push(quote! { , #param_ident: *const #sys_crate::FfiSlice<#elem_ty> });
                param_vt_toks.push(quote! { , *const #sys_crate::FfiSlice<#elem_ty> });
                prep_toks.push(quote! {
                    let #local_ident: ::std::vec::Vec<#elem_ty> = {
                        if #param_ident.is_null() {
                            return #sys_crate::FfiResult::NULL_POINTER;
                        }
                        if !#param_ident.is_aligned() {
                            return #sys_crate::FfiResult::INVALID_ARGUMENT;
                        }
                        let slice_val = unsafe { ::core::ptr::read(#param_ident) };
                        let status = slice_val.validate();
                        if status.is_err() {
                            return status;
                        }
                        if slice_val.len == 0 {
                            ::std::vec::Vec::new()
                        } else {
                            let elems = unsafe { slice_val.as_slice() };
                            elems.to_vec()
                        }
                    };
                });
            }
            FieldKind::Nested { .. } => {
                param_fn_toks.push(quote! { , #param_ident: *const ::core::ffi::c_void });
                param_vt_toks.push(quote! { , *const ::core::ffi::c_void });
                prep_toks.push(quote! {
                    let #local_ident: #f_ty = {
                        if #param_ident.is_null() {
                            return #sys_crate::FfiResult::NULL_POINTER;
                        }
                        let nested_ptr = #param_ident.cast::<#f_ty>();
                        if !nested_ptr.is_aligned() {
                            return #sys_crate::FfiResult::INVALID_ARGUMENT;
                        }
                        unsafe { ::core::clone::Clone::clone(&*nested_ptr) }
                    };
                });
            }
            FieldKind::Pad(ty) => {
                return Err(syn::Error::new_spanned(
                    ty.as_ref(),
                    "`#[mod_field(pad)]` is not supported on enum variant fields",
                ));
            }
        }

        if let Some(ref rust_ident) = info.rust_ident {
            ctor_fields.push(quote! { #rust_ident: #local_ident });
        } else {
            ctor_fields.push(quote! { #local_ident });
        }
    }

    let ctor_expr = if pv.is_unit {
        quote! { #enum_ident::#v_ident }
    } else if pv.is_named {
        quote! { #enum_ident::#v_ident { #(#ctor_fields),* } }
    } else {
        quote! { #enum_ident::#v_ident(#(#ctor_fields),*) }
    };

    let set_slot_ident = Ident::new(&set_slot_str, Span::call_site());
    let set_fn_ident = format_ident!("__bam_{}_{}", enum_snake, set_slot_str);
    let export_attr = if state.export_symbols {
        quote! {
            #[allow(unreachable_pub)]
            #[unsafe(export_name = #set_export_str)]
            pub
        }
    } else {
        quote! {}
    };

    state.extern_fns.push(quote! {
        #[allow(clippy::undocumented_unsafe_blocks)]
        #export_attr
        unsafe extern "C" fn #set_fn_ident(
            ptr: *mut ::core::ffi::c_void
            #(#param_fn_toks)*
        ) -> #sys_crate::FfiResult {
            if ptr.is_null() {
                return #sys_crate::FfiResult::NULL_POINTER;
            }
            let typed_ptr = ptr.cast::<#enum_ident>();
            if !typed_ptr.is_aligned() {
                return #sys_crate::FfiResult::INVALID_ARGUMENT;
            }
            #(#prep_toks)*
            unsafe {
                *typed_ptr = #ctor_expr;
            }
            #sys_crate::FfiResult::OK
        }
    });

    let doc = format!(
        "Generated C-ABI `{set_slot_str}` checked variant setter slot for `{enum_ident}::{v_name}`."
    );
    state.vtable_fields.push(quote! {
        #[doc = #doc]
        pub #set_slot_ident: ::core::option::Option<
            unsafe extern "C" fn(*mut ::core::ffi::c_void #(#param_vt_toks)*) -> #sys_crate::FfiResult
        >,
    });
    state.vtable_inits.push(quote! {
        #set_slot_ident: ::core::option::Option::Some(#set_fn_ident),
    });
    let sym_expr = state.canonical_name.symbol_name_expr(&set_slot_str);
    state.symbol_entries.push(quote! {
        #sys_crate::SymbolEntry {
            name: #sym_expr,
            ptr: #set_fn_ident as *const ::core::ffi::c_void,
        }
    });
    Ok(())
}

/// Emits `try_get_<var_snake>_<field_name>` payload getter slots for one enum variant.
fn emit_enum_variant_getter_slots(
    state: &mut CodegenState,
    pv: &ParsedEnumVariant<'_>,
    v_snake: &str,
    variant_count: usize,
) -> syn::Result<()> {
    let enum_ident = state.struct_ident.clone();
    let enum_snake = crate::attrs::to_snake_case(&enum_ident.to_string());
    let sys_crate = state.sys_crate.clone();
    let v_ident = &pv.variant.ident;
    let v_name = v_ident.to_string();

    for (idx, info) in pv.fields_info.iter().enumerate() {
        if matches!(info.kind, FieldKind::Skip) {
            continue;
        }
        let try_slot_str = format!("try_get_{v_snake}_{}", info.field_name);
        let try_export_str = format!("{}_{try_slot_str}", state.export_fn_prefix);
        if !state.used_vtable_fields.insert(try_slot_str.clone())
            || !state.used_export_names.insert(try_export_str.clone())
        {
            return Err(syn::Error::new_spanned(
                v_ident,
                format!(
                    "enum variant `{v_name}` field `{}` produces colliding VTable slot `{try_slot_str}`",
                    info.field_name
                ),
            ));
        }

        let f_ty = &info.raw_ty;
        let (out_ty, write_expr) = match &info.kind {
            FieldKind::Skip => {
                return Err(syn::Error::new_spanned(
                    f_ty,
                    "internal error: skipped field in enum getter slot",
                ));
            }
            FieldKind::Bool(_) => (
                quote! { #sys_crate::FfiBool },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, #sys_crate::FfiBool::from_bool(*binding));
                    }
                },
            ),
            FieldKind::Primitive { .. } => (
                quote! { #f_ty },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, *binding);
                    }
                },
            ),
            FieldKind::FfiOption { inner_ty, .. } => (
                quote! { #sys_crate::FfiOption<#inner_ty> },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, *binding);
                    }
                },
            ),
            FieldKind::HostString(_) => (
                quote! { #sys_crate::StrView },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, __bam_read_string::<#f_ty>(binding));
                    }
                },
            ),
            FieldKind::HostVec { elem_ty, .. } => (
                quote! { #sys_crate::FfiSlice<#elem_ty> },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, __bam_read_vec::<#elem_ty, #f_ty>(binding));
                    }
                },
            ),
            FieldKind::Nested { .. } => (
                quote! { *const ::core::ffi::c_void },
                quote! {
                    unsafe {
                        ::core::ptr::write(out_val, (binding as *const #f_ty).cast::<::core::ffi::c_void>());
                    }
                },
            ),
            FieldKind::Pad(ty) => {
                return Err(syn::Error::new_spanned(
                    ty.as_ref(),
                    "`#[mod_field(pad)]` is not supported on enum variant fields",
                ));
            }
        };

        let match_pat = if pv.is_named {
            let f_ident = info.rust_ident.as_ref().ok_or_else(|| {
                syn::Error::new_spanned(
                    v_ident,
                    "internal error: named enum variant field missing ident",
                )
            })?;
            quote! { #enum_ident::#v_ident { #f_ident: binding, .. } }
        } else {
            let mut pats = Vec::with_capacity(pv.fields_info.len());
            for i in 0..pv.fields_info.len() {
                if i == idx {
                    pats.push(quote! { binding });
                } else {
                    pats.push(quote! { _ });
                }
            }
            quote! { #enum_ident::#v_ident(#(#pats),*) }
        };

        let fallback_arm = if variant_count > 1 {
            quote! { _ => #sys_crate::FfiResult::INVALID_STATE, }
        } else {
            quote! {}
        };

        let try_slot_ident = Ident::new(&try_slot_str, Span::call_site());
        let try_fn_ident = format_ident!("__bam_{}_{}", enum_snake, try_slot_str);
        let export_attr = if state.export_symbols {
            quote! {
                #[allow(unreachable_pub)]
                #[unsafe(export_name = #try_export_str)]
                pub
            }
        } else {
            quote! {}
        };

        state.extern_fns.push(quote! {
            #[allow(clippy::undocumented_unsafe_blocks)]
            #export_attr
            unsafe extern "C" fn #try_fn_ident(
                ptr: *const ::core::ffi::c_void,
                out_val: *mut #out_ty,
            ) -> #sys_crate::FfiResult {
                if ptr.is_null() || out_val.is_null() {
                    return #sys_crate::FfiResult::NULL_POINTER;
                }
                let typed_ptr = ptr.cast::<#enum_ident>();
                if !typed_ptr.is_aligned() || !out_val.is_aligned() {
                    return #sys_crate::FfiResult::INVALID_ARGUMENT;
                }
                match unsafe { &*typed_ptr } {
                    #match_pat => {
                        #write_expr
                        #sys_crate::FfiResult::OK
                    }
                    #fallback_arm
                }
            }
        });

        let doc = format!(
            "Generated C-ABI `{try_slot_str}` checked payload getter slot for `{enum_ident}::{v_name}`."
        );
        state.vtable_fields.push(quote! {
            #[doc = #doc]
            pub #try_slot_ident: ::core::option::Option<
                unsafe extern "C" fn(*const ::core::ffi::c_void, *mut #out_ty) -> #sys_crate::FfiResult
            >,
        });
        state.vtable_inits.push(quote! {
            #try_slot_ident: ::core::option::Option::Some(#try_fn_ident),
        });
        let sym_expr = state.canonical_name.symbol_name_expr(&try_slot_str);
        state.symbol_entries.push(quote! {
            #sys_crate::SymbolEntry {
                name: #sym_expr,
                ptr: #try_fn_ident as *const ::core::ffi::c_void,
            }
        });
    }
    Ok(())
}

/// Expands `#[derive(ModComponent)]` for fieldless and tagged data-carrying enums.
///
/// Intended purpose: exposes Rust enums across the C-ABI safely via checked `tag`,
/// `set_<variant>`, and `try_get_<variant>_<field>` VTable slots while forbidding `AbiPod` and
/// `RawInsertable` (enums have invalid discriminant bit patterns).
///
/// What it does: validates `#[repr(C, <int>)]` / `#[repr(<int>)]` and non-empty variants,
/// parses explicit/implicit discriminants, emits tag/setter/getter extern functions and witness
/// symbols, and delegates final VTable and `ModComponent` impl emission to `vtable::emit_vtable_and_impls`.
pub(crate) fn expand_enum_mod_component(
    input: &DeriveInput,
    data_enum: &DataEnum,
    repr_flags: &ReprFlags,
    mut state: CodegenState,
    vtable_ident: &Ident,
) -> syn::Result<TokenStream> {
    if state.guest_crate.is_some() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[mod_component(guest)]` cannot be applied to enums because `GuestComponent` requires `AbiPod`",
        ));
    }
    if state.pod_only {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[mod_component(pod_only)]` cannot be applied to enums",
        ));
    }
    if let Some(ref bad_int) = repr_flags.unsupported_int_repr {
        return Err(syn::Error::new_spanned(
            &input.ident,
            format!(
                "`#[derive(ModComponent)]` does not support `#[repr({bad_int})]` on enums; use `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, or `i64`"
            ),
        ));
    }
    if data_enum.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(ModComponent)]` does not support zero-variant enums",
        ));
    }

    let is_fieldless = data_enum
        .variants
        .iter()
        .all(|v| matches!(v.fields, Fields::Unit));

    // Enumerations always have invalid bit patterns, so `AbiPod` and `RawInsertable` are NEVER emitted.
    state.can_impl_raw_insertable = false;
    state.can_impl_abi_pod = false;

    let int_repr = repr_flags.int_repr;
    let variant_count = data_enum.variants.len();
    let parsed_variants = parse_enum_variants(input, data_enum, repr_flags, is_fieldless)?;

    emit_enum_tag_slot(&mut state, &parsed_variants, is_fieldless, int_repr)?;

    let mut decl_variants = Vec::with_capacity(variant_count);
    let mut witness_variants = Vec::with_capacity(variant_count);

    for pv in &parsed_variants {
        let v_name = pv.variant.ident.to_string();
        let v_snake = to_snake_case(&v_name);
        let (decl_fields, schema_variant_fields, witness_fields) =
            collect_variant_field_metadata(&mut state, &pv.fields_info)?;

        if !pv.has_skipped {
            emit_enum_variant_setter_slot(&mut state, pv, &v_snake)?;
        }
        emit_enum_variant_getter_slots(&mut state, pv, &v_snake, variant_count)?;

        decl_variants.push(DeclEnumVariant {
            name: v_name.clone(),
            snake_name: v_snake,
            discriminant: pv.disc_val,
            has_skipped: pv.has_skipped,
            fields: decl_fields,
            schema_fields: schema_variant_fields,
        });
        witness_variants.push(WitnessEnumVariant {
            name: v_name,
            is_unit: pv.is_unit,
            is_named: pv.is_named,
            has_skipped: pv.has_skipped,
            fields: witness_fields,
        });
    }

    state.decl_enum = Some(DeclEnumInfo {
        is_fieldless,
        int_repr,
        variants: decl_variants,
    });

    emit_enum_witnesses(&mut state, &witness_variants)?;
    vtable::emit_vtable_and_impls(&state, vtable_ident)
}
