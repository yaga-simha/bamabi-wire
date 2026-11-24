//! Single source of truth for `bamabi` canonical wire types, `#[repr(C)]` layout solving,
//! component UUIDs, schema fingerprints, and ABI `0.4.0` struct/slot metadata.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod abi_meta;
pub mod hash;
pub mod layout;
pub mod wire_types;

pub use abi_meta::{
    Align32, FieldLayoutSpec, HOST_API_SIZE_32, HOST_API_SIZE_64, HOST_API_SLOT_COUNT,
    HOST_API_SLOTS, HostApiSlotSpec, StructLayoutSpec, WIRE_STRUCT_SPECS, find_wire_struct_spec,
    host_api_size, host_api_slot_specs, wire_struct_specs,
};
pub use hash::{
    EnumVariantSchemaInput, FNV1A_64_OFFSET_BASIS, FNV1A_64_PRIME, SIPHASH_KEY_0, SIPHASH_KEY_1,
    SchemaFieldInput, compute_component_uuid, compute_component_uuid_bytes,
    compute_component_uuid_bytes_raw, compute_enum_schema_fingerprint, compute_enum_schema_hash,
    compute_schema_fingerprint, fnv1a_64_init, fnv1a_64_str, fnv1a_64_update_bytes,
    fnv1a_64_update_u64, format_uuid_compact, format_uuid_hyphenated, hash_field_schema,
    siphash24_128, try_parse_uuid_hex,
};
pub use layout::{LayoutError, align_up, compute_field_placement, compute_struct_size_and_padding};
#[cfg(feature = "alloc")]
pub use layout::{RawFieldInput, SolvedFieldLayout, SolvedStructLayout, solve_repr_c_layout};
pub use wire_types::{
    BamFieldKindId, TargetLayout, WIRE_TYPE_NAMES, WireTypeSpec, canonical_leaf_ident,
    is_banned_rust_wire_type, lookup_wire_type,
};
#[cfg(feature = "alloc")]
pub use wire_types::{canonical_array_tag, canonical_ffi_option_tag, canonical_vec_tag};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_asset_handle_canonical_wire_spec_matches_frozen_abi() {
        for target in [
            TargetLayout::PTR64,
            TargetLayout::PTR32_U64_ALIGN8,
            TargetLayout::PTR32_U64_ALIGN4,
        ] {
            let spec = lookup_wire_type("FfiAssetHandle", target)
                .expect("FfiAssetHandle must be registered");
            assert_eq!(spec.canonical_tag, "FfiAssetHandle");
            assert_eq!(spec.size, 32);
            assert_eq!(spec.align, 4);
            assert_eq!(spec.kind, BamFieldKindId::NESTED);
            assert_ne!(spec.kind, BamFieldKindId::UNKNOWN);
            assert!(spec.is_abi_pod);
            assert!(spec.is_raw_insertable);
        }
    }

    #[test]
    fn host_api_meta_matches_frozen_0_4_slot_offsets() {
        assert_eq!(HOST_API_SLOT_COUNT, 90);
        assert_eq!(HOST_API_SIZE_64, 728);
        assert_eq!(HOST_API_SIZE_32, 368);
        for (idx, s) in host_api_slot_specs().iter().enumerate() {
            assert_eq!(s.index, idx);
            assert_eq!(s.offset_64, 8 + 8 * idx);
            assert_eq!(s.offset_32, 8 + 4 * idx);
        }
        let host_api_spec = find_wire_struct_spec("BamHostApi").expect("BamHostApi spec");
        assert_eq!(host_api_spec.fields.len(), 92);
        assert_eq!(host_api_spec.size_for(TargetLayout::PTR64), 728);
        assert_eq!(host_api_spec.size_for(TargetLayout::PTR32_U64_ALIGN4), 368);
    }

    #[test]
    fn uuid_hex_roundtrip_and_enum_hashing() {
        let bytes = compute_component_uuid_bytes("test::Health", 4, 4, 0);
        let hex_arr = format_uuid_hyphenated(&bytes);
        let hex_str = core::str::from_utf8(&hex_arr).expect("valid ascii");
        assert_eq!(try_parse_uuid_hex(hex_str), Some(bytes));
        let compact = format_uuid_compact(&bytes);
        let compact_str = core::str::from_utf8(&compact).expect("valid ascii");
        assert_eq!(try_parse_uuid_hex(compact_str), Some(bytes));

        let variants = [
            EnumVariantSchemaInput {
                name: "Idle",
                discriminant: 0,
                fields: &[],
            },
            EnumVariantSchemaInput {
                name: "Chase",
                discriminant: 2,
                fields: &[("target", "u64")],
            },
        ];
        let hash = compute_enum_schema_hash("u8", &variants);
        assert_ne!(hash, 0);
        let fp = compute_enum_schema_fingerprint("test::AiState", 16, 8, 0, "u8", &variants);
        assert_ne!(fp, 0);
    }

    #[test]
    fn wire_type_names_all_resolve_across_target_layouts() {
        for &name in WIRE_TYPE_NAMES {
            for target in [
                TargetLayout::PTR64,
                TargetLayout::PTR32_U64_ALIGN8,
                TargetLayout::PTR32_U64_ALIGN4,
            ] {
                let spec = lookup_wire_type(name, target)
                    .unwrap_or_else(|| panic!("WIRE_TYPE_NAMES entry {name} must resolve"));
                assert_eq!(spec.canonical_tag, name);
                assert!(spec.align >= 1);
            }
        }
        let ticks = lookup_wire_type("BamChangeTicks", TargetLayout::PTR64)
            .expect("BamChangeTicks must resolve");
        assert_eq!(ticks.size, 16);
        assert_eq!(ticks.align, 4);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn layout_solver_synthesizes_explicit_pad_fields_and_matching_identity() {
        extern crate alloc;
        let fields = alloc::vec![
            RawFieldInput {
                name: alloc::string::String::from("flag"),
                type_tag: alloc::string::String::from("u8"),
                kind: BamFieldKindId::U8,
                size: 1,
                align: 1,
                is_abi_pod: true,
                is_raw_insertable: true,
            },
            RawFieldInput {
                name: alloc::string::String::from("tick"),
                type_tag: alloc::string::String::from("u64"),
                kind: BamFieldKindId::U64,
                size: 8,
                align: 8,
                is_abi_pod: true,
                is_raw_insertable: true,
            },
            RawFieldInput {
                name: alloc::string::String::from("tail"),
                type_tag: alloc::string::String::from("u16"),
                kind: BamFieldKindId::U16,
                size: 2,
                align: 2,
                is_abi_pod: true,
                is_raw_insertable: true,
            },
        ];
        let solved = solve_repr_c_layout("test::Padded", &fields, 0, "_pad")
            .expect("layout must solve cleanly");
        assert_eq!(solved.size, 24);
        assert_eq!(solved.align, 8);
        assert_eq!(solved.explicit_pad_bytes, 13);
        assert!(solved.is_abi_pod);
        assert_eq!(solved.fields.len(), 5);
        assert_eq!(solved.fields[1].name, "_pad0");
        assert_eq!(solved.fields[1].type_tag, "[u8;7]");
        assert_eq!(solved.fields[4].name, "_pad1");
        assert_eq!(solved.fields[4].type_tag, "[u8;6]");
    }
}
