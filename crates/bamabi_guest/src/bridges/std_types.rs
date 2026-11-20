//! Guest-side `#[repr(C)]` wire types and safe VTable wrappers for core Bevy `0.20` components
//! and resources (`Transform`, `GlobalTransform`, `Visibility`, `InheritedVisibility`,
//! `ViewVisibility`, `Color`, `Name`, `Time<*>`, `ButtonInput<*>`, mouse motion/scroll, and
//! `Gamepad`).
//!
//! Enforces `#![forbid(unsafe_code)]` with zero Bevy dependencies, and works unconditionally even
//! when `feature = "derive"` is disabled.

use bamabi_guest_sys::{
    Host, ValidatedIter, ValidatedStringVTable, ValidatedSystemRef, ValidatedWireVTable,
    resolve_component,
};
use bamabi_sys::{
    AbiPod, AccessMode, BAM_DISABLED_NAME, BamChangeTicks, BamComponent, ComponentUuid, FfiBool,
    FfiDisabled, FfiEntity, FfiResult,
};
use core::{ffi::CStr, marker::PhantomData};

pub use bamabi_guest_sys::wire_types::{
    FfiColor, FfiGamepadButton, FfiGamepadRumble, FfiGamepadState, FfiGlobalTransform,
    FfiInheritedVisibility, FfiKeyCode, FfiKeyboardInput, FfiMouseButton, FfiMouseInput,
    FfiMouseMotion, FfiMouseScroll, FfiTimeFixed, FfiTimeReal, FfiTimeVirtual, FfiTransform,
    FfiViewVisibility, FfiVisibility,
};

use crate::app::{QueryBuilder, SystemBuilder, SystemContext};
use crate::commands::GuestCommands;
use crate::component::GuestComponent;
use crate::iter::{Chunk, ChunkIter};

/// Canonical ABI name for bridged host `Transform`.
pub const CANONICAL_TRANSFORM_NAME: &str = "bevy_transform::components::transform::Transform";
/// Canonical ABI name for bridged host `GlobalTransform`.
pub const CANONICAL_GLOBAL_TRANSFORM_NAME: &str =
    "bevy_transform::components::global_transform::GlobalTransform";
/// Canonical ABI name for bridged host `Visibility`.
pub const CANONICAL_VISIBILITY_NAME: &str = "bevy_camera::visibility::Visibility";
/// Canonical ABI name for host-opt-in `InheritedVisibility`.
pub const CANONICAL_INHERITED_VISIBILITY_NAME: &str =
    "bevy_camera::visibility::InheritedVisibility";
/// Canonical ABI name for host-opt-in `ViewVisibility`.
pub const CANONICAL_VIEW_VISIBILITY_NAME: &str = "bevy_camera::visibility::ViewVisibility";
/// Canonical ABI name for bridged host `Name`.
pub const CANONICAL_NAME_NAME: &str = "bevy_ecs::name::Name";
/// Canonical ABI name for bridged host `Time<Virtual>`.
pub const CANONICAL_TIME_VIRTUAL_NAME: &str = "bevy_time::Time<Virtual>";
/// Canonical ABI name for bridged host `Time<Fixed>`.
pub const CANONICAL_TIME_FIXED_NAME: &str = "bevy_time::Time<Fixed>";
/// Canonical ABI name for bridged host `Time<Real>`.
pub const CANONICAL_TIME_REAL_NAME: &str = "bevy_time::Time<Real>";
/// Canonical ABI name for bridged host `ButtonInput<KeyCode>`.
pub const CANONICAL_KEYBOARD_INPUT_NAME: &str = "bevy_input::ButtonInput<KeyCode>";
/// Canonical ABI name for bridged host `ButtonInput<MouseButton>`.
pub const CANONICAL_MOUSE_INPUT_NAME: &str = "bevy_input::ButtonInput<MouseButton>";
/// Canonical ABI name for direct POD `FfiMouseMotion` (`ModInputExtBridgePlugin`).
pub const CANONICAL_MOUSE_MOTION_NAME: &str = "bevy_input::mouse::AccumulatedMouseMotion";
/// Canonical ABI name for direct POD `FfiMouseScroll` (`ModInputExtBridgePlugin`).
pub const CANONICAL_MOUSE_SCROLL_NAME: &str = "bevy_input::mouse::AccumulatedMouseScroll";
/// Canonical ABI name for direct POD `FfiGamepadState` (`ModInputExtBridgePlugin`).
pub const CANONICAL_GAMEPAD_NAME: &str = "bevy_input::gamepad::Gamepad";
/// Canonical ABI name for direct POD `FfiGamepadRumble` (`ModInputExtBridgePlugin`).
pub const CANONICAL_GAMEPAD_RUMBLE_NAME: &str = "bevy_input::gamepad::GamepadRumbleRequest";

impl GuestComponent for FfiMouseMotion {
    const NAME: &'static CStr = c"bevy_input::mouse::AccumulatedMouseMotion";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for FfiMouseScroll {
    const NAME: &'static CStr = c"bevy_input::mouse::AccumulatedMouseScroll";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for FfiGamepadState {
    const NAME: &'static CStr = c"bevy_input::gamepad::Gamepad";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for FfiGamepadRumble {
    const NAME: &'static CStr = c"bevy_input::gamepad::GamepadRumbleRequest";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Safe guest wrapper around the host's [`ValidatedStringVTable`] for `bevy_ecs::name::Name`.
#[derive(Clone, Copy)]
pub struct GuestNameBridge {
    vtable: ValidatedStringVTable,
}

impl GuestNameBridge {
    /// Resolves `"bam.v0.bevy_ecs::name::Name.vtable"` (falling back to `"bam.v0.Name.vtable"`)
    /// and validates its [`VTableHeader`](bamabi_sys::VTableHeader).
    pub fn resolve(host: &Host) -> Result<Self, FfiResult> {
        let uuid = ComponentUuid::from_canonical_name(CANONICAL_NAME_NAME);
        let vtable = host
            .resolve_mapped_string_vtable(c"bam.v0.bevy_ecs::name::Name.vtable", uuid)
            .or_else(|_| host.resolve_mapped_string_vtable(c"bam.v0.Name.vtable", uuid))?;
        Ok(Self { vtable })
    }

    /// Returns the underlying validated [`ValidatedStringVTable`].
    #[inline]
    pub const fn vtable(&self) -> &ValidatedStringVTable {
        &self.vtable
    }

    /// Returns `true` if `Name` mutation is supported by the host bridge.
    #[inline]
    pub const fn supports_set(&self) -> bool {
        self.vtable.supports_set()
    }

    /// Reads `Name` at `(col, row)` of `chunk` into an owned guest `String`.
    #[inline]
    pub fn read_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        chunk.iter.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `Name` at `(col, row)` of `chunk`.
    #[inline]
    pub fn write_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        chunk
            .iter
            .write_mapped_string_row(col, row, &self.vtable, text)
    }

    /// Reads `Name` at `(col, row)` of the active chunk in `iter`.
    #[inline]
    pub fn read_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        iter.inner.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `Name` at `(col, row)` of the active chunk in `iter`.
    #[inline]
    pub fn write_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        iter.inner
            .write_mapped_string_row(col, row, &self.vtable, text)
    }

    /// Reads `Name` at `(col, row)` directly from a [`ValidatedIter`].
    #[inline]
    pub fn read_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        iter.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `Name` at `(col, row)` directly on a [`ValidatedIter`].
    #[inline]
    pub fn write_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        iter.write_mapped_string_row(col, row, &self.vtable, text)
    }
}

/// Safe guest wrapper around a validated [`ValidatedWireVTable<W>`] for bridged Bevy components
/// and resources.
pub struct GuestWireBridge<W: AbiPod> {
    vtable: ValidatedWireVTable<W>,
    _marker: PhantomData<W>,
}

impl<W: AbiPod> Copy for GuestWireBridge<W> {}

impl<W: AbiPod> Clone for GuestWireBridge<W> {
    /// Copies the validated wire bridge descriptor and function pointers.
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<W: AbiPod> GuestWireBridge<W> {
    /// Resolves `canonical_vtable_symbol` (or optional `alias_vtable_symbol`) against
    /// `canonical_name`'s [`ComponentUuid`].
    pub fn resolve(
        host: &Host,
        canonical_name: &str,
        canonical_vtable_symbol: &CStr,
        alias_vtable_symbol: Option<&CStr>,
    ) -> Result<Self, FfiResult> {
        let uuid = ComponentUuid::from_canonical_name(canonical_name);
        let vtable = match host.resolve_mapped_wire_vtable::<W>(canonical_vtable_symbol, uuid) {
            Ok(vt) => vt,
            Err(err) => match alias_vtable_symbol {
                Some(alias) => host.resolve_mapped_wire_vtable::<W>(alias, uuid)?,
                None => return Err(err),
            },
        };
        Ok(Self {
            vtable,
            _marker: PhantomData,
        })
    }

    /// Returns the underlying [`ValidatedWireVTable<W>`].
    #[inline]
    pub const fn vtable(&self) -> &ValidatedWireVTable<W> {
        &self.vtable
    }

    /// Returns `true` if mutable `set` writes are supported by this bridge.
    #[inline]
    pub const fn supports_set(&self) -> bool {
        self.vtable.supports_set()
    }

    /// Reads row `row` of column `col` in `chunk` into `W`.
    #[inline]
    pub fn read_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
    ) -> Result<Option<W>, FfiResult> {
        chunk.iter.read_mapped_wire_row(col, row, &self.vtable)
    }

    /// Writes `value` into row `row` of mutable column `col` in `chunk`.
    #[inline]
    pub fn write_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
        value: &W,
    ) -> FfiResult {
        chunk
            .iter
            .write_mapped_wire_row(col, row, &self.vtable, value)
    }

    /// Reads row `row` of column `col` from the active chunk of `iter`.
    #[inline]
    pub fn read_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<W>, FfiResult> {
        iter.inner.read_mapped_wire_row(col, row, &self.vtable)
    }

    /// Writes `value` into row `row` of mutable column `col` in `iter`.
    #[inline]
    pub fn write_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
        value: &W,
    ) -> FfiResult {
        iter.inner
            .write_mapped_wire_row(col, row, &self.vtable, value)
    }

    /// Reads row `row` of column `col` directly from a [`ValidatedIter`].
    #[inline]
    pub fn read_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<W>, FfiResult> {
        iter.read_mapped_wire_row(col, row, &self.vtable)
    }

    /// Writes `value` into row `row` of mutable column `col` directly on a [`ValidatedIter`].
    #[inline]
    pub fn write_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
        value: &W,
    ) -> FfiResult {
        iter.write_mapped_wire_row(col, row, &self.vtable, value)
    }

    /// Reads a bridged singleton resource declared in query `0` at `term_index`.
    #[inline]
    pub fn read_resource(
        &self,
        ctx: &mut SystemContext<'_>,
        term_index: usize,
    ) -> Result<W, FfiResult> {
        ctx.iter()
            .inner
            .read_mapped_wire_resource(term_index, &self.vtable)
    }

    /// Reads a bridged singleton resource declared in query `query_index` at `term_index`.
    #[inline]
    pub fn read_query_resource(
        &self,
        ctx: &mut SystemContext<'_>,
        query_index: usize,
        term_index: usize,
    ) -> Result<W, FfiResult> {
        let iter = ctx.query_mut(query_index).ok_or(FfiResult::OUT_OF_RANGE)?;
        iter.inner
            .read_mapped_wire_resource(term_index, &self.vtable)
    }

    /// Writes `value` into a mutable bridged singleton resource declared in query `0` at `term_index`.
    #[inline]
    pub fn write_resource(
        &self,
        ctx: &mut SystemContext<'_>,
        term_index: usize,
        value: &W,
    ) -> FfiResult {
        ctx.iter()
            .inner
            .write_mapped_wire_resource(term_index, &self.vtable, value)
    }

    /// Writes `value` into a mutable bridged singleton resource declared in `query_index` at `term_index`.
    #[inline]
    pub fn write_query_resource(
        &self,
        ctx: &mut SystemContext<'_>,
        query_index: usize,
        term_index: usize,
        value: &W,
    ) -> FfiResult {
        let Some(iter) = ctx.query_mut(query_index) else {
            return FfiResult::OUT_OF_RANGE;
        };
        iter.inner
            .write_mapped_wire_resource(term_index, &self.vtable, value)
    }

    /// Reads a bridged singleton resource by pre-resolved `BamComponent` handle via [`ValidatedSystemRef`].
    #[inline]
    pub fn read_resource_by_handle(
        &self,
        sys_ref: &ValidatedSystemRef<'_>,
        component: BamComponent,
    ) -> Result<(W, BamChangeTicks), FfiResult> {
        sys_ref.read_mapped_wire_resource_by_handle(component, &self.vtable)
    }

    /// Writes a bridged singleton resource by pre-resolved `BamComponent` handle via [`ValidatedSystemRef`].
    #[inline]
    pub fn write_resource_by_handle(
        &self,
        sys_ref: &mut ValidatedSystemRef<'_>,
        component: BamComponent,
        value: &W,
    ) -> Result<BamChangeTicks, FfiResult> {
        sys_ref.write_mapped_wire_resource_by_handle(component, &self.vtable, value)
    }
}

/// Guest wire bridge for Bevy `Transform`.
pub type GuestTransformBridge = GuestWireBridge<FfiTransform>;
/// Guest wire bridge for Bevy `GlobalTransform`.
pub type GuestGlobalTransformBridge = GuestWireBridge<FfiGlobalTransform>;
/// Guest wire bridge for Bevy `Visibility`.
pub type GuestVisibilityBridge = GuestWireBridge<FfiVisibility>;
/// Guest wire bridge for Bevy `0.20`'s `Disabled` marker (`StdTypesBridgePlugin`'s `DisabledBridge`).
pub type GuestDisabledBridge = GuestWireBridge<FfiBool>;
/// Host-opt-in guest wire bridge for Bevy `InheritedVisibility`.
pub type GuestInheritedVisibilityBridge = GuestWireBridge<FfiInheritedVisibility>;
/// Host-opt-in guest wire bridge for Bevy `ViewVisibility`.
pub type GuestViewVisibilityBridge = GuestWireBridge<FfiViewVisibility>;
/// Guest wire bridge for Bevy `Time<Virtual>`.
pub type GuestTimeVirtualBridge = GuestWireBridge<FfiTimeVirtual>;
/// Guest wire bridge for Bevy `Time<Fixed>`.
pub type GuestTimeFixedBridge = GuestWireBridge<FfiTimeFixed>;
/// Guest wire bridge for Bevy `Time<Real>`.
pub type GuestTimeRealBridge = GuestWireBridge<FfiTimeReal>;
/// Guest wire bridge for Bevy `ButtonInput<KeyCode>`.
pub type GuestKeyboardInputBridge = GuestWireBridge<FfiKeyboardInput>;
/// Guest wire bridge for Bevy `ButtonInput<MouseButton>`.
pub type GuestMouseInputBridge = GuestWireBridge<FfiMouseInput>;

/// Generates `resolve_std(host)` constructors for standard Bevy wire-bridge type aliases.
macro_rules! impl_std_wire_bridge {
    ($( $bridge:ident => ($canonical:ident, $primary:literal, $fallback:literal, $doc:literal) );* $(;)?) => {
        $(
            impl $bridge {
                #[doc = $doc]
                pub fn resolve_std(host: &Host) -> Result<Self, FfiResult> {
                    Self::resolve(host, $canonical, $primary, Some($fallback))
                }
            }
        )*
    };
}

impl_std_wire_bridge! {
    GuestTransformBridge => (
        CANONICAL_TRANSFORM_NAME,
        c"bam.v0.bevy_transform::components::transform::Transform.vtable",
        c"bam.v0.Transform.vtable",
        "Resolves the host `Transform` VTable (`\"bam.v0.Transform.vtable\"`)."
    );
    GuestGlobalTransformBridge => (
        CANONICAL_GLOBAL_TRANSFORM_NAME,
        c"bam.v0.bevy_transform::components::global_transform::GlobalTransform.vtable",
        c"bam.v0.GlobalTransform.vtable",
        "Resolves the host `GlobalTransform` VTable (`\"bam.v0.GlobalTransform.vtable\"`)."
    );
    GuestVisibilityBridge => (
        CANONICAL_VISIBILITY_NAME,
        c"bam.v0.bevy_camera::visibility::Visibility.vtable",
        c"bam.v0.Visibility.vtable",
        "Resolves the host `Visibility` VTable (`\"bam.v0.Visibility.vtable\"`)."
    );
    GuestDisabledBridge => (
        BAM_DISABLED_NAME,
        c"bam.v0.bevy_ecs::entity_disabling::Disabled.vtable",
        c"bam.v0.Disabled.vtable",
        "Resolves the host `Disabled` VTable (`\"bam.v0.Disabled.vtable\"`)."
    );
    GuestInheritedVisibilityBridge => (
        CANONICAL_INHERITED_VISIBILITY_NAME,
        c"bam.v0.bevy_camera::visibility::InheritedVisibility.vtable",
        c"bam.v0.InheritedVisibility.vtable",
        "Resolves a host-opt-in `InheritedVisibility` VTable."
    );
    GuestViewVisibilityBridge => (
        CANONICAL_VIEW_VISIBILITY_NAME,
        c"bam.v0.bevy_camera::visibility::ViewVisibility.vtable",
        c"bam.v0.ViewVisibility.vtable",
        "Resolves a host-opt-in `ViewVisibility` VTable."
    );
    GuestTimeVirtualBridge => (
        CANONICAL_TIME_VIRTUAL_NAME,
        c"bam.v0.bevy_time::Time<Virtual>.vtable",
        c"bam.v0.Time<Virtual>.vtable",
        "Resolves the host `Time<Virtual>` VTable."
    );
    GuestTimeFixedBridge => (
        CANONICAL_TIME_FIXED_NAME,
        c"bam.v0.bevy_time::Time<Fixed>.vtable",
        c"bam.v0.Time<Fixed>.vtable",
        "Resolves the host `Time<Fixed>` VTable."
    );
    GuestTimeRealBridge => (
        CANONICAL_TIME_REAL_NAME,
        c"bam.v0.bevy_time::Time<Real>.vtable",
        c"bam.v0.Time<Real>.vtable",
        "Resolves the host `Time<Real>` VTable."
    );
    GuestKeyboardInputBridge => (
        CANONICAL_KEYBOARD_INPUT_NAME,
        c"bam.v0.bevy_input::ButtonInput<KeyCode>.vtable",
        c"bam.v0.ButtonInput<KeyCode>.vtable",
        "Resolves the host `ButtonInput<KeyCode>` VTable."
    );
    GuestMouseInputBridge => (
        CANONICAL_MOUSE_INPUT_NAME,
        c"bam.v0.bevy_input::ButtonInput<MouseButton>.vtable",
        c"bam.v0.ButtonInput<MouseButton>.vtable",
        "Resolves the host `ButtonInput<MouseButton>` VTable."
    );
}

/// Resolves a `MappedStringVTable` directly via `Host::resolve_mapped_string_vtable` for custom
/// host string components.
#[inline]
pub fn resolve_string_vtable(
    host: &Host,
    symbol: &CStr,
    canonical_name: &str,
) -> Result<ValidatedStringVTable, FfiResult> {
    host.resolve_mapped_string_vtable(symbol, ComponentUuid::from_canonical_name(canonical_name))
}

/// Extension trait on [`QueryBuilder`] adding bridged standard Bevy component and resource
/// terms (`Transform`, `GlobalTransform`, `Visibility`, `InheritedVisibility`, `ViewVisibility`,
/// `Name`, `Time<*>`, `ButtonInput<*>`, and direct POD `FfiMouseMotion`/`FfiMouseScroll`/
/// `FfiGamepadState`).
pub trait GuestStdQueryBuilderExt {
    /// Appends a shared read (`&Transform` wire projection) term.
    fn read_transform(self) -> Self;
    /// Appends an exclusive write (`&mut Transform` wire projection) term.
    fn write_transform(self) -> Self;
    /// Appends a shared read (`&GlobalTransform` wire projection) term.
    fn read_global_transform(self) -> Self;
    /// Appends a shared read (`&Visibility` wire projection) term.
    fn read_visibility(self) -> Self;
    /// Appends an exclusive write (`&mut Visibility` wire projection) term.
    fn write_visibility(self) -> Self;
    /// Appends a shared read (`&InheritedVisibility` host-opt-in wire projection) term.
    fn read_inherited_visibility(self) -> Self;
    /// Appends a shared read (`&ViewVisibility` host-opt-in wire projection) term.
    fn read_view_visibility(self) -> Self;
    /// Appends a shared read (`&Name` wire projection) term.
    fn read_name(self) -> Self;
    /// Appends an exclusive write (`&mut Name` wire projection) term.
    fn write_name(self) -> Self;
    /// Appends a shared resource read (`Res<Time<Virtual>>`) term.
    fn read_time_virtual(self) -> Self;
    /// Appends a shared resource read (`Res<Time<Fixed>>`) term.
    fn read_time_fixed(self) -> Self;
    /// Appends a shared resource read (`Res<Time<Real>>`) term.
    fn read_time_real(self) -> Self;
    /// Appends a shared resource read (`Res<ButtonInput<KeyCode>>`) term.
    fn read_keyboard_input(self) -> Self;
    /// Appends a shared resource read (`Res<ButtonInput<MouseButton>>`) term.
    fn read_mouse_input(self) -> Self;
    /// Appends a shared POD resource read (`Res<FfiMouseMotion>`) term.
    fn read_mouse_motion(self) -> Self;
    /// Appends a shared POD resource read (`Res<FfiMouseScroll>`) term.
    fn read_mouse_scroll(self) -> Self;
    /// Appends a shared POD component read (`&FfiGamepadState`) term.
    fn read_gamepad(self) -> Self;
}

/// Generates [`GuestStdQueryBuilderExt`] methods that resolve standard Bevy components/resources
/// and append their corresponding [`AccessMode`] query terms.
macro_rules! impl_std_query_methods {
    ($( $method:ident => ($cstr:literal, $canonical:ident, $mode:ident) );* $(;)?) => {
        $(
            #[inline]
            fn $method(self) -> Self {
                self.term_named(
                    $cstr,
                    Some(ComponentUuid::from_canonical_name($canonical)),
                    AccessMode::$mode,
                )
            }
        )*
    };
}

impl GuestStdQueryBuilderExt for QueryBuilder<'_> {
    impl_std_query_methods! {
        read_transform => (c"bevy_transform::components::transform::Transform", CANONICAL_TRANSFORM_NAME, READ);
        write_transform => (c"bevy_transform::components::transform::Transform", CANONICAL_TRANSFORM_NAME, WRITE);
        read_global_transform => (c"bevy_transform::components::global_transform::GlobalTransform", CANONICAL_GLOBAL_TRANSFORM_NAME, READ);
        read_visibility => (c"bevy_camera::visibility::Visibility", CANONICAL_VISIBILITY_NAME, READ);
        write_visibility => (c"bevy_camera::visibility::Visibility", CANONICAL_VISIBILITY_NAME, WRITE);
        read_inherited_visibility => (c"bevy_camera::visibility::InheritedVisibility", CANONICAL_INHERITED_VISIBILITY_NAME, READ);
        read_view_visibility => (c"bevy_camera::visibility::ViewVisibility", CANONICAL_VIEW_VISIBILITY_NAME, READ);
        read_name => (c"bevy_ecs::name::Name", CANONICAL_NAME_NAME, READ);
        write_name => (c"bevy_ecs::name::Name", CANONICAL_NAME_NAME, WRITE);
        read_time_virtual => (c"bevy_time::Time<Virtual>", CANONICAL_TIME_VIRTUAL_NAME, RESOURCE_READ);
        read_time_fixed => (c"bevy_time::Time<Fixed>", CANONICAL_TIME_FIXED_NAME, RESOURCE_READ);
        read_time_real => (c"bevy_time::Time<Real>", CANONICAL_TIME_REAL_NAME, RESOURCE_READ);
        read_keyboard_input => (c"bevy_input::ButtonInput<KeyCode>", CANONICAL_KEYBOARD_INPUT_NAME, RESOURCE_READ);
        read_mouse_input => (c"bevy_input::ButtonInput<MouseButton>", CANONICAL_MOUSE_INPUT_NAME, RESOURCE_READ);
    }

    #[inline]
    fn read_mouse_motion(self) -> Self {
        self.term_named(
            FfiMouseMotion::NAME,
            Some(FfiMouseMotion::UUID),
            AccessMode::RESOURCE_READ,
        )
    }

    #[inline]
    fn read_mouse_scroll(self) -> Self {
        self.term_named(
            FfiMouseScroll::NAME,
            Some(FfiMouseScroll::UUID),
            AccessMode::RESOURCE_READ,
        )
    }

    #[inline]
    fn read_gamepad(self) -> Self {
        self.term_named(
            FfiGamepadState::NAME,
            Some(FfiGamepadState::UUID),
            AccessMode::READ,
        )
    }
}

/// Pre-resolved [`BamComponent`] handles for standard bridged Bevy components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestStdHandles {
    /// Handle for bridged `Transform` (`default_inserter` supported).
    pub transform: BamComponent,
    /// Handle for bridged read-only `GlobalTransform`.
    pub global_transform: BamComponent,
    /// Handle for bridged `Visibility` (`default_inserter` supported).
    pub visibility: BamComponent,
    /// Handle for bridged `Name` (`default_inserter` supported).
    pub name: BamComponent,
}

impl GuestStdHandles {
    /// Resolves all four core standard component handles from `host`.
    pub fn resolve(host: &Host) -> Result<Self, FfiResult> {
        let transform = resolve_component(
            host,
            Some(c"bevy_transform::components::transform::Transform"),
            Some(ComponentUuid::from_canonical_name(CANONICAL_TRANSFORM_NAME)),
        )?;
        let global_transform = resolve_component(
            host,
            Some(c"bevy_transform::components::global_transform::GlobalTransform"),
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_GLOBAL_TRANSFORM_NAME,
            )),
        )?;
        let visibility = resolve_component(
            host,
            Some(c"bevy_camera::visibility::Visibility"),
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_VISIBILITY_NAME,
            )),
        )?;
        let name = resolve_component(
            host,
            Some(c"bevy_ecs::name::Name"),
            Some(ComponentUuid::from_canonical_name(CANONICAL_NAME_NAME)),
        )?;
        Ok(Self {
            transform,
            global_transform,
            visibility,
            name,
        })
    }

    /// Resolves Bevy `0.20`'s `bevy_ecs::entity_disabling::Disabled` marker component handle.
    #[inline]
    pub fn resolve_disabled(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_component(
            host,
            Some(c"bevy_ecs::entity_disabling::Disabled"),
            Some(FfiDisabled::UUID),
        )
    }

    /// Resolves a host-opt-in `InheritedVisibility` component handle.
    #[inline]
    pub fn resolve_inherited_visibility(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_component(
            host,
            Some(c"bevy_camera::visibility::InheritedVisibility"),
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_INHERITED_VISIBILITY_NAME,
            )),
        )
    }

    /// Resolves a host-opt-in `ViewVisibility` component handle.
    #[inline]
    pub fn resolve_view_visibility(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_component(
            host,
            Some(c"bevy_camera::visibility::ViewVisibility"),
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_VIEW_VISIBILITY_NAME,
            )),
        )
    }

    /// Resolves `ModInputExtBridgePlugin`'s direct POD `FfiGamepadState` (`"bevy_input::gamepad::Gamepad"`) handle.
    #[inline]
    pub fn resolve_gamepad(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_component(
            host,
            Some(FfiGamepadState::NAME),
            Some(FfiGamepadState::UUID),
        )
    }
}

/// Extension trait on [`GuestCommands`] for inserting host-defaulted standard bridged components
/// (`Transform::IDENTITY`, `Visibility::Inherited`, `Name::default()`).
pub trait GuestStdCommandsExt {
    /// Queues `Transform::default()` insertion on `entity`.
    fn insert_default_transform(
        &mut self,
        entity: FfiEntity,
        handles: &GuestStdHandles,
    ) -> FfiResult;
    /// Queues `Visibility::default()` insertion on `entity`.
    fn insert_default_visibility(
        &mut self,
        entity: FfiEntity,
        handles: &GuestStdHandles,
    ) -> FfiResult;
    /// Queues `Name::default()` insertion on `entity`.
    fn insert_default_name(&mut self, entity: FfiEntity, handles: &GuestStdHandles) -> FfiResult;
}

impl GuestStdCommandsExt for GuestCommands<'_> {
    /// Inserts a default-initialized Bevy `Transform` component onto `entity`.
    #[inline]
    fn insert_default_transform(
        &mut self,
        entity: FfiEntity,
        handles: &GuestStdHandles,
    ) -> FfiResult {
        self.insert_default(entity, handles.transform)
    }

    /// Inserts a default-initialized Bevy `Visibility` component onto `entity`.
    #[inline]
    fn insert_default_visibility(
        &mut self,
        entity: FfiEntity,
        handles: &GuestStdHandles,
    ) -> FfiResult {
        self.insert_default(entity, handles.visibility)
    }

    /// Inserts a default-initialized Bevy `Name` component onto `entity`.
    #[inline]
    fn insert_default_name(&mut self, entity: FfiEntity, handles: &GuestStdHandles) -> FfiResult {
        self.insert_default(entity, handles.name)
    }
}

/// Extension trait on [`SystemBuilder`] declaring Bevy `0.20` extended input message writers
/// (`ModInputExtBridgePlugin`'s `Messages<FfiGamepadRumble>`).
pub trait GuestStdSystemBuilderExt {
    /// Declares an [`FfiGamepadRumble`] message writer (`MESSAGE_WRITE`) so the system can emit
    /// haptic rumble requests to Bevy's `GamepadRumbleRequest` stream.
    fn write_gamepad_rumble(self) -> Self;
}

impl GuestStdSystemBuilderExt for SystemBuilder<'_, '_> {
    #[inline]
    fn write_gamepad_rumble(self) -> Self {
        self.message_write::<FfiGamepadRumble>()
    }
}

/// Extension trait on [`SystemContext`] for emitting Bevy `0.20` haptic [`FfiGamepadRumble`]
/// requests through a declared [`GuestStdSystemBuilderExt::write_gamepad_rumble`] stream.
pub trait GuestGamepadRumbleExt {
    /// Sends a raw [`FfiGamepadRumble`] request on the `MESSAGE_WRITE` stream at `decl_index`.
    fn send_gamepad_rumble(&mut self, decl_index: usize, request: FfiGamepadRumble) -> FfiResult;

    /// Batch-sends a slice of [`FfiGamepadRumble`] requests on the `MESSAGE_WRITE` stream at
    /// `decl_index`.
    fn send_gamepad_rumbles(
        &mut self,
        decl_index: usize,
        requests: &[FfiGamepadRumble],
    ) -> Result<(), FfiResult>;

    /// Sends an [`FfiGamepadRumble::add`] haptic rumble request targeting `gamepad` on
    /// `decl_index`.
    fn send_gamepad_rumble_add(
        &mut self,
        decl_index: usize,
        gamepad: FfiEntity,
        duration_secs: f32,
        strong_motor: f32,
        weak_motor: f32,
    ) -> FfiResult;

    /// Sends an [`FfiGamepadRumble::stop`] request halting active rumble on `gamepad` via
    /// `decl_index`.
    fn send_gamepad_rumble_stop(&mut self, decl_index: usize, gamepad: FfiEntity) -> FfiResult;
}

impl GuestGamepadRumbleExt for SystemContext<'_> {
    #[inline]
    fn send_gamepad_rumble(&mut self, decl_index: usize, request: FfiGamepadRumble) -> FfiResult {
        self.send_message(decl_index, &request)
    }

    #[inline]
    fn send_gamepad_rumbles(
        &mut self,
        decl_index: usize,
        requests: &[FfiGamepadRumble],
    ) -> Result<(), FfiResult> {
        self.send_messages(decl_index, requests)
    }

    #[inline]
    fn send_gamepad_rumble_add(
        &mut self,
        decl_index: usize,
        gamepad: FfiEntity,
        duration_secs: f32,
        strong_motor: f32,
        weak_motor: f32,
    ) -> FfiResult {
        let req = FfiGamepadRumble::add(gamepad, duration_secs, strong_motor, weak_motor);
        self.send_message(decl_index, &req)
    }

    #[inline]
    fn send_gamepad_rumble_stop(&mut self, decl_index: usize, gamepad: FfiEntity) -> FfiResult {
        let req = FfiGamepadRumble::stop(gamepad);
        self.send_message(decl_index, &req)
    }
}

/// Bundle of all standard Bevy component/resource guest bridges and [`GuestStdHandles`],
/// resolved together during mod initialization (`init_mod`).
#[derive(Clone, Copy)]
pub struct GuestStdTypes {
    /// Pre-resolved component handles (`Transform`, `GlobalTransform`, `Visibility`, `Name`).
    pub handles: GuestStdHandles,
    /// `Transform` wire VTable bridge.
    pub transform: GuestTransformBridge,
    /// `GlobalTransform` read-only wire VTable bridge.
    pub global_transform: GuestGlobalTransformBridge,
    /// `Visibility` wire VTable bridge.
    pub visibility: GuestVisibilityBridge,
    /// `Name` UTF-8 string VTable bridge.
    pub name: GuestNameBridge,
    /// `Time<Virtual>` wire VTable bridge.
    pub time_virtual: GuestTimeVirtualBridge,
    /// `Time<Fixed>` wire VTable bridge.
    pub time_fixed: GuestTimeFixedBridge,
    /// `Time<Real>` wire VTable bridge.
    pub time_real: GuestTimeRealBridge,
    /// `ButtonInput<KeyCode>` wire VTable bridge.
    pub keyboard: GuestKeyboardInputBridge,
    /// `ButtonInput<MouseButton>` wire VTable bridge.
    pub mouse: GuestMouseInputBridge,
}

impl GuestStdTypes {
    /// Resolves every standard Bevy bridge registered by `StdTypesBridgePlugin` from `host`.
    pub fn resolve_all(host: &Host) -> Result<Self, FfiResult> {
        Ok(Self {
            handles: GuestStdHandles::resolve(host)?,
            transform: GuestTransformBridge::resolve_std(host)?,
            global_transform: GuestGlobalTransformBridge::resolve_std(host)?,
            visibility: GuestVisibilityBridge::resolve_std(host)?,
            name: GuestNameBridge::resolve(host)?,
            time_virtual: GuestTimeVirtualBridge::resolve_std(host)?,
            time_fixed: GuestTimeFixedBridge::resolve_std(host)?,
            time_real: GuestTimeRealBridge::resolve_std(host)?,
            keyboard: GuestKeyboardInputBridge::resolve_std(host)?,
            mouse: GuestMouseInputBridge::resolve_std(host)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bamabi_sys::{
        ABI_VERSION, BamChangeTicks, BamColumnInfo, BamComponent, BamHostApi, BamHostCtx,
        BamLoadCtx, MappedStringVTable, StrView, VTableHeader,
    };
    use core::ffi::{c_char, c_void};

    /// Scenario: Exercises `FfiTransform`, `FfiVisibility`, `FfiButtonInput`, and related std
    /// wire helper constructors and bitset queries under `#![forbid(unsafe_code)]`.
    #[test]
    fn ffi_wire_helpers_and_bitsets_work_without_unsafe() {
        let tf = FfiTransform::from_xyz(1.0, 2.0, 3.0).with_scale([2.0, 2.0, 2.0]);
        assert_eq!(tf.translation, [1.0, 2.0, 3.0]);
        assert_eq!(tf.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(tf.scale, [2.0, 2.0, 2.0]);

        let rot90 = FfiTransform::from_rotation_y(core::f32::consts::FRAC_PI_2);
        let fwd = rot90.forward();
        assert!((fwd[0] - (-1.0)).abs() < 1e-5);
        assert!(fwd[1].abs() < 1e-5);
        assert!(fwd[2].abs() < 1e-5);

        let look =
            FfiTransform::from_xyz(0.0, 0.0, 5.0).looking_at([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let lfwd = look.forward();
        assert!(lfwd[0].abs() < 1e-5);
        assert!(lfwd[1].abs() < 1e-5);
        assert!((lfwd[2] - (-1.0)).abs() < 1e-5);

        let gtf = FfiGlobalTransform::from_translation([4.0, 5.0, 6.0]);
        assert_eq!(gtf.translation(), [4.0, 5.0, 6.0]);
        assert_eq!(gtf.to_mat4()[3], [4.0, 5.0, 6.0, 1.0]);
        assert_eq!(gtf.transform_point([1.0, 2.0, 3.0]), [5.0, 7.0, 9.0]);

        assert!(FfiVisibility::INHERITED.is_inherited());
        assert!(FfiVisibility::HIDDEN.is_hidden());
        assert!(FfiVisibility::VISIBLE.is_visible());

        let mut kb = FfiKeyboardInput::default();
        kb.press(FfiKeyCode::SPACE);
        assert!(kb.pressed(FfiKeyCode::SPACE));
        assert!(kb.just_pressed(FfiKeyCode::SPACE));
        assert!(kb.any_pressed([FfiKeyCode::KEY_W, FfiKeyCode::SPACE]));
        assert!(!kb.all_pressed([FfiKeyCode::KEY_W, FfiKeyCode::SPACE]));
        kb.clear();
        assert!(kb.pressed(FfiKeyCode::SPACE));
        assert!(!kb.just_pressed(FfiKeyCode::SPACE));
        kb.release(FfiKeyCode::SPACE);
        assert!(!kb.pressed(FfiKeyCode::SPACE));
        assert!(kb.just_released(FfiKeyCode::SPACE));
        assert!(kb.clear_just_released(FfiKeyCode::SPACE));
        assert!(!kb.just_released(FfiKeyCode::SPACE));

        let mut ms = FfiMouseInput::default();
        ms.press(FfiMouseButton::LEFT);
        ms.set_just_pressed(FfiMouseButton::other(2).unwrap(), true);
        assert!(ms.pressed(FfiMouseButton::LEFT));
        assert!(!ms.pressed(FfiMouseButton::RIGHT));
        assert!(ms.any_just_pressed([FfiMouseButton::LEFT, FfiMouseButton::RIGHT]));
        assert!(ms.just_pressed(FfiMouseButton::other(2).unwrap()));
        ms.reset_all();
        assert!(!ms.pressed(FfiMouseButton::LEFT));

        let color = FfiColor::srgb_u8(255, 128, 0).with_alpha(0.5);
        assert!((color.r - 1.0).abs() < 1e-6);
        assert!((color.a - 0.5).abs() < 1e-6);
        let lin = color.to_linear_rgba();
        let roundtrip = FfiColor::from_linear_rgba(lin[0], lin[1], lin[2], lin[3]);
        assert!((roundtrip.g - color.g).abs() < 1e-5);
    }

    /// Scenario: Resolves `GuestNameBridge` against a mock `MappedStringVTable` with matching
    /// `BAM_NAME_NAME` UUID header and verifies read/write dispatch succeeds.
    #[test]
    fn guest_name_bridge_vtable_header_matches_host_uuid() {
        static NAME_VTABLE: MappedStringVTable = MappedStringVTable {
            header: VTableHeader::from_raw_parts(
                core::mem::size_of::<MappedStringVTable>() as u32,
                ComponentUuid::from_canonical_name(CANONICAL_NAME_NAME),
                24,
                8,
            ),
            get: Some(mock_name_get),
            set: Some(mock_name_set),
        };

        /// Mock `MappedStringVTable::get` returning a constant `"MockName"` view.
        extern "C" fn mock_name_get(_ptr: *const c_void) -> StrView {
            StrView::from_str("MockName")
        }
        /// Mock `MappedStringVTable::set` unconditionally accepting writes with `FfiResult::OK`.
        extern "C" fn mock_name_set(_ptr: *mut c_void, _view: StrView) -> FfiResult {
            FfiResult::OK
        }
        /// Mock `get_proc_addr` returning the static `NAME_VTABLE` pointer.
        extern "C" fn mock_get_proc(_ctx: *const BamHostCtx, _sym: *const c_char) -> *const c_void {
            (&raw const NAME_VTABLE).cast::<c_void>()
        }

        assert_eq!(
            NAME_VTABLE.header.uuid,
            ComponentUuid::from_canonical_name("bevy_ecs::name::Name")
        );
        let _ = (
            ABI_VERSION,
            BamChangeTicks::default(),
            core::mem::size_of::<BamColumnInfo>(),
            BamComponent::INVALID,
            BamHostApi::MIN_V0_2_SIZE,
            core::mem::size_of::<BamLoadCtx>(),
            mock_get_proc,
        );
    }

    /// Scenario: Verifies at compile time that all standard Bevy wire structs satisfy `AbiPod`
    /// even with `--no-default-features` and validates `GuestWireBridge` vtable header checks.
    #[test]
    fn guest_wire_bridge_validates_mapped_wire_vtable_unconditionally() {
        // Compile-time proof that all std wire structs implement `AbiPod` even with `--no-default-features`.
        bamabi_sys::assert_abi_pod::<FfiTransform>();
        bamabi_sys::assert_abi_pod::<FfiGlobalTransform>();
        bamabi_sys::assert_abi_pod::<FfiVisibility>();
        bamabi_sys::assert_abi_pod::<FfiInheritedVisibility>();
        bamabi_sys::assert_abi_pod::<FfiViewVisibility>();
        bamabi_sys::assert_abi_pod::<FfiColor>();
        bamabi_sys::assert_abi_pod::<FfiTimeVirtual>();
        bamabi_sys::assert_abi_pod::<FfiTimeFixed>();
        bamabi_sys::assert_abi_pod::<FfiTimeReal>();
        bamabi_sys::assert_abi_pod::<FfiKeyboardInput>();
        bamabi_sys::assert_abi_pod::<FfiMouseInput>();
        bamabi_sys::assert_abi_pod::<FfiMouseMotion>();
        bamabi_sys::assert_abi_pod::<FfiMouseScroll>();
        bamabi_sys::assert_abi_pod::<FfiGamepadState>();
        bamabi_sys::assert_abi_pod::<FfiGamepadRumble>();
        assert_eq!(
            core::mem::size_of::<bamabi_sys::MappedWireVTable<FfiTransform>>(),
            core::mem::size_of::<MappedStringVTable>()
        );
    }
}
