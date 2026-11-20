//! Guest-side `#![forbid(unsafe_code)]`, zero-Bevy helpers for bridged Bevy `0.20` states
//! matching `bamabi_bridges::state` bit-for-bit:
//!
//! - Unbounded dynamic `ModStateSlab` (`ModStateHandle`, `ModStateEntry`,
//!   `ModStateTransitionRequest`, `ModDespawnOnExit`, `ModDespawnOnEnter`) supporting arbitrary
//!   `States`, `SubStates`, and read-only `ComputedStates` without fixed slot limits.
//! - Primary slot-0 legacy `GuestStateBridge` (`"bevy_state::State#0"` / `"bamabi::ModNextState#0"`),
//!   deterministic `OnEnter`/`OnExit`/`OnTransition` schedule IDs, `.run_if_in_state(...)`, and
//!   `DespawnOnExit`/`DespawnOnEnter` markers.

use bamabi_guest_sys::{Host, ValidatedSystemRef, ValidatedWireVTable, resolve_component};
use bamabi_sys::{
    AccessMode, BamChangeTicks, BamComponent, ComponentUuid, FfiEntity, FfiResult, ScheduleId,
};
use core::ffi::CStr;
use std::ffi::CString;

pub use bamabi_guest_sys::wire_types::{
    FfiStateTransitionEvent, MOD_STATE_FLAG_ACTIVE, MOD_STATE_FLAG_COMPUTED,
    MOD_STATE_FLAG_MUTABLE, MOD_STATE_FLAG_SUB_STATE, ModDespawnOnEnter, ModDespawnOnExit,
    ModStateEntry, ModStateHandle, ModStateTransitionRequest, STATE_ABSENT_DISCRIMINANT,
};

use crate::app::{QueryBuilder, SystemBuilder, SystemContext};
use crate::commands::GuestCommands;
use crate::component::GuestComponent;

/// Canonical ABI name for the dynamic read-only [`ModStateEntry`] snapshot component.
pub const CANONICAL_MOD_STATE_ENTRY_NAME: &str = "bevy_state::ModStateEntry";
/// Canonical ABI name for the dynamic [`ModStateTransitionRequest`] message stream.
pub const CANONICAL_MOD_STATE_TRANSITION_REQUEST_NAME: &str =
    "bevy_state::ModStateTransitionRequest";
/// Canonical ABI name for the dynamic [`ModDespawnOnExit`] component.
pub const CANONICAL_MOD_DESPAWN_ON_EXIT_NAME: &str = "bevy_state::ModDespawnOnExit";
/// Canonical ABI name for the dynamic [`ModDespawnOnEnter`] component.
pub const CANONICAL_MOD_DESPAWN_ON_ENTER_NAME: &str = "bevy_state::ModDespawnOnEnter";
/// Canonical ABI name for host-opt-in bridged Bevy `0.20` `StateTransitionEvent` messages.
pub const CANONICAL_STATE_TRANSITION_EVENT_NAME: &str = "bevy_state::state::StateTransitionEvent";

impl GuestComponent for ModStateEntry {
    const NAME: &'static CStr = c"bevy_state::ModStateEntry";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for ModStateTransitionRequest {
    const NAME: &'static CStr = c"bevy_state::ModStateTransitionRequest";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for ModDespawnOnExit {
    const NAME: &'static CStr = c"bevy_state::ModDespawnOnExit";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for ModDespawnOnEnter {
    const NAME: &'static CStr = c"bevy_state::ModDespawnOnEnter";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

impl GuestComponent for FfiStateTransitionEvent {
    const NAME: &'static CStr = c"bevy_state::state::StateTransitionEvent";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Builds the canonical host state-gating system-set name (`"host::state.<state_name>.<variant>"`)
/// for [`GuestStateSystemBuilderExt::run_if_in_state`].
pub fn state_set_name(state_name: &str, variant: &str) -> Result<CString, FfiResult> {
    if state_name.is_empty() || variant.is_empty() {
        return Err(FfiResult::INVALID_ARGUMENT);
    }
    CString::new(format!("host::state.{state_name}.{variant}"))
        .map_err(|_| FfiResult::INVALID_ARGUMENT)
}

/// Computes the deterministic `OnEnter(variant)` custom [`ScheduleId`] for `variant_index`
/// (`0..variant_count`) under `base`.
#[inline]
pub const fn on_enter_schedule(base: ScheduleId, variant_index: u32) -> ScheduleId {
    ScheduleId::custom(base.0 + variant_index)
}

/// Computes the deterministic `OnExit(variant)` custom [`ScheduleId`] for `variant_index`
/// (`0..variant_count`) under `base`.
#[inline]
pub const fn on_exit_schedule(
    base: ScheduleId,
    variant_count: u32,
    variant_index: u32,
) -> ScheduleId {
    ScheduleId::custom(base.0 + variant_count + variant_index)
}

/// Computes the deterministic `OnTransition { exited, entered }` custom [`ScheduleId`] under `base`.
#[inline]
pub const fn on_transition_schedule(
    base: ScheduleId,
    variant_count: u32,
    exited_index: u32,
    entered_index: u32,
) -> ScheduleId {
    ScheduleId::custom(base.0 + 2 * variant_count + exited_index * variant_count + entered_index)
}

/// Safe guest bridge for reading `State<S>` (`u32` discriminant, or [`STATE_ABSENT_DISCRIMINANT`]
/// when absent) and writing `ModNextState<S>` (`NextState::set_if_different`) on a host-exposed
/// primary Bevy state machine (`slot = 0`).
///
/// For unbounded multi-state, `SubStates`, and `ComputedStates` workflows backed by host
/// `ModStateSlab`, query [`ModStateEntry`] components directly and emit
/// [`ModStateTransitionRequest`] messages.
#[derive(Clone, Copy)]
pub struct GuestStateBridge {
    state_slot: usize,
    state_handle: BamComponent,
    next_state_handle: BamComponent,
    state_vtable: ValidatedWireVTable<u32>,
    next_state_vtable: ValidatedWireVTable<u32>,
}

impl GuestStateBridge {
    /// Resolves the primary exposed state (`slot = 0`, alias `"State"` / `"ModNextState"`).
    #[inline]
    pub fn resolve_primary(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve_slot(host, 0)
    }

    /// Resolves exposed state slot `slot` (`"bevy_state::State#<slot>"` and
    /// `"bamabi::ModNextState#<slot>"`).
    pub fn resolve_slot(host: &Host, slot: usize) -> Result<Self, FfiResult> {
        let state_canonical = format!("bevy_state::State#{slot}");
        let next_canonical = format!("bamabi::ModNextState#{slot}");
        let state_comp_cstr =
            CString::new(state_canonical.as_str()).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
        let next_comp_cstr =
            CString::new(next_canonical.as_str()).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
        let state_sym = CString::new(format!("bam.v0.{state_canonical}.vtable"))
            .map_err(|_| FfiResult::INVALID_ARGUMENT)?;
        let next_sym = CString::new(format!("bam.v0.{next_canonical}.vtable"))
            .map_err(|_| FfiResult::INVALID_ARGUMENT)?;

        let state_uuid = ComponentUuid::from_canonical_name(&state_canonical);
        let next_uuid = ComponentUuid::from_canonical_name(&next_canonical);

        let state_handle = resolve_component(host, Some(&state_comp_cstr), Some(state_uuid))?;
        let next_state_handle = resolve_component(host, Some(&next_comp_cstr), Some(next_uuid))?;

        let state_vtable = host
            .resolve_mapped_wire_vtable::<u32>(&state_sym, state_uuid)
            .or_else(|err| {
                if slot == 0 {
                    host.resolve_mapped_wire_vtable::<u32>(c"bam.v0.State.vtable", state_uuid)
                } else {
                    Err(err)
                }
            })?;
        let next_state_vtable = host
            .resolve_mapped_wire_vtable::<u32>(&next_sym, next_uuid)
            .or_else(|err| {
                if slot == 0 {
                    host.resolve_mapped_wire_vtable::<u32>(c"bam.v0.ModNextState.vtable", next_uuid)
                } else {
                    Err(err)
                }
            })?;

        Ok(Self {
            state_slot: slot,
            state_handle,
            next_state_handle,
            state_vtable,
            next_state_vtable,
        })
    }

    /// Returns the 0-based state slot index bound to this bridge.
    #[inline]
    pub const fn state_slot(&self) -> usize {
        self.state_slot
    }

    /// Returns the resolved [`BamComponent`] resource handle for `State<S>` in this slot.
    #[inline]
    pub const fn state_handle(&self) -> BamComponent {
        self.state_handle
    }

    /// Returns the resolved [`BamComponent`] resource handle for `ModNextState<S>` in this slot.
    #[inline]
    pub const fn next_state_handle(&self) -> BamComponent {
        self.next_state_handle
    }

    /// Reads the current `State<S>` raw `u32` discriminant declared in query `0` at `term_index`.
    #[inline]
    pub fn current(
        &self,
        ctx: &mut SystemContext<'_>,
        term_index: usize,
    ) -> Result<u32, FfiResult> {
        ctx.iter()
            .inner
            .read_mapped_wire_resource(term_index, &self.state_vtable)
    }

    /// Reads the current `State<S>` discriminant, mapping [`STATE_ABSENT_DISCRIMINANT`]
    /// (`u32::MAX`) or `FfiResult::NOT_FOUND` to `Ok(None)`.
    #[inline]
    pub fn current_optional(
        &self,
        ctx: &mut SystemContext<'_>,
        term_index: usize,
    ) -> Result<Option<u32>, FfiResult> {
        match self.current(ctx, term_index) {
            Ok(STATE_ABSENT_DISCRIMINANT) | Err(FfiResult::NOT_FOUND) => Ok(None),
            Ok(disc) => Ok(Some(disc)),
            Err(err) => Err(err),
        }
    }

    /// Reads the current `State<S>` `u32` discriminant declared in `query_index` at `term_index`.
    #[inline]
    pub fn current_in_query(
        &self,
        ctx: &mut SystemContext<'_>,
        query_index: usize,
        term_index: usize,
    ) -> Result<u32, FfiResult> {
        let iter = ctx.query_mut(query_index).ok_or(FfiResult::OUT_OF_RANGE)?;
        iter.inner
            .read_mapped_wire_resource(term_index, &self.state_vtable)
    }

    /// Returns `true` if `State<S>` declared in query `0` at `term_index` equals `discriminant`.
    #[inline]
    pub fn is_in_state(
        &self,
        ctx: &mut SystemContext<'_>,
        term_index: usize,
        discriminant: u32,
    ) -> Result<bool, FfiResult> {
        Ok(self.current(ctx, term_index)? == discriminant)
    }

    /// Queues a state transition via `ModNextState<S>` (`NextState::set_if_different`) declared in
    /// query `0` at `next_term_index`.
    #[inline]
    pub fn set_state_if_different(
        &self,
        ctx: &mut SystemContext<'_>,
        next_term_index: usize,
        discriminant: u32,
    ) -> FfiResult {
        ctx.iter().inner.write_mapped_wire_resource(
            next_term_index,
            &self.next_state_vtable,
            &discriminant,
        )
    }

    /// Queues a state transition via `ModNextState<S>` declared in `query_index` at `next_term_index`.
    #[inline]
    pub fn set_state_if_different_in_query(
        &self,
        ctx: &mut SystemContext<'_>,
        query_index: usize,
        next_term_index: usize,
        discriminant: u32,
    ) -> FfiResult {
        let Some(iter) = ctx.query_mut(query_index) else {
            return FfiResult::OUT_OF_RANGE;
        };
        iter.inner.write_mapped_wire_resource(
            next_term_index,
            &self.next_state_vtable,
            &discriminant,
        )
    }

    /// Reads the current `State<S>` `u32` discriminant by handle on a [`ValidatedSystemRef`].
    #[inline]
    pub fn current_by_handle(
        &self,
        sys_ref: &ValidatedSystemRef<'_>,
        state_handle: BamComponent,
    ) -> Result<(u32, BamChangeTicks), FfiResult> {
        sys_ref.read_mapped_wire_resource_by_handle(state_handle, &self.state_vtable)
    }

    /// Queues a state transition via `ModNextState<S>` handle on a [`ValidatedSystemRef`].
    #[inline]
    pub fn set_state_if_different_by_handle(
        &self,
        sys_ref: &mut ValidatedSystemRef<'_>,
        next_handle: BamComponent,
        discriminant: u32,
    ) -> Result<BamChangeTicks, FfiResult> {
        sys_ref.write_mapped_wire_resource_by_handle(
            next_handle,
            &self.next_state_vtable,
            &discriminant,
        )
    }

    /// Resolves the legacy slot-0 `DespawnOnExit` marker handle for `variant_slot` (`0..32`).
    pub fn resolve_despawn_on_exit(
        &self,
        host: &Host,
        variant_slot: usize,
    ) -> Result<BamComponent, FfiResult> {
        let name = format!(
            "bevy_state::DespawnOnExit#{}::{variant_slot}",
            self.state_slot
        );
        let cstr = CString::new(name.as_str()).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
        let uuid = ComponentUuid::from_canonical_name(&name);
        bamabi_guest_sys::resolve_component(host, Some(&cstr), Some(uuid))
    }

    /// Resolves the legacy slot-0 `DespawnOnEnter` marker handle for `variant_slot` (`0..32`).
    pub fn resolve_despawn_on_enter(
        &self,
        host: &Host,
        variant_slot: usize,
    ) -> Result<BamComponent, FfiResult> {
        let name = format!(
            "bevy_state::DespawnOnEnter#{}::{variant_slot}",
            self.state_slot
        );
        let cstr = CString::new(name.as_str()).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
        let uuid = ComponentUuid::from_canonical_name(&name);
        bamabi_guest_sys::resolve_component(host, Some(&cstr), Some(uuid))
    }
}

/// Extension trait on [`SystemBuilder`] attaching host-exposed `in_state` sets and `ModStateSlab`
/// transition message writers.
pub trait GuestStateSystemBuilderExt<'b> {
    /// Schedules this system inside `host_state_set` (which runs on the host iff `in_state(S::V)`
    /// holds).
    fn run_if_in_state(self, host_state_set: &'b CStr) -> Self;

    /// Declares a [`ModStateTransitionRequest`] message writer (`MESSAGE_WRITE`) so the system can
    /// request transitions on any `FreelyMutableState` or `SubStates` slot in host `ModStateSlab`.
    fn write_state_transitions(self) -> Self;
}

impl<'b, 'a> GuestStateSystemBuilderExt<'b> for SystemBuilder<'b, 'a> {
    /// Places the system inside the host-managed state-gated system set `host_state_set`.
    #[inline]
    fn run_if_in_state(self, host_state_set: &'b CStr) -> Self {
        self.in_set(host_state_set)
    }

    /// Appends `message_write::<ModStateTransitionRequest>()` to this system.
    #[inline]
    fn write_state_transitions(self) -> Self {
        self.message_write::<ModStateTransitionRequest>()
    }
}

/// Extension trait on [`SystemContext`] for requesting transitions on any dynamic `ModStateSlab`
/// handle (`FreelyMutableState` or `SubStates`).
pub trait GuestStateSlabContextExt {
    /// Sends a [`ModStateTransitionRequest`] for `(state, target_discriminant)` on the
    /// `MESSAGE_WRITE` stream declared at `decl_index`.
    fn request_state_transition(
        &mut self,
        decl_index: usize,
        state: ModStateHandle,
        target_discriminant: u32,
    ) -> FfiResult;
}

impl GuestStateSlabContextExt for SystemContext<'_> {
    #[inline]
    fn request_state_transition(
        &mut self,
        decl_index: usize,
        state: ModStateHandle,
        target_discriminant: u32,
    ) -> FfiResult {
        self.send_message(
            decl_index,
            &ModStateTransitionRequest::new(state, target_discriminant),
        )
    }
}

/// Extension trait on [`GuestCommands`] for attaching both legacy `DespawnOnExit`/`DespawnOnEnter`
/// marker handles and unbounded `ModStateSlab` [`ModDespawnOnExit`]/[`ModDespawnOnEnter`]
/// components.
pub trait GuestCommandsStateExt {
    /// Attaches a pre-resolved legacy `DespawnOnExit` marker component (`exit_marker`) onto `entity`.
    fn despawn_on_exit(&mut self, entity: FfiEntity, exit_marker: BamComponent) -> FfiResult;

    /// Attaches a pre-resolved legacy `DespawnOnEnter` marker component (`enter_marker`) onto `entity`.
    fn despawn_on_enter(&mut self, entity: FfiEntity, enter_marker: BamComponent) -> FfiResult;

    /// Attaches a dynamic [`ModDespawnOnExit`] component for `(state, discriminant)` onto `entity`.
    fn despawn_on_exit_slab(
        &mut self,
        entity: FfiEntity,
        state: ModStateHandle,
        discriminant: u32,
    ) -> FfiResult;

    /// Attaches a dynamic [`ModDespawnOnEnter`] component for `(state, discriminant)` onto `entity`.
    fn despawn_on_enter_slab(
        &mut self,
        entity: FfiEntity,
        state: ModStateHandle,
        discriminant: u32,
    ) -> FfiResult;
}

impl GuestCommandsStateExt for GuestCommands<'_> {
    #[inline]
    fn despawn_on_exit(&mut self, entity: FfiEntity, exit_marker: BamComponent) -> FfiResult {
        self.insert_default(entity, exit_marker)
    }

    #[inline]
    fn despawn_on_enter(&mut self, entity: FfiEntity, enter_marker: BamComponent) -> FfiResult {
        self.insert_default(entity, enter_marker)
    }

    #[inline]
    fn despawn_on_exit_slab(
        &mut self,
        entity: FfiEntity,
        state: ModStateHandle,
        discriminant: u32,
    ) -> FfiResult {
        self.insert_component(entity, &ModDespawnOnExit::new(state, discriminant))
    }

    #[inline]
    fn despawn_on_enter_slab(
        &mut self,
        entity: FfiEntity,
        state: ModStateHandle,
        discriminant: u32,
    ) -> FfiResult {
        self.insert_component(entity, &ModDespawnOnEnter::new(state, discriminant))
    }
}

/// Extension trait on [`QueryBuilder`] adding bridged `State<S>`, `ModNextState<S>`, and dynamic
/// `ModStateEntry` terms.
pub trait GuestStateQueryBuilderExt {
    /// Appends a shared resource read (`Res<State<S>>`) term using `bridge`.
    fn read_state(self, bridge: &GuestStateBridge) -> Self;
    /// Appends an exclusive resource write (`ResMut<ModNextState<S>>`) term using `bridge`.
    fn write_next_state(self, bridge: &GuestStateBridge) -> Self;
    /// Appends a shared resource read (`Res<State<S>>`) term for primary state slot `0`.
    fn read_primary_state(self) -> Self;
    /// Appends an exclusive resource write (`ResMut<ModNextState<S>>`) term for primary slot `0`.
    fn write_primary_next_state(self) -> Self;
    /// Appends a shared read (`&ModStateEntry`) term covering all dynamic `States`, `SubStates`,
    /// and `ComputedStates` entries in host `ModStateSlab`.
    fn read_state_entries(self) -> Self;
}

impl GuestStateQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_state(self, bridge: &GuestStateBridge) -> Self {
        self.resource_read_handle(bridge.state_handle())
    }

    #[inline]
    fn write_next_state(self, bridge: &GuestStateBridge) -> Self {
        self.resource_write_handle(bridge.next_state_handle())
    }

    #[inline]
    fn read_primary_state(self) -> Self {
        self.term_named(
            c"bevy_state::State#0",
            Some(ComponentUuid::from_canonical_name("bevy_state::State#0")),
            AccessMode::RESOURCE_READ,
        )
    }

    #[inline]
    fn write_primary_next_state(self) -> Self {
        self.term_named(
            c"bamabi::ModNextState#0",
            Some(ComponentUuid::from_canonical_name("bamabi::ModNextState#0")),
            AccessMode::RESOURCE_WRITE,
        )
    }

    #[inline]
    fn read_state_entries(self) -> Self {
        self.term_named(
            ModStateEntry::NAME,
            Some(ModStateEntry::UUID),
            AccessMode::READ,
        )
    }
}

/// Bundled state bridge + pre-resolved legacy `DespawnOnExit` / `DespawnOnEnter` marker handles
/// (`0..variant_count`) for primary state slot `0`.
#[derive(Clone)]
pub struct GuestStateHandles {
    /// Resolved state VTable and resource handle bridge.
    pub bridge: GuestStateBridge,
    /// `DespawnOnExit(variant)` marker handles indexed by `variant_slot`.
    pub despawn_on_exit: Vec<BamComponent>,
    /// `DespawnOnEnter(variant)` marker handles indexed by `variant_slot`.
    pub despawn_on_enter: Vec<BamComponent>,
}

impl GuestStateHandles {
    /// Resolves primary state slot `0` and all `variant_count` `DespawnOnExit` /
    /// `DespawnOnEnter` handles.
    pub fn resolve_primary(host: &Host, variant_count: usize) -> Result<Self, FfiResult> {
        Self::resolve_slot(host, 0, variant_count)
    }

    /// Resolves `slot` and all `variant_count` `DespawnOnExit` / `DespawnOnEnter` handles.
    pub fn resolve_slot(host: &Host, slot: usize, variant_count: usize) -> Result<Self, FfiResult> {
        let bridge = GuestStateBridge::resolve_slot(host, slot)?;
        let mut despawn_on_exit = Vec::with_capacity(variant_count);
        let mut despawn_on_enter = Vec::with_capacity(variant_count);
        for idx in 0..variant_count {
            despawn_on_exit.push(bridge.resolve_despawn_on_exit(host, idx)?);
            despawn_on_enter.push(bridge.resolve_despawn_on_enter(host, idx)?);
        }
        Ok(Self {
            bridge,
            despawn_on_exit,
            despawn_on_enter,
        })
    }

    /// Returns the `DespawnOnExit` handle for `variant_slot`, if present.
    #[inline]
    pub fn exit_marker(&self, variant_slot: usize) -> Option<BamComponent> {
        self.despawn_on_exit.get(variant_slot).copied()
    }

    /// Returns the `DespawnOnEnter` handle for `variant_slot`, if present.
    #[inline]
    pub fn enter_marker(&self, variant_slot: usize) -> Option<BamComponent> {
        self.despawn_on_enter.get(variant_slot).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: Verifies that `on_enter_schedule`, `on_exit_schedule`, and
    /// `on_transition_schedule` compute deterministic `ScheduleId::custom(...)` offsets matching
    /// the host's contiguous state-transition layout.
    #[test]
    fn deterministic_state_schedule_ids_match_host_layout() {
        let base = ScheduleId::custom(2000);
        assert_eq!(on_enter_schedule(base, 0), ScheduleId::custom(2000));
        assert_eq!(on_enter_schedule(base, 1), ScheduleId::custom(2001));
        assert_eq!(on_exit_schedule(base, 3, 0), ScheduleId::custom(2003));
        assert_eq!(on_exit_schedule(base, 3, 2), ScheduleId::custom(2005));
        assert_eq!(
            on_transition_schedule(base, 3, 0, 1),
            ScheduleId::custom(2007)
        );
        assert_eq!(
            on_transition_schedule(base, 3, 2, 1),
            ScheduleId::custom(2013)
        );
    }
}
