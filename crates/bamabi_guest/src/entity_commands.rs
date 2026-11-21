//! Fluent Bevy-idiomatic `EntityCommands` ([`GuestEntityCommands`]) and `ChildSpawnerCommands`
//! ([`GuestChildSpawner`]) wrappers over [`GuestCommands`] (`#![forbid(unsafe_code)]`).

use bamabi_guest_sys::SpawnEntry;
use bamabi_sys::{AbiPod, BamComponent, FfiDisabled, FfiEntity, FfiResult, ModComponent};

use crate::{
    bridges::state::{GuestCommandsStateExt, ModStateHandle},
    commands::GuestCommands,
    component::GuestComponent,
};

/// Fluent entity-scoped deferred command builder mirroring Bevy's `EntityCommands<'a>`.
///
/// Created via [`GuestCommands::entity`], [`GuestCommands::spawn_entity`],
/// [`GuestCommands::spawn_entity_with`], or [`GuestCommands::spawn_entity_persistent`]. Every
/// chained operation accumulates the first non-`OK` [`FfiResult`] (accessible via [`Self::status`]
/// or [`Self::finish`]) and fails closed without panicking.
pub struct GuestEntityCommands<'cmd, 'a> {
    commands: &'cmd mut GuestCommands<'a>,
    entity: FfiEntity,
    first_error: Option<FfiResult>,
}

impl<'cmd, 'a> GuestEntityCommands<'cmd, 'a> {
    /// Creates a fluent entity command builder targeting `entity`.
    #[inline]
    pub fn new(commands: &'cmd mut GuestCommands<'a>, entity: FfiEntity) -> Self {
        let first_error = if entity.is_null() {
            Some(FfiResult::INVALID_ENTITY)
        } else {
            None
        };
        Self {
            commands,
            entity,
            first_error,
        }
    }

    #[inline]
    fn record(&mut self, status: FfiResult) {
        if !status.is_ok() && self.first_error.is_none() {
            self.first_error = Some(status);
        }
    }

    /// Returns the targeted [`FfiEntity`] identifier.
    #[inline]
    pub const fn id(&self) -> FfiEntity {
        self.entity
    }

    /// Returns the first non-`OK` [`FfiResult`] recorded by any chained command, or `FfiResult::OK`.
    #[inline]
    pub const fn status(&self) -> FfiResult {
        match self.first_error {
            Some(err) => err,
            None => FfiResult::OK,
        }
    }

    /// Consumes this builder, returning `Ok(self.id())` if every queued command succeeded or the
    /// first [`FfiResult`] error encountered.
    #[inline]
    pub const fn finish(self) -> Result<FfiEntity, FfiResult> {
        match self.first_error {
            Some(err) => Err(err),
            None => Ok(self.entity),
        }
    }

    /// Borrows the underlying [`GuestCommands`] mutably.
    #[inline]
    pub fn commands(&mut self) -> &mut GuestCommands<'a> {
        self.commands
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) under pre-resolved `component`.
    #[inline]
    pub fn insert<T: AbiPod>(&mut self, component: BamComponent, value: &T) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert(self.entity, component, value);
            self.record(status);
        }
        self
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) under an init-validated
    /// [`crate::TypedComponent`] without a per-command `component_layout_info` FFI round-trip.
    #[inline]
    pub fn insert_typed<T: AbiPod>(
        &mut self,
        component: crate::TypedComponent<T>,
        value: &T,
    ) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_typed(self.entity, component, value);
            self.record(status);
        }
        self
    }

    /// Queues deferred conditional insertion (`InsertMode::Keep`) of `value` (`T: AbiPod`) under
    /// an init-validated [`crate::TypedComponent`] without a per-command `component_layout_info`
    /// FFI round-trip.
    #[inline]
    pub fn insert_if_new_typed<T: AbiPod>(
        &mut self,
        component: crate::TypedComponent<T>,
        value: &T,
    ) -> &mut Self {
        if self.first_error.is_none()
            && let Err(err) = self
                .commands
                .insert_if_new_typed(self.entity, component, value)
        {
            self.record(err);
        }
        self
    }

    /// Resolves `T: GuestComponent` and queues deferred insertion of `value`.
    #[inline]
    pub fn insert_component<T: GuestComponent>(&mut self, value: &T) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_component(self.entity, value);
            self.record(status);
        }
        self
    }

    /// Queues deferred insertion of host-constructed `T::default()` for pre-resolved `component`.
    #[inline]
    pub fn insert_default(&mut self, component: BamComponent) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_default(self.entity, component);
            self.record(status);
        }
        self
    }

    /// Resolves `T: GuestComponent` and queues deferred insertion of host `T::default()`.
    #[inline]
    pub fn insert_component_default<T: GuestComponent>(&mut self) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_component_default::<T>(self.entity);
            self.record(status);
        }
        self
    }

    /// Queues deferred conditional insertion (`InsertMode::Keep`) of `value` under `component`.
    #[inline]
    pub fn insert_if_new<T: AbiPod>(&mut self, component: BamComponent, value: &T) -> &mut Self {
        if self.first_error.is_none()
            && let Err(err) = self.commands.insert_if_new(self.entity, component, value)
        {
            self.record(err);
        }
        self
    }

    /// Resolves `T: GuestComponent` and queues deferred conditional insertion (`InsertMode::Keep`).
    #[inline]
    pub fn insert_component_if_new<T: GuestComponent>(&mut self, value: &T) -> &mut Self {
        if self.first_error.is_none()
            && let Err(err) = self.commands.insert_component_if_new(self.entity, value)
        {
            self.record(err);
        }
        self
    }

    /// Moves an owned dynamic [`ModComponent`] `value` onto this entity under `component`.
    #[inline]
    pub fn insert_dynamic<T: ModComponent>(
        &mut self,
        component: BamComponent,
        value: T,
    ) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_dynamic(self.entity, component, value);
            self.record(status);
        }
        self
    }

    /// Resolves `T: ModComponent` and moves `value` onto this entity.
    #[inline]
    pub fn insert_dynamic_component<T: ModComponent>(&mut self, value: T) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_dynamic_component(self.entity, value);
            self.record(status);
        }
        self
    }

    /// Queues deferred removal of pre-resolved `component` from this entity.
    #[inline]
    pub fn remove(&mut self, component: BamComponent) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.remove(self.entity, component);
            self.record(status);
        }
        self
    }

    /// Resolves `T: GuestComponent` and queues deferred removal from this entity.
    #[inline]
    pub fn remove_component<T: GuestComponent>(&mut self) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.remove_component::<T>(self.entity);
            self.record(status);
        }
        self
    }

    /// Attaches a marker/tag relationship edge `self --[relation]--> target`.
    #[inline]
    pub fn insert_relation(&mut self, relation: BamComponent, target: FfiEntity) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.insert_relation(self.entity, relation, target);
            self.record(status);
        }
        self
    }

    /// Attaches a payload-carrying relationship edge `self --[relation(payload)]--> target`.
    #[inline]
    pub fn insert_relation_with_payload<T: AbiPod>(
        &mut self,
        relation: BamComponent,
        target: FfiEntity,
        payload: &T,
    ) -> &mut Self {
        if self.first_error.is_none() {
            let status =
                self.commands
                    .insert_relation_with_payload(self.entity, relation, target, payload);
            self.record(status);
        }
        self
    }

    /// Removes relationship edge `self --[relation]--> target`.
    #[inline]
    pub fn remove_relation(&mut self, relation: BamComponent, target: FfiEntity) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.remove_relation(self.entity, relation, target);
            self.record(status);
        }
        self
    }

    /// Sets `parent` as this entity's hierarchy parent (`ChildOf(parent)`).
    #[inline]
    pub fn set_parent(&mut self, parent: FfiEntity) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.add_child(parent, self.entity);
            self.record(status);
        }
        self
    }

    /// Detaches this entity from its current hierarchy parent (`remove::<ChildOf>()`).
    #[inline]
    pub fn remove_parent(&mut self) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.remove_parent(self.entity);
            self.record(status);
        }
        self
    }

    /// Attaches `child` as a hierarchy child of this entity (`ChildOf(self)`).
    #[inline]
    pub fn add_child(&mut self, child: FfiEntity) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.add_child(self.entity, child);
            self.record(status);
        }
        self
    }

    /// Attaches every entity in `children` as a hierarchy child of this entity.
    #[inline]
    pub fn add_children(&mut self, children: &[FfiEntity]) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.add_children(self.entity, children);
            self.record(status);
        }
        self
    }

    /// Spawns or attaches child entities under this parent using a scoped [`GuestChildSpawner`]
    /// (mirroring Bevy's `EntityCommands::with_children`).
    pub fn with_children(&mut self, f: impl FnOnce(&mut GuestChildSpawner<'_, 'a>)) -> &mut Self {
        if self.first_error.is_none() {
            let mut spawner = GuestChildSpawner {
                commands: self.commands,
                parent: self.entity,
                first_error: None,
            };
            f(&mut spawner);
            if let Some(err) = spawner.first_error {
                self.record(err);
            }
        }
        self
    }

    /// Disables this entity by inserting Bevy `0.20`'s [`FfiDisabled`] (`Disabled`) marker.
    #[inline]
    pub fn disable(&mut self) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.disable_entity(self.entity);
            self.record(status);
        }
        self
    }

    /// Re-enables this entity by removing Bevy `0.20`'s [`FfiDisabled`] (`Disabled`) marker.
    #[inline]
    pub fn enable(&mut self) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.enable_entity(self.entity);
            self.record(status);
        }
        self
    }

    /// Clones all cloneable components from this entity onto `dst` via
    /// `BamHostApi::cmd_clone_entity` (slot 70).
    #[inline]
    pub fn clone_to(&mut self, dst: FfiEntity) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.clone_entity(self.entity, dst);
            self.record(status);
        }
        self
    }

    /// Attaches a pre-resolved `DespawnOnExit` state-scoped lifetime marker to this entity.
    #[inline]
    pub fn despawn_on_exit(&mut self, exit_marker: BamComponent) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.despawn_on_exit(self.entity, exit_marker);
            self.record(status);
        }
        self
    }

    /// Attaches a pre-resolved `DespawnOnEnter` state-scoped lifetime marker to this entity.
    #[inline]
    pub fn despawn_on_enter(&mut self, enter_marker: BamComponent) -> &mut Self {
        if self.first_error.is_none() {
            let status = self.commands.despawn_on_enter(self.entity, enter_marker);
            self.record(status);
        }
        self
    }

    /// Attaches a `ModStateSlab` `ModDespawnOnExit` marker for `(state, variant)` to this entity.
    #[inline]
    pub fn despawn_on_exit_slab(&mut self, state: ModStateHandle, variant: u32) -> &mut Self {
        if self.first_error.is_none() {
            let status = self
                .commands
                .despawn_on_exit_slab(self.entity, state, variant);
            self.record(status);
        }
        self
    }

    /// Attaches a `ModStateSlab` `ModDespawnOnEnter` marker for `(state, variant)` to this entity.
    #[inline]
    pub fn despawn_on_enter_slab(&mut self, state: ModStateHandle, variant: u32) -> &mut Self {
        if self.first_error.is_none() {
            let status = self
                .commands
                .despawn_on_enter_slab(self.entity, state, variant);
            self.record(status);
        }
        self
    }

    /// Attaches a runtime entity-scoped observer for `EntityEvent` `E: GuestComponent` watching
    /// this entity (mirroring Bevy's `EntityCommands::observe`).
    #[inline]
    pub fn observe<E, F>(&mut self, callback: F) -> &mut Self
    where
        E: GuestComponent,
        F: FnMut(FfiEntity, &E, &mut GuestCommands<'_>) + Send + 'static,
    {
        if self.first_error.is_none()
            && let Err(err) = self.commands.observe_entity::<E, _>(self.entity, callback)
        {
            self.record(err);
        }
        self
    }

    /// Attaches a mutable runtime entity-scoped observer for `EntityEvent` `E: GuestComponent`
    /// watching this entity.
    #[inline]
    pub fn observe_mut<E, F>(&mut self, callback: F) -> &mut Self
    where
        E: GuestComponent,
        F: FnMut(FfiEntity, &mut E, &mut bool, &mut GuestCommands<'_>) + Send + 'static,
    {
        if self.first_error.is_none()
            && let Err(err) = self
                .commands
                .observe_entity_mut::<E, _>(self.entity, callback)
        {
            self.record(err);
        }
        self
    }

    /// Triggers an entity-targeted `EntityEvent` `E: GuestComponent` on this entity.
    #[inline]
    pub fn trigger<E: GuestComponent>(&mut self, event: &E) -> &mut Self {
        if self.first_error.is_none()
            && let Err(err) = self.commands.trigger_entity(self.entity, event)
        {
            self.record(err);
        }
        self
    }

    /// Triggers an upward-propagating `EntityEvent` `E: GuestComponent` starting at this entity
    /// along `ChildOf`.
    #[inline]
    pub fn trigger_propagated<E: GuestComponent>(&mut self, event: &E) -> &mut Self {
        if self.first_error.is_none()
            && let Err(err) = self.commands.trigger_propagated(self.entity, event)
        {
            self.record(err);
        }
        self
    }

    /// Consumes this builder and queues deferred despawning of the entity, returning the first
    /// error recorded (if any) or the despawn [`FfiResult`].
    #[inline]
    pub fn despawn(self) -> FfiResult {
        if let Some(err) = self.first_error {
            return err;
        }
        self.commands.despawn(self.entity)
    }
}

/// Scoped child spawner passed to [`GuestEntityCommands::with_children`] (mirroring Bevy's
/// `ChildSpawnerCommands`).
pub struct GuestChildSpawner<'cmd, 'a> {
    commands: &'cmd mut GuestCommands<'a>,
    parent: FfiEntity,
    first_error: Option<FfiResult>,
}

impl<'cmd, 'a> GuestChildSpawner<'cmd, 'a> {
    /// Returns the parent [`FfiEntity`] that spawned children attach to.
    #[inline]
    pub const fn parent(&self) -> FfiEntity {
        self.parent
    }

    /// Returns the first non-`OK` [`FfiResult`] recorded during child operations, or `OK`.
    #[inline]
    pub const fn status(&self) -> FfiResult {
        match self.first_error {
            Some(err) => err,
            None => FfiResult::OK,
        }
    }

    /// Borrows the underlying [`GuestCommands`] mutably.
    #[inline]
    pub fn commands(&mut self) -> &mut GuestCommands<'a> {
        self.commands
    }

    /// Spawns a new child entity carrying `ChildOf(self.parent())` and returns a fluent
    /// [`GuestEntityCommands`] builder for it.
    pub fn spawn_empty(&mut self) -> Result<GuestEntityCommands<'_, 'a>, FfiResult> {
        self.spawn(&[])
    }

    /// Spawns a new child entity carrying `ChildOf(self.parent())` plus `entries` and returns a
    /// fluent [`GuestEntityCommands`] builder for it.
    pub fn spawn(
        &mut self,
        entries: &[SpawnEntry<'_>],
    ) -> Result<GuestEntityCommands<'_, 'a>, FfiResult> {
        match self.commands.spawn_child(self.parent, entries) {
            Ok(child) => Ok(GuestEntityCommands::new(self.commands, child)),
            Err(err) => {
                if self.first_error.is_none() {
                    self.first_error = Some(err);
                }
                Err(err)
            }
        }
    }

    /// Attaches an existing `child` entity under `self.parent()`.
    #[inline]
    pub fn add_child(&mut self, child: FfiEntity) -> &mut Self {
        let status = self.commands.add_child(self.parent, child);
        if !status.is_ok() && self.first_error.is_none() {
            self.first_error = Some(status);
        }
        self
    }
}

impl<'a> GuestCommands<'a> {
    /// Returns a fluent [`GuestEntityCommands`] builder targeting `entity` (mirroring Bevy's
    /// `Commands::entity`).
    #[inline]
    pub fn entity(&mut self, entity: FfiEntity) -> GuestEntityCommands<'_, 'a> {
        GuestEntityCommands::new(self, entity)
    }

    /// Spawns an empty mod-scoped entity and returns a fluent [`GuestEntityCommands`] builder for
    /// chaining `.insert(...)`, `.with_children(...)`, `.observe(...)`, etc.
    #[inline]
    pub fn spawn_entity(&mut self) -> Result<GuestEntityCommands<'_, 'a>, FfiResult> {
        let entity = self.spawn_empty()?;
        Ok(GuestEntityCommands::new(self, entity))
    }

    /// Spawns a mod-scoped entity with initial batched `entries` and returns a fluent
    /// [`GuestEntityCommands`] builder for additional chained operations.
    #[inline]
    pub fn spawn_entity_with(
        &mut self,
        entries: &[SpawnEntry<'_>],
    ) -> Result<GuestEntityCommands<'_, 'a>, FfiResult> {
        let entity = self.spawn(entries)?;
        Ok(GuestEntityCommands::new(self, entity))
    }

    /// Spawns an empty persistent entity (surviving mod unload) and returns a fluent
    /// [`GuestEntityCommands`] builder.
    #[inline]
    pub fn spawn_entity_persistent(&mut self) -> Result<GuestEntityCommands<'_, 'a>, FfiResult> {
        let entity = self.spawn_empty_persistent()?;
        Ok(GuestEntityCommands::new(self, entity))
    }

    /// Attaches every entity in `children` as a hierarchy child of `parent` (`ChildOf(parent)`),
    /// stopping and returning the first error if any entity is invalid.
    pub fn add_children(&mut self, parent: FfiEntity, children: &[FfiEntity]) -> FfiResult {
        if parent.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        for &child in children {
            let status = self.add_child(parent, child);
            if !status.is_ok() {
                return status;
            }
        }
        FfiResult::OK
    }

    /// Disables `entity` by inserting Bevy `0.20`'s `bevy_ecs::entity_disabling::Disabled` marker
    /// ([`FfiDisabled`]).
    ///
    /// Uses `cmd_insert_default` first so bridged host `Disabled` registrations (such as
    /// `StdTypesBridgePlugin`'s immutable `DisabledBridge`, which denies raw `cmd_insert`) succeed
    /// out of the box, falling back to raw ZST `cmd_insert` only if `cmd_insert_default` reports
    /// `PERMISSION_DENIED`.
    #[inline]
    pub fn disable_entity(&mut self, entity: FfiEntity) -> FfiResult {
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        let handle = match self.resolve_component(
            Some(c"bevy_ecs::entity_disabling::Disabled"),
            Some(FfiDisabled::UUID),
        ) {
            Ok(h) => h,
            Err(err) => return err,
        };
        let status = self.insert_default(entity, handle);
        if status == FfiResult::PERMISSION_DENIED {
            self.insert(entity, handle, &FfiDisabled)
        } else {
            status
        }
    }

    /// Re-enables `entity` by removing Bevy `0.20`'s `bevy_ecs::entity_disabling::Disabled` marker
    /// ([`FfiDisabled`]).
    #[inline]
    pub fn enable_entity(&mut self, entity: FfiEntity) -> FfiResult {
        if entity.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        self.remove_component::<FfiDisabled>(entity)
    }
}
