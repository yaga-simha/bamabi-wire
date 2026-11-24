//! Single-source-of-truth ABI `0.4.0` wire struct layouts and `BamHostApi` slot metadata
//! parameterized across 64-bit (`LP64`/`LLP64`) and 32-bit (`ILP32`, `u64` align 4 or 8) targets.

use crate::wire_types::TargetLayout;

/// Total number of function-pointer slots in `BamHostApi` (`0..=89`).
pub const HOST_API_SLOT_COUNT: usize = 90;

/// Total byte size of `BamHostApi` on 64-bit targets (`8 + 8 * 90 = 728`).
pub const HOST_API_SIZE_64: usize = 8 + 8 * HOST_API_SLOT_COUNT;

/// Total byte size of `BamHostApi` on 32-bit targets (`8 + 4 * 90 = 368`).
pub const HOST_API_SIZE_32: usize = 8 + 4 * HOST_API_SLOT_COUNT;

/// 32-bit target alignment rule distinguishing fixed scalar alignment from `alignof(uint64_t)`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Align32 {
    /// Fixed byte alignment independent of `alignof(uint64_t)`.
    Fixed(usize),
    /// Alignment governed by `target.u64_align` (`4` on i686 SysV, `8` on Wasm32/ARMv7).
    U64,
}

impl Align32 {
    /// Resolves concrete byte alignment on a 32-bit `target`.
    #[inline]
    #[must_use]
    pub const fn resolve(self, target: TargetLayout) -> usize {
        match self {
            Self::Fixed(a) => a,
            Self::U64 => target.u64_align,
        }
    }
}

/// Wire struct field layout entry parameterized across 64-bit and 32-bit targets.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FieldLayoutSpec {
    /// Canonical C99 field identifier.
    pub c_name: &'static str,
    /// Byte offset on 64-bit targets (`ptr_bytes == 8`).
    pub offset_64: usize,
    /// Byte offset on 32-bit targets (`ptr_bytes == 4`).
    pub offset_32: usize,
}

impl FieldLayoutSpec {
    /// Returns the expected field byte offset for `target`.
    #[inline]
    #[must_use]
    pub const fn offset_for(&self, target: TargetLayout) -> usize {
        if target.ptr_bytes == 8 {
            self.offset_64
        } else {
            self.offset_32
        }
    }
}

/// Complete size, alignment, and field-offset specification for one C-ABI wire struct.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct StructLayoutSpec {
    /// Canonical C99 struct name (e.g. `"BamHostApi"`).
    pub c_name: &'static str,
    /// Lowercase assertion identifier prefix (`"bam_host_api"`).
    pub assert_stem: &'static str,
    /// Byte size on 64-bit targets.
    pub size_64: usize,
    /// Byte alignment on 64-bit targets.
    pub align_64: usize,
    /// Byte size on 32-bit targets.
    pub size_32: usize,
    /// Alignment rule on 32-bit targets.
    pub align_32: Align32,
    /// Ordered field offsets.
    pub fields: &'static [FieldLayoutSpec],
}

impl StructLayoutSpec {
    /// Returns the expected byte size for `target`.
    #[inline]
    #[must_use]
    pub const fn size_for(&self, target: TargetLayout) -> usize {
        if target.ptr_bytes == 8 {
            self.size_64
        } else {
            self.size_32
        }
    }

    /// Returns the expected byte alignment for `target`.
    #[inline]
    #[must_use]
    pub const fn align_for(&self, target: TargetLayout) -> usize {
        if target.ptr_bytes == 8 {
            self.align_64
        } else {
            self.align_32.resolve(target)
        }
    }
}

/// Metadata descriptor for a single function-pointer slot inside `BamHostApi`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct HostApiSlotSpec {
    /// 0-based slot index (`0..90`).
    pub index: usize,
    /// Rust/C struct field name.
    pub name: &'static str,
    /// C typedef name for this slot's function pointer type.
    pub c_typedef: &'static str,
    /// 64-bit byte offset (`8 + 8 * index`).
    pub offset_64: usize,
    /// 32-bit byte offset (`8 + 4 * index`).
    pub offset_32: usize,
    /// Minimum ABI version (`"0.2"`, `"0.3"`, or `"0.4"`) that introduced this slot.
    pub since_abi: &'static str,
}

impl HostApiSlotSpec {
    /// Returns the slot's byte offset in `BamHostApi` for `target`.
    #[inline]
    #[must_use]
    pub const fn offset_for(&self, target: TargetLayout) -> usize {
        if target.ptr_bytes == 8 {
            self.offset_64
        } else {
            self.offset_32
        }
    }
}

/// Returns the expected total byte size of `BamHostApi` for `target`.
#[inline]
#[must_use]
pub const fn host_api_size(target: TargetLayout) -> usize {
    if target.ptr_bytes == 8 {
        HOST_API_SIZE_64
    } else {
        HOST_API_SIZE_32
    }
}

#[inline]
const fn slot(
    index: usize,
    name: &'static str,
    c_typedef: &'static str,
    since_abi: &'static str,
) -> HostApiSlotSpec {
    HostApiSlotSpec {
        index,
        name,
        c_typedef,
        offset_64: 8 + 8 * index,
        offset_32: 8 + 4 * index,
        since_abi,
    }
}

/// Canonical ordered metadata table for all 90 `BamHostApi` slots (`0..=89`).
pub const HOST_API_SLOTS: [HostApiSlotSpec; HOST_API_SLOT_COUNT] = [
    slot(0, "get_proc_address", "BamGetProcAddressFn", "0.2"),
    slot(1, "last_error", "BamLastErrorFn", "0.2"),
    slot(2, "load_host_ctx", "BamLoadHostCtxFn", "0.2"),
    slot(3, "register_component", "BamRegisterComponentFn", "0.2"),
    slot(4, "resolve_component", "BamResolveComponentFn", "0.2"),
    slot(5, "register_system", "BamRegisterSystemFn", "0.2"),
    slot(6, "sys_host_ctx", "BamSysHostCtxFn", "0.2"),
    slot(7, "sys_query", "BamSysQueryFn", "0.2"),
    slot(8, "sys_commands", "BamSysCommandsFn", "0.2"),
    slot(9, "iter_next_chunk", "BamIterNextChunkFn", "0.2"),
    slot(10, "iter_reset", "BamIterResetFn", "0.2"),
    slot(11, "iter_entities", "BamIterEntitiesFn", "0.2"),
    slot(12, "iter_column_readonly", "BamIterColumnReadonlyFn", "0.2"),
    slot(13, "iter_column_mutable", "BamIterColumnMutableFn", "0.2"),
    slot(
        14,
        "iter_column_mutable_untracked",
        "BamIterColumnMutableUntrackedFn",
        "0.2",
    ),
    slot(
        15,
        "iter_mark_changed_range",
        "BamIterMarkChangedRangeFn",
        "0.2",
    ),
    slot(
        16,
        "iter_column_rows_readonly",
        "BamIterColumnRowsReadonlyFn",
        "0.2",
    ),
    slot(
        17,
        "iter_column_rows_mutable",
        "BamIterColumnRowsMutableFn",
        "0.2",
    ),
    slot(18, "iter_column_info", "BamIterColumnInfoFn", "0.2"),
    slot(19, "res_ptr", "BamResPtrFn", "0.2"),
    slot(20, "res_mut_ptr", "BamResMutPtrFn", "0.2"),
    slot(21, "cmd_spawn_empty", "BamCmdSpawnEmptyFn", "0.2"),
    slot(22, "cmd_spawn", "BamCmdSpawnFn", "0.2"),
    slot(23, "cmd_despawn", "BamCmdDespawnFn", "0.2"),
    slot(24, "cmd_insert", "BamCmdInsertFn", "0.2"),
    slot(25, "cmd_insert_default", "BamCmdInsertDefaultFn", "0.2"),
    slot(26, "cmd_remove", "BamCmdRemoveFn", "0.2"),
    slot(27, "par_for_each_chunk", "BamParForEachChunkFn", "0.2"),
    slot(28, "sys_entity_get", "BamSysEntityGetFn", "0.2"),
    slot(29, "sys_entity_get_mut", "BamSysEntityGetMutFn", "0.2"),
    slot(30, "msg_read_batch", "BamMsgReadBatchFn", "0.2"),
    slot(31, "msg_send", "BamMsgSendFn", "0.2"),
    slot(32, "msg_write_batch", "BamMsgWriteBatchFn", "0.2"),
    slot(33, "res_ticks", "BamResTicksFn", "0.2"),
    slot(34, "sys_res_ptr", "BamSysResPtrFn", "0.2"),
    slot(35, "sys_res_mut_ptr", "BamSysResMutPtrFn", "0.2"),
    slot(36, "register_observer", "BamRegisterObserverFn", "0.2"),
    slot(37, "cmd_trigger", "BamCmdTriggerFn", "0.2"),
    slot(38, "cmd_spawn_persistent", "BamCmdSpawnFn", "0.2"),
    slot(39, "cmd_spawn_batch", "BamCmdSpawnBatchFn", "0.2"),
    slot(40, "cmd_despawn_all", "BamCmdDespawnAllFn", "0.2"),
    slot(41, "cmd_insert_if_new", "BamCmdInsertFn", "0.2"),
    slot(42, "cmd_trigger_ex", "BamCmdTriggerExFn", "0.2"),
    slot(43, "cmd_observe", "BamCmdObserveFn", "0.2"),
    slot(44, "iter_read_removed", "BamIterReadRemovedFn", "0.2"),
    slot(45, "sys_entity_children", "BamSysEntityChildrenFn", "0.2"),
    slot(46, "asset_load", "BamAssetLoadFn", "0.2"),
    slot(47, "asset_get_load_state", "BamAssetGetLoadStateFn", "0.2"),
    slot(48, "asset_release", "BamAssetReleaseFn", "0.2"),
    slot(49, "cmd_spawn_scene", "BamCmdSpawnSceneFn", "0.2"),
    slot(50, "log_diagnostic", "BamLogDiagnosticFn", "0.3"),
    slot(51, "report_guest_fault", "BamReportGuestFaultFn", "0.3"),
    slot(
        52,
        "resolve_component_by_uuid",
        "BamResolveComponentByUuidFn",
        "0.3",
    ),
    slot(
        53,
        "component_layout_info",
        "BamComponentLayoutInfoFn",
        "0.3",
    ),
    slot(54, "register_relation", "BamRegisterRelationFn", "0.3"),
    slot(55, "intern_symbol", "BamInternSymbolFn", "0.3"),
    slot(56, "bind_named_object", "BamBindNamedObjectFn", "0.3"),
    slot(57, "iter_count", "BamIterCountFn", "0.3"),
    slot(58, "iter_up_cascade", "BamIterUpCascadeFn", "0.3"),
    slot(
        59,
        "par_for_each_chunk_readonly",
        "BamParForEachChunkFn",
        "0.3",
    ),
    slot(
        60,
        "register_system_handle",
        "BamRegisterSystemHandleFn",
        "0.3",
    ),
    slot(
        61,
        "configure_schedule_set",
        "BamConfigureScheduleSetFn",
        "0.3",
    ),
    slot(
        62,
        "mount_system_in_schedule",
        "BamMountSystemInScheduleFn",
        "0.3",
    ),
    slot(63, "cmd_checkpoint", "BamCmdCheckpointFn", "0.3"),
    slot(64, "cmd_rollback", "BamCmdRollbackFn", "0.3"),
    slot(65, "cmd_insert_relation", "BamCmdInsertRelationFn", "0.3"),
    slot(66, "cmd_remove_relation", "BamCmdRemoveRelationFn", "0.3"),
    slot(
        67,
        "migrate_component_column",
        "BamMigrateComponentColumnFn",
        "0.3",
    ),
    slot(
        68,
        "iter_next_chunk_batch",
        "BamIterNextChunkBatchFn",
        "0.3",
    ),
    slot(69, "iter_column_ticks", "BamIterColumnTicksFn", "0.3"),
    slot(70, "cmd_clone_entity", "BamCmdCloneEntityFn", "0.3"),
    slot(71, "cmd_insert_resource", "BamCmdInsertResourceFn", "0.3"),
    slot(72, "cmd_remove_resource", "BamCmdRemoveResourceFn", "0.3"),
    slot(73, "register_resource", "BamRegisterComponentFn", "0.4"),
    slot(74, "register_message", "BamRegisterComponentFn", "0.4"),
    slot(75, "resolve_named_object", "BamResolveNamedObjectFn", "0.4"),
    slot(76, "resolve_symbol_str", "BamResolveSymbolStrFn", "0.4"),
    slot(
        77,
        "iter_column_rows_mutable_untracked",
        "BamIterColumnRowsMutableUntrackedFn",
        "0.4",
    ),
    slot(
        78,
        "iter_mark_changed_ranges",
        "BamIterMarkChangedRangesFn",
        "0.4",
    ),
    slot(79, "sys_current_ticks", "BamSysCurrentTicksFn", "0.4"),
    slot(80, "sys_time_info", "BamSysTimeInfoFn", "0.4"),
    slot(81, "par_for_each_chunk_ex", "BamParForEachChunkExFn", "0.4"),
    slot(82, "sys_relation_targets", "BamSysRelationTargetsFn", "0.4"),
    slot(83, "sys_relation_sources", "BamSysRelationSourcesFn", "0.4"),
    slot(
        84,
        "sys_relation_edge_data",
        "BamSysRelationEdgeDataFn",
        "0.4",
    ),
    slot(85, "cmd_despawn_batch", "BamCmdDespawnBatchFn", "0.4"),
    slot(86, "cmd_insert_batch", "BamCmdInsertBatchFn", "0.4"),
    slot(87, "cmd_remove_batch", "BamCmdRemoveBatchFn", "0.4"),
    slot(88, "msg_read_slice", "BamMsgReadSliceFn", "0.4"),
    slot(89, "cmd_spawn_batch_ex", "BamCmdSpawnBatchExFn", "0.4"),
];

const BAM_HOST_API_FIELDS: [FieldLayoutSpec; 2 + HOST_API_SLOT_COUNT] = {
    let mut fields = [FieldLayoutSpec {
        c_name: "",
        offset_64: 0,
        offset_32: 0,
    }; 2 + HOST_API_SLOT_COUNT];
    fields[0] = FieldLayoutSpec {
        c_name: "struct_size",
        offset_64: 0,
        offset_32: 0,
    };
    fields[1] = FieldLayoutSpec {
        c_name: "abi_version",
        offset_64: 4,
        offset_32: 4,
    };
    let mut i = 0;
    while i < HOST_API_SLOT_COUNT {
        let s = &HOST_API_SLOTS[i];
        fields[2 + i] = FieldLayoutSpec {
            c_name: s.name,
            offset_64: s.offset_64,
            offset_32: s.offset_32,
        };
        i += 1;
    }
    fields
};

#[inline]
const fn f(c_name: &'static str, offset_64: usize, offset_32: usize) -> FieldLayoutSpec {
    FieldLayoutSpec {
        c_name,
        offset_64,
        offset_32,
    }
}

/// Complete layout specification slice for all ABI `0.4.0` C-ABI wire structures.
pub const WIRE_STRUCT_SPECS: &[StructLayoutSpec] = &[
    StructLayoutSpec {
        c_name: "BamComponentUuid",
        assert_stem: "bam_component_uuid",
        size_64: 16,
        align_64: 1,
        size_32: 16,
        align_32: Align32::Fixed(1),
        fields: &[f("bytes", 0, 0)],
    },
    StructLayoutSpec {
        c_name: "BamStrView",
        assert_stem: "bam_str_view",
        size_64: 16,
        align_64: 8,
        size_32: 8,
        align_32: Align32::Fixed(4),
        fields: &[f("ptr", 0, 0), f("len", 8, 4)],
    },
    StructLayoutSpec {
        c_name: "BamManifest",
        assert_stem: "bam_manifest",
        size_64: 48,
        align_64: 8,
        size_32: 32,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("abi_version", 4, 4),
            f("name", 8, 8),
            f("version", 16, 12),
            f("flags", 24, 16),
            f("reserved0", 28, 20),
            f("save_state", 32, 24),
            f("restore_state", 40, 28),
        ],
    },
    StructLayoutSpec {
        c_name: "BamAssetHandle",
        assert_stem: "bam_asset_handle",
        size_64: 32,
        align_64: 4,
        size_32: 32,
        align_32: Align32::Fixed(4),
        fields: &[
            f("kind", 0, 0),
            f("reserved0", 1, 1),
            f("asset_type", 4, 4),
            f("index", 8, 8),
            f("generation", 12, 12),
            f("uuid", 16, 16),
        ],
    },
    StructLayoutSpec {
        c_name: "BamTermDesc",
        assert_stem: "bam_term_desc",
        size_64: 12,
        align_64: 4,
        size_32: 12,
        align_32: Align32::Fixed(4),
        fields: &[
            f("component", 0, 0),
            f("mode", 4, 4),
            f("flags", 8, 8),
            f("or_group", 10, 10),
            f("reserved0", 11, 11),
        ],
    },
    StructLayoutSpec {
        c_name: "BamQueryDesc",
        assert_stem: "bam_query_desc",
        size_64: 32,
        align_64: 8,
        size_32: 24,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("terms", 8, 8),
            f("term_count", 16, 12),
            f("param_set_group", 24, 16),
            f("reserved0", 28, 20),
        ],
    },
    StructLayoutSpec {
        c_name: "BamSystemDesc",
        assert_stem: "bam_system_desc",
        size_64: 192,
        align_64: 8,
        size_32: 104,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("name", 8, 8),
            f("queries", 16, 12),
            f("query_count", 24, 16),
            f("callback", 32, 20),
            f("user_data", 40, 24),
            f("schedule", 48, 28),
            f("reserved0", 52, 32),
            f("after", 56, 36),
            f("after_count", 64, 40),
            f("before", 72, 44),
            f("before_count", 80, 48),
            f("run_if", 88, 52),
            f("run_if_user_data", 96, 56),
            f("in_sets", 104, 60),
            f("in_set_count", 112, 64),
            f("after_weak", 120, 68),
            f("after_weak_count", 128, 72),
            f("before_weak", 136, 76),
            f("before_weak_count", 144, 80),
            f("ambiguous_with", 152, 84),
            f("ambiguous_with_count", 160, 88),
            f("run_if_ctx", 168, 92),
            f("run_if_query", 176, 96),
            f("run_if_ctx_user_data", 184, 100),
        ],
    },
    StructLayoutSpec {
        c_name: "BamObserverDesc",
        assert_stem: "bam_observer_desc",
        size_64: 120,
        align_64: 8,
        size_32: 80,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("name", 8, 8),
            f("event", 16, 12),
            f("entity_lo", 20, 16),
            f("entity_hi", 24, 20),
            f("reserved0", 28, 24),
            f("callback", 32, 28),
            f("user_data", 40, 32),
            f("event_kind", 48, 36),
            f("reserved1", 52, 40),
            f("watched_components", 56, 44),
            f("watched_component_count", 64, 48),
            f("mut_callback", 72, 52),
            f("propagate_relationship", 80, 56),
            f("reserved2", 84, 60),
            f("ctx_callback", 88, 64),
            f("queries", 96, 68),
            f("query_count", 104, 72),
            f("ctx_mut_callback", 112, 76),
        ],
    },
    StructLayoutSpec {
        c_name: "BamTriggerDesc",
        assert_stem: "bam_trigger_desc",
        size_64: 64,
        align_64: 8,
        size_32: 48,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("event", 8, 8),
            f("event_kind", 12, 12),
            f("entity_lo", 16, 16),
            f("entity_hi", 20, 20),
            f("data_ptr", 24, 24),
            f("data_len", 32, 28),
            f("watched_components", 40, 32),
            f("watched_component_count", 48, 36),
            f("propagate_relationship", 56, 40),
            f("reserved0", 60, 44),
        ],
    },
    StructLayoutSpec {
        c_name: "BamChangeTicks",
        assert_stem: "bam_change_ticks",
        size_64: 16,
        align_64: 4,
        size_32: 16,
        align_32: Align32::Fixed(4),
        fields: &[
            f("added", 0, 0),
            f("changed", 4, 4),
            f("last_run", 8, 8),
            f("this_run", 12, 12),
        ],
    },
    StructLayoutSpec {
        c_name: "BamFieldDesc",
        assert_stem: "bam_field_desc",
        size_64: 64,
        align_64: 8,
        size_32: 36,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("kind", 4, 4),
            f("name", 8, 8),
            f("type_tag", 24, 16),
            f("offset", 40, 24),
            f("size", 48, 28),
            f("align", 56, 32),
        ],
    },
    StructLayoutSpec {
        c_name: "BamRequiredComponent",
        assert_stem: "bam_required_component",
        size_64: 64,
        align_64: 8,
        size_32: 48,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("component", 8, 8),
            f("reserved0", 12, 12),
            f("uuid", 16, 16),
            f("init_bytes", 32, 32),
            f("init_size", 40, 36),
            f("init_fn", 48, 40),
            f("user_data", 56, 44),
        ],
    },
    StructLayoutSpec {
        c_name: "BamRelationDesc",
        assert_stem: "bam_relation_desc",
        size_64: 56,
        align_64: 8,
        size_32: 44,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("name", 8, 8),
            f("uuid", 16, 12),
            f("data_size", 32, 28),
            f("data_align", 40, 32),
            f("max_degree", 48, 36),
            f("reserved0", 52, 40),
        ],
    },
    StructLayoutSpec {
        c_name: "BamLayoutInfo",
        assert_stem: "bam_layout_info",
        size_64: 64,
        align_64: 8,
        size_32: 48,
        align_32: Align32::U64,
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("uuid", 8, 8),
            f("schema_fingerprint", 24, 24),
            f("size", 32, 32),
            f("align", 40, 36),
            f("fields", 48, 40),
            f("field_count", 56, 44),
        ],
    },
    StructLayoutSpec {
        c_name: "BamScheduleSetDesc",
        assert_stem: "bam_schedule_set_desc",
        size_64: 88,
        align_64: 8,
        size_32: 60,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("schedule", 8, 8),
            f("before_count", 12, 12),
            f("after_count", 16, 16),
            f("reserved0", 20, 20),
            f("name", 24, 24),
            f("before_sets", 32, 28),
            f("after_sets", 40, 32),
            f("run_if_ctx", 48, 36),
            f("run_if_query", 56, 40),
            f("run_if_ctx_user_data", 64, 44),
            f("parent_sets", 72, 48),
            f("parent_set_count", 80, 52),
            f("reserved1", 84, 56),
        ],
    },
    StructLayoutSpec {
        c_name: "BamComponentDesc",
        assert_stem: "bam_component_desc",
        size_64: 176,
        align_64: 8,
        size_32: 104,
        align_32: Align32::U64,
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("name", 8, 8),
            f("uuid", 16, 12),
            f("size", 32, 28),
            f("align", 40, 32),
            f("drop_fn", 48, 36),
            f("vtable_ptr", 56, 40),
            f("on_add", 64, 44),
            f("on_remove", 72, 48),
            f("hook_user_data", 80, 52),
            f("on_insert", 88, 56),
            f("on_discard", 96, 60),
            f("on_despawn", 104, 64),
            f("clone_fn", 112, 68),
            f("required_components", 120, 72),
            f("required_component_count", 128, 76),
            f("schema_fingerprint", 136, 80),
            f("fields", 144, 88),
            f("field_count", 152, 92),
            f("required_init_descs", 160, 96),
            f("required_init_desc_count", 168, 100),
        ],
    },
    StructLayoutSpec {
        c_name: "BamColumnInfo",
        assert_stem: "bam_column_info",
        size_64: 48,
        align_64: 8,
        size_32: 40,
        align_32: Align32::Fixed(4),
        fields: &[
            f("component", 0, 0),
            f("mode", 4, 4),
            f("uuid", 8, 8),
            f("size", 24, 24),
            f("align", 32, 28),
            f("storage_flags", 40, 32),
            f("reserved0", 44, 36),
        ],
    },
    StructLayoutSpec {
        c_name: "BamVTableHeader",
        assert_stem: "bam_vtable_header",
        size_64: 40,
        align_64: 8,
        size_32: 32,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("abi_version", 4, 4),
            f("uuid", 8, 8),
            f("size", 24, 24),
            f("align", 32, 28),
        ],
    },
    StructLayoutSpec {
        c_name: "BamMappedStringVTable",
        assert_stem: "bam_mapped_string_vtable",
        size_64: 56,
        align_64: 8,
        size_32: 40,
        align_32: Align32::Fixed(4),
        fields: &[f("header", 0, 0), f("get", 40, 32), f("set", 48, 36)],
    },
    StructLayoutSpec {
        c_name: "BamRowRange",
        assert_stem: "bam_row_range",
        size_64: 16,
        align_64: 8,
        size_32: 8,
        align_32: Align32::Fixed(4),
        fields: &[f("start_row", 0, 0), f("row_count", 8, 4)],
    },
    StructLayoutSpec {
        c_name: "BamTimeInfo",
        assert_stem: "bam_time_info",
        size_64: 32,
        align_64: 8,
        size_32: 32,
        align_32: Align32::U64,
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("delta_secs", 8, 8),
            f("fixed_delta_secs", 12, 12),
            f("elapsed_secs", 16, 16),
            f("elapsed_secs_wrapped", 24, 24),
            f("overstep_fraction", 28, 28),
        ],
    },
    StructLayoutSpec {
        c_name: "BamParOptions",
        assert_stem: "bam_par_options",
        size_64: 24,
        align_64: 8,
        size_32: 16,
        align_32: Align32::Fixed(4),
        fields: &[
            f("struct_size", 0, 0),
            f("flags", 4, 4),
            f("chunk_size", 8, 8),
            f("max_partitions", 16, 12),
        ],
    },
    StructLayoutSpec {
        c_name: "BamSpawnColumnDesc",
        assert_stem: "bam_spawn_column_desc",
        size_64: 24,
        align_64: 8,
        size_32: 16,
        align_32: Align32::Fixed(4),
        fields: &[
            f("component", 0, 0),
            f("flags", 4, 4),
            f("column_ptr", 8, 8),
            f("elem_size", 16, 12),
        ],
    },
    StructLayoutSpec {
        c_name: "BamHostApi",
        assert_stem: "bam_host_api",
        size_64: HOST_API_SIZE_64,
        align_64: 8,
        size_32: HOST_API_SIZE_32,
        align_32: Align32::Fixed(4),
        fields: &BAM_HOST_API_FIELDS,
    },
];

/// Returns the static slice of all ABI `0.4.0` wire struct layout specs.
#[inline]
#[must_use]
pub const fn wire_struct_specs() -> &'static [StructLayoutSpec] {
    WIRE_STRUCT_SPECS
}

/// Returns the static slice of all 90 `BamHostApi` slot metadata specs (`0..=89`).
#[inline]
#[must_use]
pub const fn host_api_slot_specs() -> &'static [HostApiSlotSpec] {
    &HOST_API_SLOTS
}

/// Looks up a wire struct layout specification by exact C99 name (e.g. `"BamHostApi"`).
#[must_use]
pub fn find_wire_struct_spec(c_name: &str) -> Option<&'static StructLayoutSpec> {
    WIRE_STRUCT_SPECS.iter().find(|s| s.c_name == c_name)
}
