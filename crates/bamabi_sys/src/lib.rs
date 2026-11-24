#![no_std]
#![warn(missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]

//! # `bamabi_sys`: C-ABI `0.4.0` Wire Contract for `bamabi`
//!
//! This crate defines the `#![no_std]` C-ABI `0.4.0` (`0x0004_0000`) wire specification connecting
//! Bevy ECS host applications to native dynamic guest mods (`.so`, `.dylib`, `.dll`) written in
//! Rust, ISO C99, C++17/20, or Zig.
//!
//! Follow the six steps below in order to build a complete mental model of the crate.
//!
//! ## Workspace & Crate Documentation
//!
//! - **Crate guides:** [`docs::architecture`] (`crates/bamabi_sys/ARCHITECTURE.md`) · [`docs::tests`] (`crates/bamabi_sys/TESTS.md`)
//! - **Workspace guides:** [`bamabi_docs`](../bamabi_docs/index.html) ([`architecture`](../bamabi_docs/architecture/index.html), [`development`](../bamabi_docs/development/index.html), [`testing`](../bamabi_docs/testing/index.html), [`wire_spec`](../bamabi_docs/wire_spec/index.html))
//! - **Sibling crates:** [`bamabi_macros`](../bamabi_macros/index.html) · [`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html) · [`bamabi_guest_sys`](../bamabi_guest_sys/index.html) · [`bamabi_guest`](../bamabi_guest/index.html) · [`xtask`](../xtask/index.html)
//!
//! ---
//!
//! ## Step 1: Mental Model & Zero-Shared-Rust-ABI Design
//!
//! `bamabi_sys` sits at **Layer 0** of the `bamabi` workspace. It defines the binary wire boundary
//! between a Bevy host and dynamically loaded guest libraries without pulling in `bevy_*`, `std`,
//! or `alloc`:
//!
//! 1. **Zero Shared Rust ABI Assumptions:** Every wire struct is `#[repr(C)]` or
//!    `#[repr(transparent)]` with zero implicit padding, every foreign callback slot is
//!    `Option<unsafe extern "C" fn(...)>`, status/enum-like codes are `u32` newtypes rather than
//!    Rust `enum`s, and arrays/strings cross the boundary via [`FfiSlice`] / [`StrView`] or
//!    NUL-terminated C strings rather than Rust fat pointers (`&[T]`, `&str`, `dyn Trait`).
//! 2. **Strict Memory & Allocator Isolation (Owner-Frees):** Host-allocated memory (`World`
//!    columns, `String`/`Vec` component payloads, command arenas) is freed exclusively by the
//!    host, and guest-allocated memory (such as per-World [`BamInstance`] state) is freed
//!    exclusively by the guest in `bamabi_shutdown`.
//! 3. **Pre-Resolved Handles & Zero-Allocation Hot Paths:** Components and resources resolve once
//!    at load time into 1-based [`BamComponent`] handles; per-frame iteration, commands, and
//!    resource accessors perform zero string parsing or hashing.
//! 4. **Nounwind `extern "C"` Boundary:** Unwinding across `extern "C"` is forbidden. Unwinding
//!    Rust guests catch panics inside their own runtime and return [`FfiResult::PANIC_CAUGHT`].
//! 5. **Why `#![no_std]` and Zero External Dependencies?** Keeping `bamabi_sys` strictly `#![no_std]`
//!    with zero external dependencies guarantees that guest mods, proc-macro expansions, polyglot
//!    header generators, and the host engine share one identical binary contract without coupling
//!    to Bevy internals or a specific heap allocator.
//!
//! ---
//!
//! ## Step 2: Core Primitives & Handles
//!
//! All cross-boundary primitives avoid Rust-specific layout or invalid-discriminant hazards:
//!
//! - [`FfiResult`]: `#[repr(transparent)]` `u32` status newtype (`18` canonical codes `0..=16`
//!   plus `99`, including [`FfiResult::CYCLE_DETECTED`]); foreign integer values never cause UB.
//! - [`FfiBool`]: `#[repr(transparent)]` `u8` boolean (`0` = false, any non-zero `u8` = true),
//!   avoiding Rust `bool` validity UB on bytes `2..=255`.
//! - [`FfiOption`]: `#[repr(C)]` `{ is_some: FfiBool, value: T }` optional value wrapper replacing
//!   Rust `Option<T>` across C99/C++17/Zig.
//! - [`FfiSlice`] & [`StrView`]: Non-owning `{ ptr, len }` views (`StrView = FfiSlice<u8>`). Empty
//!   C slices `(NULL, 0)` safely substitute `NonNull::dangling()` in [`FfiSlice::as_slice`].
//! - [`BamComponent`]: 1-based world-scoped component/resource handle (`0` = [`BamComponent::INVALID`]).
//!   Paired with [`BamSystem`], [`BamSystemSet`], and [`BamSymbol`] (`u32::MAX` = `INVALID`).
//! - [`ComponentUuid`]: 16-byte (`align = 1`) deterministic component identity computed in `const`
//!   via keyed SipHash-2-4-128 ([`ComponentUuid::from_parts`], [`ComponentUuid::from_canonical_name`],
//!   [`ComponentUuid::hash_field_schema`], [`ComponentUuid::try_parse_hex`]).
//! - Built-in entity, hierarchy, disabling, and asset primitives: [`FfiEntity`], [`FfiChildOf`],
//!   [`FfiDisabled`], [`FfiAssetHandle`], and [`FfiAssetLoadState`].
//!
//! ```
//! use bamabi_sys::{
//!     BamComponent, ComponentUuid, FfiBool, FfiOption, FfiResult, StrView,
//! };
//!
//! let uuid = ComponentUuid::from_parts("my_game::Health", 4, 4, 0);
//! assert!(!uuid.is_zero());
//!
//! let handle = BamComponent::from_raw(1);
//! assert!(handle.is_valid());
//!
//! let opt = FfiOption::some(FfiBool::TRUE);
//! assert_eq!(opt.into_option(), Some(FfiBool::TRUE));
//!
//! let view = StrView::from_str("bamabi");
//! // SAFETY: `view` borrows the `'static` string literal `"bamabi"`.
//! assert_eq!(unsafe { view.try_as_str() }, Ok("bamabi"));
//! assert!(FfiResult::OK.is_ok());
//! ```
//!
//! ---
//!
//! ## Step 3: Host Contract & Versioning
//!
//! Version negotiation and ABI extension rely on packed SemVer words and `struct_size`-prefixed
//! tables:
//!
//! - [`ABI_VERSION`]: Packed `u32` `(major << 24) | (minor << 16) | patch` (`0x0003_0000` for
//!   `0.3.0`; mirrored in C99 as `BAM_ABI_VERSION`), manipulated via [`pack_abi_version`],
//!   [`unpack_abi_version`], and [`abi_version_is_compatible`]. Pre-1.0 requires exact `minor`
//!   equality (ignoring `patch`); post-1.0 accepts `guest_minor <= host_minor`.
//! - [`BamManifest`]: Static identity struct returned by `bamabi_manifest()` (`MIN_V0_2_SIZE` =
//!   `32` B / `24` B; full `48` B / `32` B with optional `save_state` / `restore_state` hot-reload
//!   callbacks).
//! - [`BamHostApi`]: Complete **73-slot** host function-pointer table passed to `bamabi_init`
//!   (`MIN_V0_2_SIZE` = `224` B / `116` B covering the 27 mandatory `0.2` slots; `MIN_V0_3_SIZE` =
//!   `552` B / `280` B covering the first 68 slots; full `592` B / `300` B with slots `68..=72`).
//!   Probes `struct_size` / `abi_version` safely via
//!   [`BamHostApi::raw_struct_size`], [`BamHostApi::raw_abi_version`], and [`BamHostApi::covers_slot`].
//! - [`offsets`]: Exports target-native (`<FIELD>`), 64-bit (`OFF64_<FIELD>`, [`offsets::SIZE_64`] =
//!   `592`), and 32-bit (`OFF32_<FIELD>`, [`offsets::SIZE_32`] = `300`) byte offsets for all 75
//!   fields (`struct_size`, `abi_version`, and slots `0..73`, [`offsets::SLOT_COUNT`] = `73`).
//! - [`ExtensibleStruct`]: Contract for all 13 `struct_size`-prefixed `#[repr(C)]` structs whose
//!   fields are zero-valid (`Option<unsafe extern "C" fn(...)>` niche optimization). Use
//!   [`read_extensible_struct`] and [`read_descriptor`] to copy `min(struct_size, size_of::<T>())`
//!   bytes, zero-extending missing trailing fields and truncating unknown future fields.
//!
//! ---
//!
//! ## Step 4: Descriptors & Component Traits
//!
//! Component metadata, POD safety proofs, and registration descriptors build on those primitives:
//!
//! - **Traits & VTable Headers:**
//!   - [`ModComponent`]: Trait generated by `#[derive(ModComponent)]` exposing canonical name,
//!     [`ComponentUuid`], `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL`,
//!     `ZIG_DECL`, and a static [`VTableHeader`]-prefixed VTable without requiring `Default` or
//!     [`RawInsertable`].
//!   - [`AbiPod`]: `unsafe trait` for cross-language Plain Old Data (`#[repr(C)]` /
//!     `#[repr(transparent)]`, `Copy`, no drop glue, zero implicit padding; excludes `u128`, `i128`,
//!     `usize`, `isize`, `bool`, and `char`).
//!   - [`RawInsertable`]: `unsafe trait` for same-process raw byte moves (includes `AbiPod` plus
//!     `u128`, `i128`, `usize`, `isize`).
//!   - [`VTableHeader`]: Self-describing header at byte offset `0` of every `<Struct>VTable`,
//!     [`MappedWireVTable`], and [`MappedStringVTable`].
//! - **ECS Descriptors:**
//!   - [`BamSystemDesc`], [`BamQueryDesc`], and [`BamTermDesc`] (with [`AccessMode`] and
//!     [`ScheduleId`]) define multi-query systems, `Or<(...)>` groups, schedule placement, ordering,
//!     and `run_if` predicates.
//!   - [`BamRelationDesc`] defines typed entity relationship flags (`EXCLUSIVE`, `CASCADE_DESPAWN`,
//!     `SYMMETRIC`, `ACYCLIC`, `CLEANUP_REMOVE`) and optional edge payloads.
//!   - [`BamScheduleSetDesc`] configures named schedule sets (`SEQUENCE`, `SEQUENCE_WEAK`,
//!     `PARALLEL`, `BEFORE_WEAK`, `AFTER_WEAK`).
//!   - [`BamObserverDesc`] and [`BamTriggerDesc`] configure custom, lifecycle (`ADD`..=`READY`),
//!     mutable, and relationship-propagating observers/triggers.
//!   - [`ComponentDesc`], [`BamFieldDesc`], [`BamRequiredComponent`], [`BamLayoutInfo`],
//!     [`BamColumnInfo`], and [`BamChangeTicks`] describe dynamic component registration, field
//!     reflection, required-component initializers, and column change tracking.
//!
//! ---
//!
//! ## Step 5: Module Map & Reading Order
//!
//! Except for [`host_api`] (which exposes [`offsets`]), internal modules are private and re-exported
//! flat at the crate root. Recommended source reading order:
//!
//! 1. `src/version.rs` — ABI version packing and SemVer rules.
//! 2. `src/primitives/` (`mod.rs`, `bool.rs`, `entity.rs`, `option.rs`, `slice.rs`, `asset.rs`,
//!    `uuid.rs`) — FFI-safe primitives, handles, and const SipHash-2-4-128.
//! 3. `src/extensible.rs` — [`ExtensibleStruct`] prefix copying and truncation.
//! 4. `src/access.rs` — [`AccessMode`], [`ScheduleId`], [`BamTermDesc`], and query limits.
//! 5. `src/component.rs` — [`ComponentDesc`], [`VTableHeader`], reflection, relations, and schedule sets.
//! 6. `src/system.rs` & `src/commands.rs` — [`BamQueryDesc`], [`BamSystemDesc`], [`BamObserverDesc`],
//!    [`BamTriggerDesc`], and deferred command prototypes.
//! 7. `src/iter.rs` — [`BamIter`], [`BamChangeTicks`], [`BamColumnInfo`], and iteration callbacks.
//! 8. `src/host_api.rs` — [`BamManifest`], 90-slot [`BamHostApi`], and [`offsets`].
//! 9. `src/traits.rs` — [`RawInsertable`], [`AbiPod`], [`SymbolEntry`], and [`ModComponent`].
//!
//! For deep architectural notes and a complete test/proof inventory, see `ARCHITECTURE.md` and
//! `TESTS.md` in `crates/bamabi_sys/`.
//!
//! ---
//!
//! ## Step 6: Setup, Build, Test & Formal Verification
//!
//! ```bash
//! # Check host and 32-bit / Wasm wire layout parity
//! cargo check -p bamabi_sys
//! cargo check -p bamabi_sys --target i686-unknown-linux-gnu
//! cargo check -p bamabi_sys --target wasm32-unknown-unknown
//!
//! # Lint and rustdoc gate (zero warnings)
//! cargo fmt -p bamabi_sys -- --check
//! cargo clippy -p bamabi_sys --all-targets --all-features -- -D warnings
//! RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sys --no-deps --all-features
//!
//! # Unit tests, property tests, and doctests
//! cargo test -p bamabi_sys --all-features
//! cargo test --doc -p bamabi_sys --all-features
//!
//! # Undefined Behavior & Formal Verification (nightly Miri + Kani)
//! MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_sys
//! cargo kani -p bamabi_sys
//! ```

pub mod abi_meta;
mod access;
mod commands;
mod component;
mod extensible;
pub mod host_api;
mod iter;
mod primitives;
mod system;
mod telemetry;
mod traits;
mod version;

/// Re-export of [`bamabi_schema`], the canonical zero-dependency schema, hashing, layout, and
/// ABI metadata SSOT crate.
pub use bamabi_schema as schema;

#[cfg(doc)]
pub mod docs {
    //! Embedded architecture and testing documentation for `bamabi_sys`.

    /// Crate architecture and C-ABI `0.4.0` wire specification internals (`crates/bamabi_sys/ARCHITECTURE.md`).
    pub mod architecture {
        #![doc = include_str!("../ARCHITECTURE.md")]
    }

    /// Crate test suite, proptest, Miri, and Kani formal proof catalog (`crates/bamabi_sys/TESTS.md`).
    pub mod tests {
        #![doc = include_str!("../TESTS.md")]
    }
}

#[cfg(test)]
mod tests;

#[cfg(kani)]
mod kani_proofs;

pub use access::{
    AccessMode, BAM_TERM_FLAG_ADDED, BAM_TERM_FLAG_CHANGED, BAM_TERM_KNOWN_FLAGS, BamTermDesc,
    MAX_QUERIES_PER_SYSTEM, MAX_TERMS_PER_QUERY, ScheduleId,
};
pub use commands::{
    BAM_SPAWN_COL_FLAG_SPLAT, BAM_SPAWN_COL_KNOWN_FLAGS, BAM_TRIGGER_FLAG_PROPAGATE,
    BAM_TRIGGER_KNOWN_FLAGS, BamCmdCloneEntityFn, BamCmdDespawnAllFn, BamCmdDespawnBatchFn,
    BamCmdDespawnFn, BamCmdInsertBatchFn, BamCmdInsertDefaultFn, BamCmdInsertFn,
    BamCmdInsertResourceFn, BamCmdObserveFn, BamCmdRemoveBatchFn, BamCmdRemoveFn,
    BamCmdRemoveResourceFn, BamCmdSpawnBatchExFn, BamCmdSpawnBatchFn, BamCmdSpawnEmptyFn,
    BamCmdSpawnFn, BamCmdSpawnSceneFn, BamCmdTriggerExFn, BamCmdTriggerFn, BamCommands,
    BamSpawnColumnDesc, BamTriggerDesc,
};
pub use component::{
    BAM_COMPONENT_FLAG_IMMUTABLE, BAM_COMPONENT_FLAG_SPARSE_SET, BAM_COMPONENT_FLAG_SUMMARY_TICK,
    BAM_COMPONENT_KNOWN_FLAGS, BAM_RELATION_FLAG_ACYCLIC, BAM_RELATION_FLAG_CASCADE_DELETE,
    BAM_RELATION_FLAG_CASCADE_DESPAWN, BAM_RELATION_FLAG_CLEANUP_REMOVE,
    BAM_RELATION_FLAG_EXCLUSIVE, BAM_RELATION_FLAG_SYMMETRIC, BAM_RELATION_KNOWN_FLAGS,
    BAM_REQUIRED_FLAG_CUSTOM_BYTES, BAM_REQUIRED_FLAG_INIT_FN, BAM_REQUIRED_FLAG_RESOLVE_BY_UUID,
    BAM_REQUIRED_KNOWN_FLAGS, BAM_SCHEDULE_SET_FLAG_AFTER_WEAK, BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK,
    BAM_SCHEDULE_SET_FLAG_PARALLEL, BAM_SCHEDULE_SET_FLAG_SEQUENCE,
    BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK, BAM_SCHEDULE_SET_KNOWN_FLAGS, BamFieldDesc, BamFieldKind,
    BamFieldMeta, BamLayoutInfo, BamRelationDesc, BamRequiredComponent, BamRequiredInitFn,
    BamRowMigrateFn, BamScheduleSetDesc, ComponentCloneFn, ComponentDesc, ComponentDropFn,
    ComponentHookFn, MappedStringGetFn, MappedStringSetFn, MappedStringVTable, MappedWireGetFn,
    MappedWireSetFn, MappedWireVTable, VTableHeader,
};
pub use extensible::{ExtensibleStruct, read_descriptor, read_extensible_struct};
pub use host_api::{
    BAM_HOST_API_SLOT_COUNT, BAM_LOG_DEBUG, BAM_LOG_ERROR, BAM_LOG_INFO, BAM_LOG_TRACE,
    BAM_LOG_WARN, BAM_MANIFEST_KNOWN_FLAGS, BamAssetGetLoadStateFn, BamAssetLoadFn,
    BamAssetReleaseFn, BamBindNamedObjectFn, BamCmdCheckpointFn, BamCmdInsertRelationFn,
    BamCmdRemoveRelationFn, BamCmdRollbackFn, BamComponentLayoutInfoFn, BamConfigureScheduleSetFn,
    BamGetProcAddressFn, BamHostApi, BamInitFn, BamInstance, BamInternSymbolFn, BamIterCountFn,
    BamIterUpCascadeFn, BamLastErrorFn, BamLoadHostCtxFn, BamLogDiagnosticFn, BamManifest,
    BamManifestFn, BamMigrateComponentColumnFn, BamMountSystemInScheduleFn, BamRegisterComponentFn,
    BamRegisterObserverFn, BamRegisterRelationFn, BamRegisterSystemFn, BamRegisterSystemHandleFn,
    BamReportGuestFaultFn, BamResolveComponentByUuidFn, BamResolveComponentFn,
    BamResolveNamedObjectFn, BamResolveSymbolStrFn, BamRestoreStateFn, BamSaveStateFn,
    BamShutdownFn, BamStateWriterFn, BamSysCommandsFn, BamSysEntityChildrenFn, BamSysEntityGetFn,
    BamSysEntityGetMutFn, BamSysHostCtxFn, BamSysQueryFn, BamSysResMutPtrFn, BamSysResPtrFn,
    offsets,
};
pub use iter::{
    BAM_MAX_CHANGE_AGE, BAM_STORAGE_FLAG_ENTITY_EVENT, BAM_STORAGE_FLAG_EVENT,
    BAM_STORAGE_FLAG_IMMUTABLE, BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE,
    BAM_STORAGE_FLAG_SPARSE_SET, BAM_STORAGE_FLAG_SUMMARY_TICK, BamChangeTicks, BamChunkTaskExFn,
    BamChunkTaskFn, BamColumnInfo, BamIter, BamIterColumnInfoFn, BamIterColumnMutableFn,
    BamIterColumnMutableUntrackedFn, BamIterColumnReadonlyFn, BamIterColumnRowsMutableFn,
    BamIterColumnRowsMutableUntrackedFn, BamIterColumnRowsReadonlyFn, BamIterColumnTicksFn,
    BamIterEntitiesFn, BamIterMarkChangedRangeFn, BamIterMarkChangedRangesFn,
    BamIterNextChunkBatchFn, BamIterNextChunkFn, BamIterReadRemovedFn, BamIterResetFn,
    BamMsgReadBatchFn, BamMsgReadSliceFn, BamMsgSendFn, BamMsgWriteBatchFn, BamParForEachChunkExFn,
    BamParForEachChunkFn, BamParOptions, BamResMutPtrFn, BamResPtrFn, BamResTicksFn, BamRowRange,
    DirtyRunCoalescer,
};
pub use primitives::{
    BAM_CHILD_OF_NAME, BAM_CHILDREN_NAME, BAM_DISABLED_NAME, BamComponent, BamSymbol, BamSystem,
    BamSystemSet, ComponentUuid, FNV1A_64_OFFSET_BASIS, FNV1A_64_PRIME, FfiAssetHandle,
    FfiAssetLoadState, FfiBool, FfiChildOf, FfiDisabled, FfiEntity, FfiOption, FfiResult, FfiSlice,
    SIPHASH_KEY_0, SIPHASH_KEY_1, StrView, fnv1a_64_init, fnv1a_64_str, fnv1a_64_update_bytes,
    fnv1a_64_update_u64, siphash24_128,
};
pub use system::{
    BAM_OBSERVER_FLAG_PROPAGATE, BAM_OBSERVER_KNOWN_FLAGS, BAM_QUERY_FLAG_PARAM_SET,
    BAM_QUERY_KNOWN_FLAGS, BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL, BAM_SYSTEM_FLAG_MAIN_THREAD,
    BAM_SYSTEM_KNOWN_FLAGS, BamEventKind, BamHostCtx, BamLoadCtx, BamObserverCtxFn,
    BamObserverCtxMutFn, BamObserverDesc, BamObserverFn, BamObserverMutFn, BamQueryDesc,
    BamRunIfFn, BamSysCurrentTicksFn, BamSysRelationEdgeDataFn, BamSysRelationSourcesFn,
    BamSysRelationTargetsFn, BamSysTimeInfoFn, BamSystemCtx, BamSystemDesc, BamSystemFn,
    BamTimeInfo,
};
pub use telemetry::{
    BRP_METHOD_BAM_COMPONENT_SCHEMAS, BRP_METHOD_BAM_HOT_RELOAD, BRP_METHOD_BAM_LIST_MODS,
    BRP_METHOD_BAM_SYSTEM_METRICS, SYSTEM_TELEMETRY_DIGEST_SIZE, SYSTEM_TELEMETRY_DIGEST_VERSION,
    SYSTEM_TELEMETRY_MAGIC, SystemTelemetryDigest64, TELEMETRY_FLAG_BUDGET_BREACHED,
    TELEMETRY_FLAG_FAULTED, TELEMETRY_FLAG_QUARANTINED, TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE,
    TelemetryFrameError, TelemetrySliceError, fnv1a_64_hash_system,
};
#[cfg(feature = "serde")]
pub use telemetry::{
    BrpComponentSchema, BrpFieldSchema, BrpHotReloadRequest, BrpModInfo, BrpSystemMetrics,
};
pub use traits::{
    AbiPod, ModComponent, RawInsertable, SymbolEntry, assert_abi_pod, assert_raw_insertable,
};
pub use version::{
    ABI_VERSION, ABI_VERSION_MAJOR, ABI_VERSION_MINOR, ABI_VERSION_PATCH,
    abi_version_is_compatible, abi_version_major, abi_version_minor, abi_version_patch,
    pack_abi_version, unpack_abi_version,
};
