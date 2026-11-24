//! Safe handle-based and batched deferred command buffer (`GuestCommands`).

use core::{
    ffi::CStr,
    ops::{Deref, DerefMut},
};

use bamabi_guest_sys::{Host, SpawnEntry, ValidatedCommands, resolve_component};
pub use bamabi_guest_sys::{SpawnBatchColumn, TypedComponent};
use bamabi_sys::{
    AbiPod, BamCommands, BamComponent, BamTermDesc, ComponentUuid, FfiEntity, FfiResult,
};

use crate::{
    app::QueryBuilder,
    component::{GuestComponent, resolve_guest_component},
};

/// Safe deferred command buffer bound to an active system execution context.
///
/// Supports O(1) pre-resolved [`BamComponent`] handle operations (`spawn_empty`, batched `spawn`,
/// `despawn`, `insert`, `insert_default`, `remove`) as well as typed [`GuestComponent`] convenience
/// helpers.
///
/// Constructing `GuestCommands` from raw C pointers in safe Rust is rejected at compile time:
///
/// ```compile_fail
/// use bamabi_guest::GuestCommands;
///
/// let _cmds = GuestCommands::from_raw(core::ptr::null_mut());
/// ```
///
/// Inserting non-`AbiPod` types (such as `String` or `bool`) is also rejected at compile time:
///
/// ```compile_fail
/// use bamabi_guest::{BamComponent, FfiEntity, GuestCommands};
///
/// fn bad_insert(cmds: &mut GuestCommands<'_>, e: FfiEntity, c: BamComponent) {
///     let s = String::from("heap");
///     let _ = cmds.insert(e, c, &s);
/// }
/// ```
pub struct GuestCommands<'a> {
    pub(crate) inner: ValidatedCommands<'a>,
}

impl<'a> GuestCommands<'a> {
    /// Wraps an already-validated [`ValidatedCommands`] handle.
    #[inline]
    pub const fn from_validated(inner: ValidatedCommands<'a>) -> Self {
        Self { inner }
    }

    /// Returns `true` if this command buffer holds an active non-null [`BamCommands`] handle.
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.inner.is_valid()
    }

    /// Returns the active [`Host`] table for this mod instance, lazily resolving
    /// `BamHostApi::sys_host_ctx` on demand when backed by a system trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        self.inner.host()
    }

    /// Returns the raw `*mut BamCommands` handle for FFI interoperability.
    #[inline]
    pub const fn as_raw_ptr(&self) -> *mut BamCommands {
        self.inner.as_raw()
    }

    /// Resolves a [`BamComponent`] handle by `name`, `uuid`, or both via the host registry.
    #[inline]
    pub fn resolve_component(
        &self,
        name: Option<&CStr>,
        uuid: Option<ComponentUuid>,
    ) -> Result<BamComponent, FfiResult> {
        resolve_component(self.inner.host(), name, uuid)
    }

    /// Reserves a fresh empty entity atomically and returns its [`FfiEntity`] identifier.
    #[inline]
    pub fn spawn_empty(&mut self) -> Result<FfiEntity, FfiResult> {
        self.inner.spawn_empty()
    }

    /// Reserves a fresh entity and batch-inserts `entries` in a single archetype transition.
    #[inline]
    pub fn spawn(&mut self, entries: &[SpawnEntry<'_>]) -> Result<FfiEntity, FfiResult> {
        self.inner.spawn(entries)
    }

    /// Queues deferred despawning of `entity`.
    #[inline]
    pub fn despawn(&mut self, entity: FfiEntity) -> FfiResult {
        self.inner.despawn(entity)
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) onto `entity` using pre-resolved
    /// [`BamComponent`] handle.
    #[inline]
    pub fn insert<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: &T,
    ) -> FfiResult {
        self.inner.insert(entity, component, value)
    }

    /// Queues deferred insertion of `value` (`T: AbiPod`) onto `entity` using an init-validated
    /// [`TypedComponent<T>`] handle.
    #[inline]
    pub fn insert_typed<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: TypedComponent<T>,
        value: &T,
    ) -> FfiResult {
        self.inner.insert_typed(entity, component, value)
    }

    /// Queues deferred insertion of host-constructed `T::default()` for pre-resolved `component`
    /// onto `entity`.
    #[inline]
    pub fn insert_default(&mut self, entity: FfiEntity, component: BamComponent) -> FfiResult {
        self.inner.insert_default(entity, component)
    }

    /// Queues deferred removal of pre-resolved `component` from `entity`.
    #[inline]
    pub fn remove(&mut self, entity: FfiEntity, component: BamComponent) -> FfiResult {
        self.inner.remove(entity, component)
    }

    /// Convenience helper resolving `T: GuestComponent` (including manifest-qualified names)
    /// and inserting `value`.
    ///
    /// Prefer pre-resolving [`BamComponent`] handles at mod init for hot per-frame loops.
    #[inline]
    pub fn insert_component<T: GuestComponent>(
        &mut self,
        entity: FfiEntity,
        value: &T,
    ) -> FfiResult {
        let host = self.inner.host();
        match resolve_guest_component::<T>(host, host.manifest_name()) {
            Ok(comp) => self.inner.insert(entity, comp, value),
            Err(err) => err,
        }
    }

    /// Convenience helper resolving `T: GuestComponent` (including manifest-qualified names)
    /// and inserting `T::default()` on the host.
    #[inline]
    pub fn insert_component_default<T: GuestComponent>(&mut self, entity: FfiEntity) -> FfiResult {
        let host = self.inner.host();
        match resolve_guest_component::<T>(host, host.manifest_name()) {
            Ok(comp) => self.inner.insert_default(entity, comp),
            Err(err) => err,
        }
    }

    /// Convenience helper resolving `T: GuestComponent` (including manifest-qualified names)
    /// and removing `T` from `entity`.
    #[inline]
    pub fn remove_component<T: GuestComponent>(&mut self, entity: FfiEntity) -> FfiResult {
        let host = self.inner.host();
        match resolve_guest_component::<T>(host, host.manifest_name()) {
            Ok(comp) => self.inner.remove(entity, comp),
            Err(err) => err,
        }
    }

    /// Triggers a global Bevy `Event` (`E: GuestComponent`) resolved on the host via deferred
    /// `world.trigger(...)`.
    #[inline]
    pub fn trigger<E: GuestComponent>(&mut self, event: &E) -> Result<(), FfiResult> {
        self.trigger_entity::<E>(FfiEntity::NULL, event)
    }

    /// Triggers an entity-targeted Bevy `EntityEvent` (`E: GuestComponent`) on `target` via deferred
    /// `world.trigger(...)` / `world.trigger_dynamic_targets`.
    #[inline]
    pub fn trigger_entity<E: GuestComponent>(
        &mut self,
        target: FfiEntity,
        event: &E,
    ) -> Result<(), FfiResult> {
        let host = self.inner.host();
        let comp = resolve_guest_component::<E>(host, host.manifest_name())?;
        self.trigger_handle(comp, target, event)
    }

    /// Triggers a pre-resolved event handle `handle` (Pass [`FfiEntity::NULL`] for global triggers).
    #[inline]
    pub fn trigger_handle<E: AbiPod>(
        &mut self,
        handle: BamComponent,
        target: FfiEntity,
        event: &E,
    ) -> Result<(), FfiResult> {
        let status = self.inner.trigger(handle, target, event);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Spawns `entity_count` entities in one FFI call using Structure-of-Arrays component
    /// columns, returning all reserved [`FfiEntity`] handles.
    #[inline]
    pub fn spawn_batch(
        &mut self,
        columns: &[SpawnBatchColumn<'_>],
        entity_count: usize,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        let mut out = vec![FfiEntity::NULL; entity_count];
        let status = self
            .inner
            .spawn_batch_columns(columns, entity_count, Some(&mut out));
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Spawns `entity_count` entities in one FFI call using Structure-of-Arrays component
    /// columns, optionally writing reserved [`FfiEntity`] handles into `out_entities`.
    #[inline]
    pub fn spawn_batch_into(
        &mut self,
        columns: &[SpawnBatchColumn<'_>],
        entity_count: usize,
        out_entities: Option<&mut [FfiEntity]>,
    ) -> Result<(), FfiResult> {
        let status = self
            .inner
            .spawn_batch_columns(columns, entity_count, out_entities);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues a deferred query-filtered bulk despawn of non-resource entities matching `terms`.
    #[inline]
    pub fn despawn_all(&mut self, terms: &[BamTermDesc]) -> Result<(), FfiResult> {
        let status = self.inner.despawn_all(terms);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues a deferred query-filtered bulk despawn of non-resource entities matching the
    /// filter configured via `configure(|q| ...)` (using [`QueryBuilder`]).
    #[inline]
    pub fn despawn_all_with<F>(&mut self, configure: F) -> Result<(), FfiResult>
    where
        F: for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
    {
        let host = *self.inner.host();
        let qb = QueryBuilder::new(&host, host.manifest_name());
        let finished = configure(qb);
        let terms = finished.into_terms()?;
        self.despawn_all(&terms)
    }

    /// Queues a deferred despawn of `entities` in one FFI call via
    /// `BamHostApi::cmd_despawn_batch` (slot 85).
    #[inline]
    pub fn despawn_batch(&mut self, entities: &[FfiEntity]) -> Result<(), FfiResult> {
        let status = self.inner.despawn_batch(entities);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues deferred insertion of `component` (`T: AbiPod`) across `entities` in one FFI call
    /// via `BamHostApi::cmd_insert_batch` (slot 86).
    #[inline]
    pub fn insert_batch<T: AbiPod>(
        &mut self,
        entities: &[FfiEntity],
        component: BamComponent,
        data: &[T],
    ) -> Result<(), FfiResult> {
        let status = self.inner.insert_batch(entities, component, data);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Init-validated [`TypedComponent<T>`] variant of [`Self::insert_batch`].
    #[inline]
    pub fn insert_batch_typed<T: AbiPod>(
        &mut self,
        entities: &[FfiEntity],
        component: TypedComponent<T>,
        data: &[T],
    ) -> Result<(), FfiResult> {
        let status = self.inner.insert_batch_typed(entities, component, data);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues deferred stride-0 splat insertion (`BAM_SPAWN_COL_FLAG_SPLAT`) of a single `value`
    /// (`T: AbiPod`) onto all `entities` in one FFI call via `BamHostApi::cmd_insert_batch` (slot 86).
    #[inline]
    pub fn insert_batch_splat<T: AbiPod>(
        &mut self,
        entities: &[FfiEntity],
        component: BamComponent,
        value: &T,
    ) -> Result<(), FfiResult> {
        let status = self.inner.insert_batch_splat(entities, component, value);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Init-validated [`TypedComponent<T>`] variant of [`Self::insert_batch_splat`].
    #[inline]
    pub fn insert_batch_splat_typed<T: AbiPod>(
        &mut self,
        entities: &[FfiEntity],
        component: TypedComponent<T>,
        value: &T,
    ) -> Result<(), FfiResult> {
        let status = self
            .inner
            .insert_batch_splat_typed(entities, component, value);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues deferred removal of `component` from all `entities` in one FFI call via
    /// `BamHostApi::cmd_remove_batch` (slot 87).
    #[inline]
    pub fn remove_batch(
        &mut self,
        entities: &[FfiEntity],
        component: BamComponent,
    ) -> Result<(), FfiResult> {
        let status = self.inner.remove_batch(entities, component);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Spawns `entity_count` entities via `BamHostApi::cmd_spawn_batch_ex` (slot 89), supporting
    /// both SoA columns and stride-0 splat (`BAM_SPAWN_COL_FLAG_SPLAT`) columns.
    #[inline]
    pub fn spawn_batch_ex(
        &mut self,
        columns: &[SpawnBatchColumn<'_>],
        entity_count: usize,
        out_entities: Option<&mut [FfiEntity]>,
    ) -> Result<(), FfiResult> {
        let status = self
            .inner
            .spawn_batch_ex(columns, entity_count, out_entities);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues a deferred conditional insertion (`InsertMode::Keep`) of `value` (`T: AbiPod`) onto
    /// `entity` only if `component` is not already present.
    #[inline]
    pub fn insert_if_new<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: BamComponent,
        value: &T,
    ) -> Result<(), FfiResult> {
        let status = self.inner.insert_if_new(entity, component, value);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Queues a deferred conditional insertion (`InsertMode::Keep`) of `value` (`T: AbiPod`) onto
    /// `entity` using an init-validated [`TypedComponent<T>`] handle.
    #[inline]
    pub fn insert_if_new_typed<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        component: TypedComponent<T>,
        value: &T,
    ) -> Result<(), FfiResult> {
        let status = self.inner.insert_if_new_typed(entity, component, value);
        if status.is_ok() { Ok(()) } else { Err(status) }
    }

    /// Resolves `T: GuestComponent` and queues a deferred conditional insertion (`InsertMode::Keep`)
    /// onto `entity` only if absent.
    #[inline]
    pub fn insert_component_if_new<T: GuestComponent>(
        &mut self,
        entity: FfiEntity,
        value: &T,
    ) -> Result<(), FfiResult> {
        let host = self.inner.host();
        let comp = resolve_guest_component::<T>(host, host.manifest_name())?;
        self.insert_if_new(entity, comp, value)
    }

    /// Queues deferred insertion of a marker/tag relationship edge
    /// `entity --[relation]--> target` (`BamHostApi::cmd_insert_relation`).
    #[inline]
    pub fn insert_relation(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> FfiResult {
        self.inner.insert_relation(entity, relation, target)
    }

    /// Queues deferred insertion of a payload-carrying relationship edge
    /// `entity --[relation(payload)]--> target` (`T: AbiPod`) via
    /// `BamHostApi::cmd_insert_relation`.
    #[inline]
    pub fn insert_relation_with_payload<T: AbiPod>(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
        payload: &T,
    ) -> FfiResult {
        self.inner
            .insert_relation_with_payload(entity, relation, target, payload)
    }

    /// Queues deferred removal of a typed relationship edge `entity --[relation]--> target`
    /// (`BamHostApi::cmd_remove_relation`).
    #[inline]
    pub fn remove_relation(
        &mut self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> FfiResult {
        self.inner.remove_relation(entity, relation, target)
    }

    /// Clones all cloneable components from `src_entity` onto `dst_entity` via
    /// `BamHostApi::cmd_clone_entity` (slot 70).
    #[inline]
    pub fn clone_entity(&mut self, src_entity: FfiEntity, dst_entity: FfiEntity) -> FfiResult {
        self.inner.clone_entity(src_entity, dst_entity)
    }

    /// Queues a deferred singleton resource insert for pre-resolved `resource` (`R: AbiPod`) via
    /// `BamHostApi::cmd_insert_resource` (slot 71).
    #[inline]
    pub fn insert_resource<R: AbiPod>(&mut self, resource: BamComponent, value: &R) -> FfiResult {
        self.inner.insert_resource(resource, value)
    }

    /// Queues a deferred singleton resource insert using a pre-validated [`TypedComponent<R>`]
    /// handle via `BamHostApi::cmd_insert_resource` (slot 71).
    #[inline]
    pub fn insert_resource_typed<R: AbiPod>(
        &mut self,
        resource: TypedComponent<R>,
        value: &R,
    ) -> FfiResult {
        self.inner.insert_resource_typed(resource, value)
    }

    /// Resolves `R: GuestComponent` and queues a deferred singleton resource insert via
    /// `BamHostApi::cmd_insert_resource` (slot 71).
    #[inline]
    pub fn insert_guest_resource<R: GuestComponent>(&mut self, value: &R) -> FfiResult {
        let host = self.host();
        match resolve_guest_component::<R>(host, host.manifest_name()) {
            Ok(comp) => self.inner.insert_resource(comp, value),
            Err(err) => err,
        }
    }

    /// Queues a deferred singleton resource removal for pre-resolved `resource` via
    /// `BamHostApi::cmd_remove_resource` (slot 72).
    #[inline]
    pub fn remove_resource(&mut self, resource: BamComponent) -> FfiResult {
        self.inner.remove_resource(resource)
    }

    /// Resolves `R: GuestComponent` and queues a deferred singleton resource removal via
    /// `BamHostApi::cmd_remove_resource` (slot 72).
    #[inline]
    pub fn remove_guest_resource<R: GuestComponent>(&mut self) -> FfiResult {
        let host = self.host();
        match resolve_guest_component::<R>(host, host.manifest_name()) {
            Ok(comp) => self.inner.remove_resource(comp),
            Err(err) => err,
        }
    }

    /// Captures a raw command-buffer checkpoint token via `BamHostApi::cmd_checkpoint`.
    #[inline]
    pub fn checkpoint(&mut self) -> Result<usize, FfiResult> {
        self.inner.checkpoint()
    }

    /// Rolls back all deferred commands queued after `checkpoint` via `BamHostApi::cmd_rollback`.
    #[inline]
    pub fn rollback(&mut self, checkpoint: usize) -> FfiResult {
        self.inner.rollback(checkpoint)
    }

    /// Starts a scoped [`GuestTransaction`] capturing a checkpoint on this command buffer.
    ///
    /// If the returned [`GuestTransaction`] is dropped without calling
    /// [`GuestTransaction::commit`], all queued commands since `begin_transaction` are
    /// automatically rolled back via `BamHostApi::cmd_rollback`.
    #[inline]
    pub fn begin_transaction(&mut self) -> Result<GuestTransaction<'_>, FfiResult> {
        GuestTransaction::begin(self)
    }

    /// Executes `f` inside a scoped [`GuestTransaction`]: captures a checkpoint via
    /// `cmd_checkpoint` on entry, commits automatically when `f` returns `Ok(val)`, and rolls back
    /// automatically via `cmd_rollback` when `f` returns `Err(err)` or unwinds.
    pub fn transaction<R, F>(&mut self, f: F) -> Result<R, FfiResult>
    where
        F: FnOnce(&mut GuestTransaction<'_>) -> Result<R, FfiResult>,
    {
        let mut tx = self.begin_transaction()?;
        match f(&mut tx) {
            Ok(val) => {
                tx.commit();
                Ok(val)
            }
            Err(err) => {
                let _ = tx.rollback();
                Err(err)
            }
        }
    }
}

/// Scoped transactional deferred command buffer backed by `BamHostApi::cmd_checkpoint` and
/// `BamHostApi::cmd_rollback`.
///
/// Created via [`GuestCommands::begin_transaction`] or [`GuestCommands::transaction`]. Uncommitted
/// transactions automatically roll back when dropped (including on early `?` exits or panics),
/// while [`Self::commit`] keeps all queued commands and [`Self::rollback`] explicitly discards
/// them and returns the host's [`FfiResult`].
pub struct GuestTransaction<'a> {
    commands: GuestCommands<'a>,
    checkpoint: usize,
    settled: bool,
}

impl<'a> GuestTransaction<'a> {
    /// Begins a new transaction on `commands` by capturing a `cmd_checkpoint` token.
    #[inline]
    pub fn begin(commands: &'a mut GuestCommands<'_>) -> Result<Self, FfiResult> {
        let mut reborrowed = GuestCommands::from_validated(commands.inner.reborrow());
        let checkpoint = reborrowed.inner.checkpoint()?;
        Ok(Self {
            commands: reborrowed,
            checkpoint,
            settled: false,
        })
    }

    /// Returns the opaque checkpoint token captured when this transaction began.
    #[inline]
    pub const fn checkpoint_token(&self) -> usize {
        self.checkpoint
    }

    /// Borrows the underlying [`GuestCommands`] immutably.
    #[inline]
    pub const fn as_commands(&self) -> &GuestCommands<'a> {
        &self.commands
    }

    /// Borrows the underlying [`GuestCommands`] mutably.
    #[inline]
    pub fn as_commands_mut(&mut self) -> &mut GuestCommands<'a> {
        &mut self.commands
    }

    /// Commits all deferred commands queued within this transaction so dropping it will not roll
    /// them back.
    #[inline]
    pub fn commit(mut self) {
        self.settled = true;
    }

    /// Explicitly rolls back all deferred commands queued since this transaction began and marks
    /// the transaction settled so [`Drop`] does not invoke rollback a second time.
    #[inline]
    pub fn rollback(mut self) -> FfiResult {
        self.settled = true;
        self.commands.inner.rollback(self.checkpoint)
    }
}

impl<'a> Deref for GuestTransaction<'a> {
    type Target = GuestCommands<'a>;

    /// Dereferences to the transaction's underlying [`GuestCommands`] handle.
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.commands
    }
}

impl DerefMut for GuestTransaction<'_> {
    /// Mutably dereferences to the transaction's underlying [`GuestCommands`] handle so standard
    /// command methods enqueue inside the active transaction scope.
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.commands
    }
}

impl Drop for GuestTransaction<'_> {
    /// Automatically rolls back all commands queued since `begin` if neither [`Self::commit`] nor
    /// [`Self::rollback`] was called (including on early `?` returns or unwinds).
    #[inline]
    fn drop(&mut self) {
        if !self.settled {
            self.settled = true;
            let _ = self.commands.inner.rollback(self.checkpoint);
        }
    }
}

impl<'a> From<ValidatedCommands<'a>> for GuestCommands<'a> {
    /// Wraps a [`ValidatedCommands`] handle in a safe [`GuestCommands`] buffer.
    #[inline]
    fn from(inner: ValidatedCommands<'a>) -> Self {
        Self::from_validated(inner)
    }
}
