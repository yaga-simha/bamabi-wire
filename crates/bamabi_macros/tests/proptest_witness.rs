//! Property-based tests for `#[derive(ModComponent)]` schema-evolution witness
//! (`export_fields` -> `import_field`) round-trips across arbitrary strings, vectors, and nested options.

use bamabi_macros::ModComponent;
use bamabi_sys::{FfiOption, FfiResult, FfiSlice, ModComponent, StrView};
use core::ffi::c_void;
use proptest::prelude::*;

type ExportSinkFn = unsafe extern "C" fn(
    *mut c_void,
    *const StrView,
    *const StrView,
    *const FfiSlice<u8>,
) -> FfiResult;
type ExportFieldsFn =
    unsafe extern "C" fn(*const c_void, Option<ExportSinkFn>, *mut c_void) -> FfiResult;
type ImportFieldFn = unsafe extern "C" fn(
    *mut c_void,
    *const StrView,
    *const StrView,
    *const FfiSlice<u8>,
) -> FfiResult;

/// Collects exported `(field_name, type_tag, payload_bytes)` tuples from `export_fields`.
///
/// # Safety
/// `ctx` must point to a valid, exclusively borrowed `Vec<(String, String, Vec<u8>)>`, and
/// `field_name` / `type_tag` / `payload` must uphold the witness `StrView` / `FfiSlice` contract.
unsafe extern "C" fn collect_sink(
    ctx: *mut c_void,
    field_name: *const StrView,
    type_tag: *const StrView,
    payload: *const FfiSlice<u8>,
) -> FfiResult {
    // SAFETY: Caller passes `&raw mut captured` pointing to `Vec<(String, String, Vec<u8>)>`,
    // and valid non-null aligned witness view pointers.
    let out = unsafe { &mut *ctx.cast::<Vec<(String, String, Vec<u8>)>>() };
    // SAFETY: Witness exporter always passes valid non-null aligned `*const StrView`.
    let field_name = unsafe { *field_name };
    // SAFETY: Witness exporter always passes valid non-null aligned `*const StrView`.
    let type_tag = unsafe { *type_tag };
    // SAFETY: Witness exporter always passes valid non-null aligned `*const FfiSlice<u8>`.
    let payload = unsafe { *payload };
    let bytes = if payload.len == 0 {
        Vec::new()
    } else {
        // SAFETY: `payload` is a valid non-empty slice for the duration of this callback.
        unsafe { payload.as_slice() }.to_vec()
    };
    // SAFETY: Witness macro emits valid UTF-8 `StrView` literals for `field_name` and `type_tag`.
    let name = unsafe { field_name.try_as_str() }
        .expect("valid UTF-8 field_name")
        .to_owned();
    // SAFETY: Witness macro emits valid UTF-8 `StrView` literals for `type_tag`.
    let tag = unsafe { type_tag.try_as_str() }
        .expect("valid UTF-8 type_tag")
        .to_owned();
    out.push((name, tag, bytes));
    FfiResult::OK
}

/// Resolves `"bam.v0.<Name>.export_fields"` from `T::symbols()` and casts it to `ExportFieldsFn`.
fn resolve_export_fn<T: ModComponent>() -> ExportFieldsFn {
    let sym = format!("bam.v0.{}.export_fields", T::CANONICAL_NAME);
    let ptr = T::resolve_symbol(&sym).expect("export_fields witness symbol");
    // SAFETY: `#[derive(ModComponent)]` registers `bam.v0.<Name>.export_fields` with `ExportFieldsFn` ABI.
    unsafe { core::mem::transmute::<*const c_void, ExportFieldsFn>(ptr) }
}

/// Resolves `"bam.v0.<Name>.import_field"` from `T::symbols()` and casts it to `ImportFieldFn`.
fn resolve_import_fn<T: ModComponent>() -> ImportFieldFn {
    let sym = format!("bam.v0.{}.import_field", T::CANONICAL_NAME);
    let ptr = T::resolve_symbol(&sym).expect("import_field witness symbol");
    // SAFETY: `#[derive(ModComponent)]` registers `bam.v0.<Name>.import_field` with `ImportFieldFn` ABI.
    unsafe { core::mem::transmute::<*const c_void, ImportFieldFn>(ptr) }
}

/// Exports all fields of `val` into an owned `(field_name, type_tag, payload_bytes)` vector.
fn export_all<T: ModComponent>(val: &T) -> Vec<(String, String, Vec<u8>)> {
    let export_fn = resolve_export_fn::<T>();
    let mut captured: Vec<(String, String, Vec<u8>)> = Vec::new();
    // SAFETY: `val` is a valid `T` reference and `captured` lives for the entire `export_fn` call.
    let rc = unsafe {
        export_fn(
            (&raw const *val).cast::<c_void>(),
            Some(collect_sink),
            (&raw mut captured).cast::<c_void>(),
        )
    };
    assert_eq!(rc, FfiResult::OK);
    captured
}

/// Imports every `(field_name, type_tag, payload_bytes)` entry into `dst`, asserting `FfiResult::OK`.
fn import_all<T: ModComponent>(dst: &mut T, fields: &[(String, String, Vec<u8>)]) {
    let import_fn = resolve_import_fn::<T>();
    for (name, tag, bytes) in fields {
        let name_view = StrView::from_str(name);
        let tag_view = StrView::from_str(tag);
        let pay_slice = FfiSlice::from_slice(bytes);
        // SAFETY: `dst` is a valid `T` mutable reference and view pointers point to valid locals.
        let rc = unsafe {
            import_fn(
                (&raw mut *dst).cast::<c_void>(),
                &raw const name_view,
                &raw const tag_view,
                &raw const pay_slice,
            )
        };
        assert_eq!(rc, FfiResult::OK, "import_field failed for {name} ({tag})");
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, ModComponent)]
#[mod_component(name = "prop::NestedLeaf")]
struct NestedLeaf {
    code: u8,
    nested_opt: FfiOption<FfiOption<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, ModComponent)]
#[mod_component(name = "prop::WitnessRecord")]
struct WitnessRecord {
    flag: bool,
    title: String,
    bytes: Vec<u8>,
    scores: Vec<u32>,
    opt_u32: FfiOption<u32>,
    nested_opt: FfiOption<FfiOption<u8>>,
    leaf: NestedLeaf,
}

#[derive(Debug, Clone, Default, PartialEq, ModComponent)]
#[mod_component(name = "prop::SchemaV1")]
struct SchemaV1 {
    small_signed: i16,
    ratio: f32,
    text: String,
    tags: Vec<u16>,
    nested_opt: FfiOption<FfiOption<u8>>,
    dropped_field: u64,
}

#[derive(Debug, Clone, Default, PartialEq, ModComponent)]
#[mod_component(name = "prop::SchemaV2")]
struct SchemaV2 {
    added_default: i32,
    nested_opt: FfiOption<FfiOption<u8>>,
    tags: Vec<u16>,
    text: String,
    ratio: f64,
    small_signed: i64,
}

#[repr(C, u8)]
#[derive(Debug, Clone, Default, PartialEq, ModComponent)]
#[mod_component(name = "prop::WitnessEnum")]
enum WitnessEnum {
    #[default]
    Empty = 0,
    Payload {
        name: String,
        values: Vec<u64>,
        nested_opt: FfiOption<FfiOption<u8>>,
    } = 1,
    Tuple(String, Vec<u8>, FfiOption<FfiOption<u8>>, NestedLeaf) = 2,
}

fn ffi_nested_opt_strategy() -> impl Strategy<Value = FfiOption<FfiOption<u8>>> {
    prop_oneof![
        Just(FfiOption::none()),
        Just(FfiOption::some(FfiOption::none())),
        any::<u8>().prop_map(|v| FfiOption::some(FfiOption::some(v))),
    ]
}

fn nested_leaf_strategy() -> impl Strategy<Value = NestedLeaf> {
    (any::<u8>(), ffi_nested_opt_strategy())
        .prop_map(|(code, nested_opt)| NestedLeaf { code, nested_opt })
}

fn witness_record_strategy() -> impl Strategy<Value = WitnessRecord> {
    (
        any::<bool>(),
        any::<String>(),
        prop::collection::vec(any::<u8>(), 0..=64),
        prop::collection::vec(any::<u32>(), 0..=24),
        prop::option::of(any::<u32>()).prop_map(FfiOption::from_option),
        ffi_nested_opt_strategy(),
        nested_leaf_strategy(),
    )
        .prop_map(
            |(flag, title, bytes, scores, opt_u32, nested_opt, leaf)| WitnessRecord {
                flag,
                title,
                bytes,
                scores,
                opt_u32,
                nested_opt,
                leaf,
            },
        )
}

fn witness_enum_strategy() -> impl Strategy<Value = WitnessEnum> {
    prop_oneof![
        Just(WitnessEnum::Empty),
        (
            any::<String>(),
            prop::collection::vec(any::<u64>(), 0..=16),
            ffi_nested_opt_strategy(),
        )
            .prop_map(|(name, values, nested_opt)| WitnessEnum::Payload {
                name,
                values,
                nested_opt,
            }),
        (
            any::<String>(),
            prop::collection::vec(any::<u8>(), 0..=32),
            ffi_nested_opt_strategy(),
            nested_leaf_strategy(),
        )
            .prop_map(|(s, b, opt, leaf)| WitnessEnum::Tuple(s, b, opt, leaf)),
    ]
}

fn proptest_config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig {
            cases: 4,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    } else {
        ProptestConfig::with_cases(512)
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    /// Test scenario: verifies `export_fields` -> `import_field` preserves arbitrary `bool`,
    /// `String`, `Vec<u8>`, `Vec<u32>`, `FfiOption<u32>`, nested `FfiOption<FfiOption<u8>>`, and
    /// nested `ModComponent` (`leaf.code`, `leaf.nested_opt`) values over dirty destinations.
    #[test]
    fn mod_component_struct_witness_roundtrip(
        src in witness_record_strategy(),
        dirty_dst in witness_record_strategy(),
    ) {
        let exported = export_all(&src);
        prop_assert!(exported.iter().any(|(n, _, _)| n == "leaf.code"));
        prop_assert!(exported.iter().any(|(n, _, _)| n == "leaf.nested_opt"));

        let mut dst = dirty_dst;
        import_all(&mut dst, &exported);
        prop_assert_eq!(dst, src);
    }

    /// Test scenario: verifies tagged enum witness round-trips (`__variant` switching plus
    /// named-variant, tuple-variant, and nested `ModComponent` payload restoration).
    #[test]
    fn mod_component_enum_witness_roundtrip(
        src in witness_enum_strategy(),
        dirty_dst in witness_enum_strategy(),
    ) {
        let exported = export_all(&src);
        prop_assert_eq!(&exported[0].0, "__variant");

        let mut dst = dirty_dst;
        import_all(&mut dst, &exported);
        prop_assert_eq!(dst, src);
    }

    /// Test scenario: verifies cross-schema migration (`SchemaV1` -> `SchemaV2`) handles
    /// reordered fields, dropped fields (`FfiResult::NOT_FOUND`), added fields left intact, and
    /// lossless numeric widening (`i16 -> i64`, `f32 -> f64`).
    #[test]
    fn mod_component_schema_migration_roundtrip(
        small_signed in any::<i16>(),
        ratio in any::<f32>().prop_filter("finite or inf for exact f64 equality", |f| !f.is_nan()),
        text in any::<String>(),
        tags in prop::collection::vec(any::<u16>(), 0..=24),
        nested_opt in ffi_nested_opt_strategy(),
        dropped_field in any::<u64>(),
        added_default in any::<i32>(),
    ) {
        let src = SchemaV1 {
            small_signed,
            ratio,
            text: text.clone(),
            tags: tags.clone(),
            nested_opt,
            dropped_field,
        };
        let exported = export_all(&src);

        let import_v2 = resolve_import_fn::<SchemaV2>();
        let mut dst = SchemaV2 {
            added_default,
            nested_opt: FfiOption::some(FfiOption::some(0xAA)),
            tags: vec![9, 8, 7],
            text: String::from("stale_value"),
            ratio: -1.0,
            small_signed: 123_456,
        };

        for (name, tag, bytes) in &exported {
            let name_view = StrView::from_str(name);
            let tag_view = StrView::from_str(tag);
            let pay_slice = FfiSlice::from_slice(bytes);
            // SAFETY: `dst` is valid and view pointers point to valid locals.
            let rc = unsafe {
                import_v2(
                    (&raw mut dst).cast::<c_void>(),
                    &raw const name_view,
                    &raw const tag_view,
                    &raw const pay_slice,
                )
            };
            if name == "dropped_field" {
                prop_assert_eq!(rc, FfiResult::NOT_FOUND);
            } else {
                prop_assert_eq!(rc, FfiResult::OK);
            }
        }

        prop_assert_eq!(dst.added_default, added_default);
        prop_assert_eq!(dst.small_signed, i64::from(small_signed));
        prop_assert_eq!(dst.ratio.to_bits(), f64::from(ratio).to_bits());
        prop_assert_eq!(dst.text, text);
        prop_assert_eq!(dst.tags, tags);
        prop_assert_eq!(dst.nested_opt, nested_opt);
    }
}
