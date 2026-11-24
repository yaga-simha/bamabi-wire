//! ABI `0.4.0` wire struct layout and `BamHostApi` slot metadata re-exported from
//! [`bamabi_schema::abi_meta`] alongside host-verified layout helpers.

pub use crate::host_api::offsets::ALL_SLOT_OFFSETS;
pub use bamabi_schema::TargetLayout;
pub use bamabi_schema::abi_meta::{
    Align32, FieldLayoutSpec, HOST_API_SIZE_32, HOST_API_SIZE_64, HOST_API_SLOT_COUNT,
    HOST_API_SLOTS, HostApiSlotSpec, StructLayoutSpec, WIRE_STRUCT_SPECS, find_wire_struct_spec,
    host_api_size, host_api_slot_specs, wire_struct_specs,
};

#[cfg(test)]
mod tests {
    use core::mem::{align_of, offset_of, size_of};

    use super::*;
    use crate::*;

    fn assert_spec(
        c_name: &str,
        rust_size: usize,
        rust_align: usize,
        field_offsets: &[(&str, usize)],
    ) {
        let spec = find_wire_struct_spec(c_name).expect("struct spec must exist");
        assert_eq!(
            spec.size_for(TargetLayout::HOST),
            rust_size,
            "{c_name} size"
        );
        assert_eq!(
            spec.align_for(TargetLayout::HOST),
            rust_align,
            "{c_name} align"
        );
        assert_eq!(
            spec.fields.len(),
            field_offsets.len(),
            "{c_name} field count"
        );
        for (idx, ((expected_name, rust_off), meta)) in
            field_offsets.iter().zip(spec.fields.iter()).enumerate()
        {
            assert_eq!(meta.c_name, *expected_name, "{c_name} field[{idx}] name");
            assert_eq!(
                meta.offset_for(TargetLayout::HOST),
                *rust_off,
                "{c_name}.{} offset",
                meta.c_name
            );
        }
    }

    #[test]
    fn abi_meta_matches_every_bamabi_sys_wire_struct_layout() {
        assert_spec(
            "BamComponentUuid",
            size_of::<ComponentUuid>(),
            align_of::<ComponentUuid>(),
            &[("bytes", offset_of!(ComponentUuid, bytes))],
        );
        assert_spec(
            "BamStrView",
            size_of::<StrView>(),
            align_of::<StrView>(),
            &[
                ("ptr", offset_of!(StrView, ptr)),
                ("len", offset_of!(StrView, len)),
            ],
        );
        assert_spec(
            "BamManifest",
            size_of::<BamManifest>(),
            align_of::<BamManifest>(),
            &[
                ("struct_size", offset_of!(BamManifest, struct_size)),
                ("abi_version", offset_of!(BamManifest, abi_version)),
                ("name", offset_of!(BamManifest, name)),
                ("version", offset_of!(BamManifest, version)),
                ("flags", offset_of!(BamManifest, flags)),
                ("reserved0", offset_of!(BamManifest, reserved0)),
                ("save_state", offset_of!(BamManifest, save_state)),
                ("restore_state", offset_of!(BamManifest, restore_state)),
            ],
        );
        assert_spec(
            "BamAssetHandle",
            size_of::<FfiAssetHandle>(),
            align_of::<FfiAssetHandle>(),
            &[
                ("kind", offset_of!(FfiAssetHandle, kind)),
                ("reserved0", offset_of!(FfiAssetHandle, reserved0)),
                ("asset_type", offset_of!(FfiAssetHandle, asset_type)),
                ("index", offset_of!(FfiAssetHandle, index)),
                ("generation", offset_of!(FfiAssetHandle, generation)),
                ("uuid", offset_of!(FfiAssetHandle, uuid)),
            ],
        );
        assert_spec(
            "BamTermDesc",
            size_of::<BamTermDesc>(),
            align_of::<BamTermDesc>(),
            &[
                ("component", offset_of!(BamTermDesc, component)),
                ("mode", offset_of!(BamTermDesc, mode)),
                ("flags", offset_of!(BamTermDesc, flags)),
                ("or_group", offset_of!(BamTermDesc, or_group)),
                ("reserved0", offset_of!(BamTermDesc, reserved0)),
            ],
        );
        assert_spec(
            "BamQueryDesc",
            size_of::<BamQueryDesc>(),
            align_of::<BamQueryDesc>(),
            &[
                ("struct_size", offset_of!(BamQueryDesc, struct_size)),
                ("flags", offset_of!(BamQueryDesc, flags)),
                ("terms", offset_of!(BamQueryDesc, terms)),
                ("term_count", offset_of!(BamQueryDesc, term_count)),
                ("param_set_group", offset_of!(BamQueryDesc, param_set_group)),
                ("reserved0", offset_of!(BamQueryDesc, reserved0)),
            ],
        );
        assert_spec(
            "BamObserverDesc",
            size_of::<BamObserverDesc>(),
            align_of::<BamObserverDesc>(),
            &[
                ("struct_size", offset_of!(BamObserverDesc, struct_size)),
                ("flags", offset_of!(BamObserverDesc, flags)),
                ("name", offset_of!(BamObserverDesc, name)),
                ("event", offset_of!(BamObserverDesc, event)),
                ("entity_lo", offset_of!(BamObserverDesc, entity_lo)),
                ("entity_hi", offset_of!(BamObserverDesc, entity_hi)),
                ("reserved0", offset_of!(BamObserverDesc, reserved0)),
                ("callback", offset_of!(BamObserverDesc, callback)),
                ("user_data", offset_of!(BamObserverDesc, user_data)),
                ("event_kind", offset_of!(BamObserverDesc, event_kind)),
                ("reserved1", offset_of!(BamObserverDesc, reserved1)),
                (
                    "watched_components",
                    offset_of!(BamObserverDesc, watched_components),
                ),
                (
                    "watched_component_count",
                    offset_of!(BamObserverDesc, watched_component_count),
                ),
                ("mut_callback", offset_of!(BamObserverDesc, mut_callback)),
                (
                    "propagate_relationship",
                    offset_of!(BamObserverDesc, propagate_relationship),
                ),
                ("reserved2", offset_of!(BamObserverDesc, reserved2)),
                ("ctx_callback", offset_of!(BamObserverDesc, ctx_callback)),
                ("queries", offset_of!(BamObserverDesc, queries)),
                ("query_count", offset_of!(BamObserverDesc, query_count)),
                (
                    "ctx_mut_callback",
                    offset_of!(BamObserverDesc, ctx_mut_callback),
                ),
            ],
        );
        assert_spec(
            "BamScheduleSetDesc",
            size_of::<BamScheduleSetDesc>(),
            align_of::<BamScheduleSetDesc>(),
            &[
                ("struct_size", offset_of!(BamScheduleSetDesc, struct_size)),
                ("flags", offset_of!(BamScheduleSetDesc, flags)),
                ("schedule", offset_of!(BamScheduleSetDesc, schedule)),
                ("before_count", offset_of!(BamScheduleSetDesc, before_count)),
                ("after_count", offset_of!(BamScheduleSetDesc, after_count)),
                ("reserved0", offset_of!(BamScheduleSetDesc, reserved0)),
                ("name", offset_of!(BamScheduleSetDesc, name)),
                ("before_sets", offset_of!(BamScheduleSetDesc, before_sets)),
                ("after_sets", offset_of!(BamScheduleSetDesc, after_sets)),
                ("run_if_ctx", offset_of!(BamScheduleSetDesc, run_if_ctx)),
                ("run_if_query", offset_of!(BamScheduleSetDesc, run_if_query)),
                (
                    "run_if_ctx_user_data",
                    offset_of!(BamScheduleSetDesc, run_if_ctx_user_data),
                ),
                ("parent_sets", offset_of!(BamScheduleSetDesc, parent_sets)),
                (
                    "parent_set_count",
                    offset_of!(BamScheduleSetDesc, parent_set_count),
                ),
                ("reserved1", offset_of!(BamScheduleSetDesc, reserved1)),
            ],
        );
        assert_spec(
            "BamChangeTicks",
            size_of::<BamChangeTicks>(),
            align_of::<BamChangeTicks>(),
            &[
                ("added", offset_of!(BamChangeTicks, added)),
                ("changed", offset_of!(BamChangeTicks, changed)),
                ("last_run", offset_of!(BamChangeTicks, last_run)),
                ("this_run", offset_of!(BamChangeTicks, this_run)),
            ],
        );
        assert_spec(
            "BamFieldDesc",
            size_of::<BamFieldDesc>(),
            align_of::<BamFieldDesc>(),
            &[
                ("struct_size", offset_of!(BamFieldDesc, struct_size)),
                ("kind", offset_of!(BamFieldDesc, kind)),
                ("name", offset_of!(BamFieldDesc, name)),
                ("type_tag", offset_of!(BamFieldDesc, type_tag)),
                ("offset", offset_of!(BamFieldDesc, offset)),
                ("size", offset_of!(BamFieldDesc, size)),
                ("align", offset_of!(BamFieldDesc, align)),
            ],
        );
        assert_spec(
            "BamRequiredComponent",
            size_of::<BamRequiredComponent>(),
            align_of::<BamRequiredComponent>(),
            &[
                ("struct_size", offset_of!(BamRequiredComponent, struct_size)),
                ("flags", offset_of!(BamRequiredComponent, flags)),
                ("component", offset_of!(BamRequiredComponent, component)),
                ("reserved0", offset_of!(BamRequiredComponent, reserved0)),
                ("uuid", offset_of!(BamRequiredComponent, uuid)),
                ("init_bytes", offset_of!(BamRequiredComponent, init_bytes)),
                ("init_size", offset_of!(BamRequiredComponent, init_size)),
                ("init_fn", offset_of!(BamRequiredComponent, init_fn)),
                ("user_data", offset_of!(BamRequiredComponent, user_data)),
            ],
        );
        assert_spec(
            "BamRelationDesc",
            size_of::<BamRelationDesc>(),
            align_of::<BamRelationDesc>(),
            &[
                ("struct_size", offset_of!(BamRelationDesc, struct_size)),
                ("flags", offset_of!(BamRelationDesc, flags)),
                ("name", offset_of!(BamRelationDesc, name)),
                ("uuid", offset_of!(BamRelationDesc, uuid)),
                ("data_size", offset_of!(BamRelationDesc, data_size)),
                ("data_align", offset_of!(BamRelationDesc, data_align)),
                ("max_degree", offset_of!(BamRelationDesc, max_degree)),
                ("reserved0", offset_of!(BamRelationDesc, reserved0)),
            ],
        );
        assert_spec(
            "BamLayoutInfo",
            size_of::<BamLayoutInfo>(),
            align_of::<BamLayoutInfo>(),
            &[
                ("struct_size", offset_of!(BamLayoutInfo, struct_size)),
                ("flags", offset_of!(BamLayoutInfo, flags)),
                ("uuid", offset_of!(BamLayoutInfo, uuid)),
                (
                    "schema_fingerprint",
                    offset_of!(BamLayoutInfo, schema_fingerprint),
                ),
                ("size", offset_of!(BamLayoutInfo, size)),
                ("align", offset_of!(BamLayoutInfo, align)),
                ("fields", offset_of!(BamLayoutInfo, fields)),
                ("field_count", offset_of!(BamLayoutInfo, field_count)),
            ],
        );
        assert_spec(
            "BamComponentDesc",
            size_of::<ComponentDesc>(),
            align_of::<ComponentDesc>(),
            &[
                ("struct_size", offset_of!(ComponentDesc, struct_size)),
                ("flags", offset_of!(ComponentDesc, flags)),
                ("name", offset_of!(ComponentDesc, name)),
                ("uuid", offset_of!(ComponentDesc, uuid)),
                ("size", offset_of!(ComponentDesc, size)),
                ("align", offset_of!(ComponentDesc, align)),
                ("drop_fn", offset_of!(ComponentDesc, drop_fn)),
                ("vtable_ptr", offset_of!(ComponentDesc, vtable_ptr)),
                ("on_add", offset_of!(ComponentDesc, on_add)),
                ("on_remove", offset_of!(ComponentDesc, on_remove)),
                ("hook_user_data", offset_of!(ComponentDesc, hook_user_data)),
                ("on_insert", offset_of!(ComponentDesc, on_insert)),
                ("on_discard", offset_of!(ComponentDesc, on_discard)),
                ("on_despawn", offset_of!(ComponentDesc, on_despawn)),
                ("clone_fn", offset_of!(ComponentDesc, clone_fn)),
                (
                    "required_components",
                    offset_of!(ComponentDesc, required_components),
                ),
                (
                    "required_component_count",
                    offset_of!(ComponentDesc, required_component_count),
                ),
                (
                    "schema_fingerprint",
                    offset_of!(ComponentDesc, schema_fingerprint),
                ),
                ("fields", offset_of!(ComponentDesc, fields)),
                ("field_count", offset_of!(ComponentDesc, field_count)),
                (
                    "required_init_descs",
                    offset_of!(ComponentDesc, required_init_descs),
                ),
                (
                    "required_init_desc_count",
                    offset_of!(ComponentDesc, required_init_desc_count),
                ),
            ],
        );
        assert_spec(
            "BamColumnInfo",
            size_of::<BamColumnInfo>(),
            align_of::<BamColumnInfo>(),
            &[
                ("component", offset_of!(BamColumnInfo, component)),
                ("mode", offset_of!(BamColumnInfo, mode)),
                ("uuid", offset_of!(BamColumnInfo, uuid)),
                ("size", offset_of!(BamColumnInfo, size)),
                ("align", offset_of!(BamColumnInfo, align)),
                ("storage_flags", offset_of!(BamColumnInfo, storage_flags)),
                ("reserved0", offset_of!(BamColumnInfo, reserved0)),
            ],
        );
        assert_spec(
            "BamVTableHeader",
            size_of::<VTableHeader>(),
            align_of::<VTableHeader>(),
            &[
                ("struct_size", offset_of!(VTableHeader, struct_size)),
                ("abi_version", offset_of!(VTableHeader, abi_version)),
                ("uuid", offset_of!(VTableHeader, uuid)),
                ("size", offset_of!(VTableHeader, size)),
                ("align", offset_of!(VTableHeader, align)),
            ],
        );
        assert_spec(
            "BamMappedStringVTable",
            size_of::<MappedStringVTable>(),
            align_of::<MappedStringVTable>(),
            &[
                ("header", offset_of!(MappedStringVTable, header)),
                ("get", offset_of!(MappedStringVTable, get)),
                ("set", offset_of!(MappedStringVTable, set)),
            ],
        );
        assert_spec(
            "BamSystemDesc",
            size_of::<BamSystemDesc>(),
            align_of::<BamSystemDesc>(),
            &[
                ("struct_size", offset_of!(BamSystemDesc, struct_size)),
                ("flags", offset_of!(BamSystemDesc, flags)),
                ("name", offset_of!(BamSystemDesc, name)),
                ("queries", offset_of!(BamSystemDesc, queries)),
                ("query_count", offset_of!(BamSystemDesc, query_count)),
                ("callback", offset_of!(BamSystemDesc, callback)),
                ("user_data", offset_of!(BamSystemDesc, user_data)),
                ("schedule", offset_of!(BamSystemDesc, schedule)),
                ("reserved0", offset_of!(BamSystemDesc, reserved0)),
                ("after", offset_of!(BamSystemDesc, after)),
                ("after_count", offset_of!(BamSystemDesc, after_count)),
                ("before", offset_of!(BamSystemDesc, before)),
                ("before_count", offset_of!(BamSystemDesc, before_count)),
                ("run_if", offset_of!(BamSystemDesc, run_if)),
                (
                    "run_if_user_data",
                    offset_of!(BamSystemDesc, run_if_user_data),
                ),
                ("in_sets", offset_of!(BamSystemDesc, in_sets)),
                ("in_set_count", offset_of!(BamSystemDesc, in_set_count)),
                ("after_weak", offset_of!(BamSystemDesc, after_weak)),
                (
                    "after_weak_count",
                    offset_of!(BamSystemDesc, after_weak_count),
                ),
                ("before_weak", offset_of!(BamSystemDesc, before_weak)),
                (
                    "before_weak_count",
                    offset_of!(BamSystemDesc, before_weak_count),
                ),
                ("ambiguous_with", offset_of!(BamSystemDesc, ambiguous_with)),
                (
                    "ambiguous_with_count",
                    offset_of!(BamSystemDesc, ambiguous_with_count),
                ),
                ("run_if_ctx", offset_of!(BamSystemDesc, run_if_ctx)),
                ("run_if_query", offset_of!(BamSystemDesc, run_if_query)),
                (
                    "run_if_ctx_user_data",
                    offset_of!(BamSystemDesc, run_if_ctx_user_data),
                ),
            ],
        );
        assert_spec(
            "BamTriggerDesc",
            size_of::<BamTriggerDesc>(),
            align_of::<BamTriggerDesc>(),
            &[
                ("struct_size", offset_of!(BamTriggerDesc, struct_size)),
                ("flags", offset_of!(BamTriggerDesc, flags)),
                ("event", offset_of!(BamTriggerDesc, event)),
                ("event_kind", offset_of!(BamTriggerDesc, event_kind)),
                ("entity_lo", offset_of!(BamTriggerDesc, entity_lo)),
                ("entity_hi", offset_of!(BamTriggerDesc, entity_hi)),
                ("data_ptr", offset_of!(BamTriggerDesc, data_ptr)),
                ("data_len", offset_of!(BamTriggerDesc, data_len)),
                (
                    "watched_components",
                    offset_of!(BamTriggerDesc, watched_components),
                ),
                (
                    "watched_component_count",
                    offset_of!(BamTriggerDesc, watched_component_count),
                ),
                (
                    "propagate_relationship",
                    offset_of!(BamTriggerDesc, propagate_relationship),
                ),
                ("reserved0", offset_of!(BamTriggerDesc, reserved0)),
            ],
        );
        assert_eq!(WIRE_STRUCT_SPECS.len(), 24);
        assert_spec(
            "BamRowRange",
            size_of::<BamRowRange>(),
            align_of::<BamRowRange>(),
            &[
                ("start_row", offset_of!(BamRowRange, start_row)),
                ("row_count", offset_of!(BamRowRange, row_count)),
            ],
        );
        assert_spec(
            "BamTimeInfo",
            size_of::<BamTimeInfo>(),
            align_of::<BamTimeInfo>(),
            &[
                ("struct_size", offset_of!(BamTimeInfo, struct_size)),
                ("flags", offset_of!(BamTimeInfo, flags)),
                ("delta_secs", offset_of!(BamTimeInfo, delta_secs)),
                (
                    "fixed_delta_secs",
                    offset_of!(BamTimeInfo, fixed_delta_secs),
                ),
                ("elapsed_secs", offset_of!(BamTimeInfo, elapsed_secs)),
                (
                    "elapsed_secs_wrapped",
                    offset_of!(BamTimeInfo, elapsed_secs_wrapped),
                ),
                (
                    "overstep_fraction",
                    offset_of!(BamTimeInfo, overstep_fraction),
                ),
            ],
        );
        assert_spec(
            "BamParOptions",
            size_of::<BamParOptions>(),
            align_of::<BamParOptions>(),
            &[
                ("struct_size", offset_of!(BamParOptions, struct_size)),
                ("flags", offset_of!(BamParOptions, flags)),
                ("chunk_size", offset_of!(BamParOptions, chunk_size)),
                ("max_partitions", offset_of!(BamParOptions, max_partitions)),
            ],
        );
        assert_spec(
            "BamSpawnColumnDesc",
            size_of::<BamSpawnColumnDesc>(),
            align_of::<BamSpawnColumnDesc>(),
            &[
                ("component", offset_of!(BamSpawnColumnDesc, component)),
                ("flags", offset_of!(BamSpawnColumnDesc, flags)),
                ("column_ptr", offset_of!(BamSpawnColumnDesc, column_ptr)),
                ("elem_size", offset_of!(BamSpawnColumnDesc, elem_size)),
            ],
        );
        let host_api_spec = find_wire_struct_spec("BamHostApi").expect("BamHostApi spec");
        assert_eq!(
            host_api_spec.size_for(TargetLayout::HOST),
            size_of::<BamHostApi>()
        );
        assert_eq!(
            host_api_spec.align_for(TargetLayout::HOST),
            align_of::<BamHostApi>()
        );
        assert_eq!(
            host_api_spec.fields[0].offset_for(TargetLayout::HOST),
            offset_of!(BamHostApi, struct_size)
        );
        assert_eq!(
            host_api_spec.fields[1].offset_for(TargetLayout::HOST),
            offset_of!(BamHostApi, abi_version)
        );
        for (slot_idx, slot_off) in ALL_SLOT_OFFSETS.iter().enumerate() {
            assert_eq!(
                host_api_spec.fields[2 + slot_idx].offset_for(TargetLayout::HOST),
                *slot_off
            );
        }
    }

    #[test]
    fn lookup_wire_type_matches_every_bamabi_sys_type() {
        for &name in bamabi_schema::WIRE_TYPE_NAMES {
            let spec = bamabi_schema::lookup_wire_type(name, TargetLayout::HOST)
                .unwrap_or_else(|| panic!("missing wire type spec for {name}"));
            let (expected_size, expected_align, expected_kind, expected_pod, expected_raw) =
                match name {
                    "bool" => (
                        size_of::<bool>(),
                        align_of::<bool>(),
                        BamFieldKind::BOOL,
                        false,
                        false,
                    ),
                    "u8" => (
                        size_of::<u8>(),
                        align_of::<u8>(),
                        BamFieldKind::U8,
                        true,
                        true,
                    ),
                    "i8" => (
                        size_of::<i8>(),
                        align_of::<i8>(),
                        BamFieldKind::I8,
                        true,
                        true,
                    ),
                    "u16" => (
                        size_of::<u16>(),
                        align_of::<u16>(),
                        BamFieldKind::U16,
                        true,
                        true,
                    ),
                    "i16" => (
                        size_of::<i16>(),
                        align_of::<i16>(),
                        BamFieldKind::I16,
                        true,
                        true,
                    ),
                    "u32" => (
                        size_of::<u32>(),
                        align_of::<u32>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "i32" => (
                        size_of::<i32>(),
                        align_of::<i32>(),
                        BamFieldKind::I32,
                        true,
                        true,
                    ),
                    "f32" => (
                        size_of::<f32>(),
                        align_of::<f32>(),
                        BamFieldKind::F32,
                        true,
                        true,
                    ),
                    "u64" => (
                        size_of::<u64>(),
                        align_of::<u64>(),
                        BamFieldKind::U64,
                        true,
                        true,
                    ),
                    "i64" => (
                        size_of::<i64>(),
                        align_of::<i64>(),
                        BamFieldKind::I64,
                        true,
                        true,
                    ),
                    "f64" => (
                        size_of::<f64>(),
                        align_of::<f64>(),
                        BamFieldKind::F64,
                        true,
                        true,
                    ),
                    "usize" => (
                        size_of::<usize>(),
                        align_of::<usize>(),
                        BamFieldKind::USIZE,
                        false,
                        true,
                    ),
                    "isize" => (
                        size_of::<isize>(),
                        align_of::<isize>(),
                        BamFieldKind::ISIZE,
                        false,
                        true,
                    ),
                    "u128" => (
                        size_of::<u128>(),
                        align_of::<u128>(),
                        BamFieldKind::NESTED,
                        false,
                        true,
                    ),
                    "i128" => (
                        size_of::<i128>(),
                        align_of::<i128>(),
                        BamFieldKind::NESTED,
                        false,
                        true,
                    ),
                    "FfiBool" => (
                        size_of::<FfiBool>(),
                        align_of::<FfiBool>(),
                        BamFieldKind::BOOL,
                        true,
                        true,
                    ),
                    "FfiEntity" | "TargetEntity" => (
                        size_of::<FfiEntity>(),
                        align_of::<FfiEntity>(),
                        BamFieldKind::U64,
                        true,
                        true,
                    ),
                    "BamComponent" => (
                        size_of::<BamComponent>(),
                        align_of::<BamComponent>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "BamSystem" => (
                        size_of::<BamSystem>(),
                        align_of::<BamSystem>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "BamSystemSet" => (
                        size_of::<BamSystemSet>(),
                        align_of::<BamSystemSet>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "BamSymbol" => (
                        size_of::<BamSymbol>(),
                        align_of::<BamSymbol>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "FfiAssetLoadState" => (
                        size_of::<FfiAssetLoadState>(),
                        align_of::<FfiAssetLoadState>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "BamEventKind" => (
                        size_of::<BamEventKind>(),
                        align_of::<BamEventKind>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "BamFieldKind" => (
                        size_of::<BamFieldKind>(),
                        align_of::<BamFieldKind>(),
                        BamFieldKind::U32,
                        true,
                        true,
                    ),
                    "FfiAssetHandle" => (
                        size_of::<FfiAssetHandle>(),
                        align_of::<FfiAssetHandle>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "FfiChildOf" => (
                        size_of::<FfiChildOf>(),
                        align_of::<FfiChildOf>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "ComponentUuid" => (
                        size_of::<ComponentUuid>(),
                        align_of::<ComponentUuid>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "BamChangeTicks" => (
                        size_of::<BamChangeTicks>(),
                        align_of::<BamChangeTicks>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "FfiDisabled" => (
                        size_of::<FfiDisabled>(),
                        align_of::<FfiDisabled>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "BamRowRange" => (
                        size_of::<BamRowRange>(),
                        align_of::<BamRowRange>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "BamTimeInfo" => (
                        size_of::<BamTimeInfo>(),
                        align_of::<BamTimeInfo>(),
                        BamFieldKind::NESTED,
                        true,
                        true,
                    ),
                    "String" => (
                        size_of::<[usize; 3]>(),
                        align_of::<[usize; 3]>(),
                        BamFieldKind::STRING,
                        false,
                        false,
                    ),
                    other => panic!("untested WIRE_TYPE_NAMES entry: {other}"),
                };
            assert_eq!(spec.canonical_tag, name, "{name} canonical_tag");
            assert_eq!(spec.size, expected_size, "{name} size");
            assert_eq!(spec.align, expected_align, "{name} align");
            assert_eq!(spec.kind.0, expected_kind.0, "{name} kind");
            assert_eq!(spec.is_abi_pod, expected_pod, "{name} is_abi_pod");
            assert_eq!(
                spec.is_raw_insertable, expected_raw,
                "{name} is_raw_insertable"
            );
        }
    }
}
