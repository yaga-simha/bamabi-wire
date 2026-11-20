//! Zero-copy hierarchy (`Children`) traversal, child command helpers, and custom dynamic
//! `Relationship` / `RelationshipTarget` pair registration (`#![forbid(unsafe_code)]`).

use core::ffi::CStr;

use bamabi_guest_sys::{SpawnEntry, resolve_component};
use bamabi_sys::{
    AccessMode, BAM_CHILDREN_NAME, BamComponent, ComponentUuid, FfiChildOf, FfiEntity, FfiResult,
};

use crate::{
    app::builder::{GuestApp, QueryBuilder},
    app::context::SystemContext,
    commands::GuestCommands,
    component::GuestComponent,
};

const CHILDREN_NAME_CSTR: &CStr = c"bevy_ecs::hierarchy::Children";
const CHILDREN_ALIAS_CSTR: &CStr = c"Children";
const CHILD_OF_NAME_CSTR: &CStr = c"bevy_ecs::hierarchy::ChildOf";

impl<'a> SystemContext<'a> {
    /// Borrows the contiguous zero-copy `&[FfiEntity]` slice of Bevy `Children` attached to
    /// `entity`.
    ///
    /// Returns [`FfiResult::PERMISSION_DENIED`] unless a query in this system declared read access
    /// to `Children` (for example via [`QueryBuilder::read_children`]). Returns `Ok(&[])` when
    /// `entity` is alive and has no `Children` component.
    #[inline]
    pub fn children(&self, entity: FfiEntity) -> Result<&[FfiEntity], FfiResult> {
        self.sys_ref.entity_children(entity, BamComponent::INVALID)
    }

    /// Borrows the contiguous zero-copy `&[FfiEntity]` slice of incoming source entities recorded
    /// under `target_handle` (either a bridged `RelationshipTarget` or the target half of a
    /// dynamic pair registered via [`GuestApp::register_relationship_pair`]).
    ///
    /// Returns [`FfiResult::PERMISSION_DENIED`] unless a query in this system declared read access
    /// to `target_handle` (for example via [`QueryBuilder::read_handle`] or
    /// [`QueryBuilder::optional_read_handle`]).
    #[inline]
    pub fn relationship_targets(
        &self,
        entity: FfiEntity,
        target_handle: BamComponent,
    ) -> Result<&[FfiEntity], FfiResult> {
        if !target_handle.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        self.sys_ref.entity_children(entity, target_handle)
    }

    /// Collects all recursive descendants of `root` in breadth-first order via Bevy's `Children`
    /// relationship (mirroring `HierarchyQueryExt::iter_descendants`), guarding against cycles.
    ///
    /// Requires read access to `Children` (e.g. [`QueryBuilder::read_children`]). Excludes `root`
    /// itself from the returned `Vec`.
    pub fn iter_descendants(&self, root: FfiEntity) -> Result<Vec<FfiEntity>, FfiResult> {
        if root.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        let mut visited = std::collections::HashSet::new();
        visited.insert(root);
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(root);
        let mut descendants = Vec::new();
        while let Some(curr) = queue.pop_front() {
            for &child in self.children(curr)? {
                if !child.is_null() && visited.insert(child) {
                    descendants.push(child);
                    queue.push_back(child);
                }
            }
        }
        Ok(descendants)
    }

    /// Collects all recursive descendants of `root` in breadth-first order along a custom
    /// `RelationshipTarget` handle `target_handle`, guarding against cycles.
    pub fn iter_descendants_of(
        &self,
        root: FfiEntity,
        target_handle: BamComponent,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        if root.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        if !target_handle.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        let mut visited = std::collections::HashSet::new();
        visited.insert(root);
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(root);
        let mut descendants = Vec::new();
        while let Some(curr) = queue.pop_front() {
            for &child in self.relationship_targets(curr, target_handle)? {
                if !child.is_null() && visited.insert(child) {
                    descendants.push(child);
                    queue.push_back(child);
                }
            }
        }
        Ok(descendants)
    }
}

impl<'b> QueryBuilder<'b> {
    /// Resolves Bevy's built-in `Children` component (`"bevy_ecs::hierarchy::Children"` /
    /// `"Children"`) and appends an [`AccessMode::READ`] term so [`SystemContext::children`] has
    /// scheduler-verified shared read permission.
    pub fn read_children(mut self) -> Self {
        let uuid = ComponentUuid::from_canonical_name(BAM_CHILDREN_NAME);
        let resolved = match resolve_component(self.host, Some(CHILDREN_NAME_CSTR), Some(uuid)) {
            Ok(comp) => Ok(comp),
            Err(FfiResult::NOT_FOUND) => {
                resolve_component(self.host, Some(CHILDREN_ALIAS_CSTR), Some(uuid))
            }
            Err(err) => Err(err),
        };
        match resolved {
            Ok(comp) => self.push_term(comp, AccessMode::READ),
            Err(err) => self.record_error(err),
        }
        self
    }
}

impl<'a> GuestCommands<'a> {
    /// Attaches `FfiChildOf::new(parent)` onto `child`, adding `child` to `parent`'s `Children`
    /// slice when deferred commands flush.
    #[inline]
    pub fn add_child(&mut self, parent: FfiEntity, child: FfiEntity) -> FfiResult {
        if parent.is_null() || child.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        self.insert_component(child, &FfiChildOf::new(parent))
    }

    /// Removes `ChildOf` (`FfiChildOf`) from `child`, detaching `child` from its current parent's
    /// `Children` slice when deferred commands flush.
    #[inline]
    pub fn remove_parent(&mut self, child: FfiEntity) -> FfiResult {
        if child.is_null() {
            return FfiResult::INVALID_ENTITY;
        }
        self.remove_component::<FfiChildOf>(child)
    }

    /// Spawns a new entity carrying `FfiChildOf::new(parent)` together with `extra_entries` in a
    /// single batched archetype transition, returning the reserved child [`FfiEntity`].
    pub fn spawn_child(
        &mut self,
        parent: FfiEntity,
        extra_entries: &[SpawnEntry<'_>],
    ) -> Result<FfiEntity, FfiResult> {
        if !self.is_valid() {
            return Err(FfiResult::NULL_POINTER);
        }
        if parent.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        let child_of_handle =
            match self.resolve_component(Some(CHILD_OF_NAME_CSTR), Some(FfiChildOf::UUID)) {
                Ok(handle) => handle,
                Err(FfiResult::NOT_FOUND) => {
                    self.resolve_component(Some(c"ChildOf"), Some(FfiChildOf::UUID))?
                }
                Err(err) => return Err(err),
            };
        let child_of = FfiChildOf::new(parent);
        let mut entries = Vec::with_capacity(1 + extra_entries.len());
        entries.push(SpawnEntry::new(child_of_handle, &child_of));
        entries.extend_from_slice(extra_entries);
        self.spawn(&entries)
    }
}

impl<'a> GuestApp<'a> {
    /// Registers a dynamic `(Rel, Target)` relationship pair backed by host `on_insert`,
    /// `on_discard`, and `on_despawn` hooks, returning `(rel_handle, target_handle)`.
    ///
    /// - `Rel` must have `size == 8` and `align == 8` (storing an `FfiEntity` target handle, e.g.
    ///   `#[repr(C)] struct Orbiting(FfiEntity);`).
    /// - `target_name` is registered as a zero-sized marker component (manifest-qualified when
    ///   unqualified) so queries (`With`, `Has`, `Changed`, `read_handle`, `optional_read_handle`)
    ///   and [`SystemContext::relationship_targets`] operate natively under Bevy's scheduler.
    pub fn register_relationship_pair<Rel: GuestComponent>(
        &mut self,
        target_name: &str,
    ) -> Result<(BamComponent, BamComponent), FfiResult> {
        if core::mem::size_of::<Rel>() != core::mem::size_of::<FfiEntity>()
            || core::mem::align_of::<Rel>() != core::mem::align_of::<FfiEntity>()
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if target_name.is_empty() {
            return Err(FfiResult::INVALID_ARGUMENT);
        }
        let qualified_target =
            if !target_name.contains("::") && !self.instance.manifest_name().is_empty() {
                format!("{}::{target_name}", self.instance.manifest_name())
            } else {
                target_name.to_string()
            };
        let rel_handle = self.register_component::<Rel>()?;
        let target_handle = self.register_component_with_layout(&qualified_target, 0, 1, None)?;
        let bind_spec = format!("__bam_rel_bind::{}::{}", rel_handle.0, target_handle.0);
        let _ = self.register_component_with_layout(&bind_spec, 0, 1, None)?;
        Ok((rel_handle, target_handle))
    }
}

#[cfg(test)]
mod tests {
    use bamabi_guest_sys::SpawnEntry;
    use bamabi_sys::{BamComponent, FfiEntity, FfiResult};

    use crate::{GuestApp, GuestCommands, QueryBuilder, SystemContext};

    /// Compile-time signature fixture exercising all safe hierarchy and custom-relationship SDK
    /// methods under `#![forbid(unsafe_code)]`.
    #[allow(dead_code)]
    fn _verify_hierarchy_sdk_signatures<'a>(
        ctx: &'a SystemContext<'a>,
        qb: QueryBuilder<'a>,
        cmds: &'a mut GuestCommands<'a>,
        app: &'a mut GuestApp<'a>,
        entity: FfiEntity,
        target: BamComponent,
        entries: &[SpawnEntry<'_>],
    ) {
        let _: Result<&[FfiEntity], FfiResult> = ctx.children(entity);
        let _: Result<&[FfiEntity], FfiResult> = ctx.relationship_targets(entity, target);
        let _: QueryBuilder<'a> = qb.read_children();
        let _: FfiResult = cmds.add_child(entity, entity);
        let _: FfiResult = cmds.remove_parent(entity);
        let _: Result<FfiEntity, FfiResult> = cmds.spawn_child(entity, entries);
        let _: Result<(BamComponent, BamComponent), FfiResult> =
            app.register_relationship_pair::<bamabi_sys::FfiChildOf>("test::Targets");
    }

    /// Scenario: Verifies that all hierarchy traversal (`children`, `relationship_targets`),
    /// command (`add_child`, `remove_parent`, `spawn_child`), and relationship-pair registration
    /// signatures compile cleanly under `#![forbid(unsafe_code)]`.
    #[test]
    fn hierarchy_guest_sdk_signatures_compile_with_forbid_unsafe() {
        let _f = _verify_hierarchy_sdk_signatures;
    }
}
