//! Safe `Has<T>` chunk accessors and `RemovedComponents<T>` batch cursor readers.

use bamabi_sys::{AbiPod, AccessMode, FfiEntity, FfiResult};

use crate::{
    app::SystemContext,
    component::{GuestComponent, guest_uuid_filter},
    iter::{Chunk, ChunkIter},
};

/// Verifies that `term_index` on `iter` is a `REMOVED_READ` entity component stream matching
/// `T`'s `(size, align)` and manifest-aware [`ComponentUuid`](bamabi_sys::ComponentUuid).
fn validate_removed_term<T: GuestComponent>(
    iter: &ChunkIter<'_>,
    term_index: usize,
) -> Result<(), FfiResult> {
    let info = iter.column_info(term_index)?;
    if info.mode != AccessMode::REMOVED_READ || info.is_resource() || info.is_message() {
        return Err(FfiResult::TYPE_MISMATCH);
    }
    if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
        return Err(FfiResult::TYPE_MISMATCH);
    }
    let filter = guest_uuid_filter::<T>(iter.host().manifest_name());
    if !filter.matches(info.uuid) {
        return Err(FfiResult::TYPE_MISMATCH);
    }
    Ok(())
}

impl<'iter, 'ctx> Chunk<'iter, 'ctx> {
    /// Returns the archetypal presence boolean (`Has<T>`) for `T: GuestComponent` at column
    /// `col` in the active chunk, validating `T`'s layout and UUID.
    ///
    /// Returns `None` on layout/UUID/mode mismatch or if no chunk is active.
    #[inline]
    pub fn has<T: GuestComponent>(&self, col: usize) -> Option<bool> {
        let info = self.iter.column_info(col).ok()?;
        if info.mode != AccessMode::HAS || info.is_resource() || info.is_message() {
            return None;
        }
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return None;
        }
        let filter = guest_uuid_filter::<T>(self.iter.host().manifest_name());
        if !filter.matches(info.uuid) {
            return None;
        }
        self.iter.has_column(col)
    }

    /// Returns the archetypal presence boolean (`Has<T>`) for `T: AbiPod` at column `col`,
    /// validating `(size, align, mode)`.
    #[inline]
    pub fn has_pod<T: AbiPod>(&self, col: usize) -> Option<bool> {
        let info = self.iter.column_info(col).ok()?;
        if info.mode != AccessMode::HAS || info.is_resource() || info.is_message() {
            return None;
        }
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return None;
        }
        self.iter.has_column(col)
    }

    /// Returns the archetypal presence boolean (`AccessMode::HAS`) at column `col` without
    /// checking a Rust type layout (useful for opaque/bridged host components).
    #[inline]
    pub fn has_untyped(&self, col: usize) -> Option<bool> {
        self.iter.has_column(col)
    }
}

impl<'a> ChunkIter<'a> {
    /// Reads up to `out_entities.len()` entity IDs from a `RemovedComponents<T>` stream declared
    /// with [`AccessMode::REMOVED_READ`] at `term_index`, advancing this system's persistent cursor.
    #[inline]
    pub fn read_removed_batch(
        &mut self,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        self.inner.read_removed_batch(term_index, out_entities)
    }

    /// Drains all unread `RemovedComponents<T>` entity IDs from `term_index` (`REMOVED_READ`).
    #[inline]
    pub fn drain_removed(&mut self, term_index: usize) -> Result<Vec<FfiEntity>, FfiResult> {
        self.inner.drain_removed(term_index)
    }

    /// Drains all unread `RemovedComponents<T>` entity IDs for `T: GuestComponent` at `term_index`,
    /// validating `T`'s layout and UUID.
    #[inline]
    pub fn read_removed<T: GuestComponent>(
        &mut self,
        term_index: usize,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        validate_removed_term::<T>(self, term_index)?;
        self.inner.drain_removed(term_index)
    }

    /// Reads up to `out_entities.len()` `RemovedComponents<T>` entity IDs for `T: GuestComponent`
    /// at `term_index`, validating `T`'s layout and UUID and advancing this system's cursor.
    #[inline]
    pub fn read_removed_batch_typed<T: GuestComponent>(
        &mut self,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        validate_removed_term::<T>(self, term_index)?;
        self.inner.read_removed_batch(term_index, out_entities)
    }
}

impl<'a> SystemContext<'a> {
    /// Drains all unread `RemovedComponents<T>` entity IDs declared at `term_index` either via
    /// [`SystemBuilder::removed`](crate::SystemBuilder::removed) (system-level bundle) or via
    /// [`QueryBuilder::removed`](crate::QueryBuilder::removed) on primary query `0`, validating
    /// `T`'s layout and UUID.
    #[inline]
    pub fn read_removed<T: GuestComponent>(
        &mut self,
        term_index: usize,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        let mut res_err = None;
        if let Some(res_q) = self.resource_query_mut() {
            match validate_removed_term::<T>(res_q, term_index) {
                Ok(()) => return res_q.drain_removed(term_index),
                Err(err) => res_err = Some(err),
            }
        }
        match self.query_mut(0) {
            Some(q0) => q0.read_removed::<T>(term_index),
            None => Err(res_err.unwrap_or(FfiResult::OUT_OF_RANGE)),
        }
    }

    /// Reads up to `out_entities.len()` `RemovedComponents<T>` entity IDs declared at
    /// `term_index` either via [`SystemBuilder::removed`](crate::SystemBuilder::removed) or on
    /// primary query `0`, advancing the persistent cursor.
    #[inline]
    pub fn read_removed_batch<T: GuestComponent>(
        &mut self,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        let mut res_err = None;
        if let Some(res_q) = self.resource_query_mut() {
            match validate_removed_term::<T>(res_q, term_index) {
                Ok(()) => return res_q.read_removed_batch(term_index, out_entities),
                Err(err) => res_err = Some(err),
            }
        }
        match self.query_mut(0) {
            Some(q0) => q0.read_removed_batch_typed::<T>(term_index, out_entities),
            None => Err(res_err.unwrap_or(FfiResult::OUT_OF_RANGE)),
        }
    }

    /// Drains all unread `RemovedComponents<T>` entity IDs declared at `(query_index, term_index)`,
    /// validating `T`'s layout and UUID.
    #[inline]
    pub fn query_read_removed<T: GuestComponent>(
        &mut self,
        query_index: usize,
        term_index: usize,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        self.query_mut(query_index)
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_removed::<T>(term_index)
    }

    /// Reads up to `out_entities.len()` `RemovedComponents<T>` entity IDs declared at
    /// `(query_index, term_index)`, advancing the persistent cursor.
    #[inline]
    pub fn query_read_removed_batch<T: GuestComponent>(
        &mut self,
        query_index: usize,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        self.query_mut(query_index)
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_removed_batch_typed::<T>(term_index, out_entities)
    }

    /// Untyped drain of unread `RemovedComponents` entity IDs at `(query_index, term_index)`.
    #[inline]
    pub fn read_removed_raw(
        &mut self,
        query_index: usize,
        term_index: usize,
    ) -> Result<Vec<FfiEntity>, FfiResult> {
        self.query_mut(query_index)
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_removed(term_index)
    }

    /// Untyped batch read of unread `RemovedComponents` entity IDs at `(query_index, term_index)`.
    #[inline]
    pub fn read_removed_batch_raw(
        &mut self,
        query_index: usize,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        self.query_mut(query_index)
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_removed_batch(term_index, out_entities)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        FfiChildOf,
        iter::{Added, Allow, Changed, Has, Or2, Or3, QueryTerm, QueryTuple, With, Without},
    };

    /// Compile-time trait-bound assertion helper for [`QueryTerm`].
    fn assert_term<T: QueryTerm>() {}
    /// Compile-time trait-bound assertion helper for [`QueryTuple`].
    fn assert_tuple<Q: QueryTuple>() {}

    /// Verifies that `Allow<T>`, `Has<T>`, `Or2<..>`, and `Or3<..>` filter wrappers satisfy
    /// [`QueryTerm`] and compose cleanly inside [`QueryTuple`] signatures.
    #[test]
    fn filter_query_terms_and_tuples_typecheck() {
        assert_term::<Allow<FfiChildOf>>();
        assert_term::<Has<FfiChildOf>>();
        assert_term::<Or2<With<FfiChildOf>, Without<FfiChildOf>>>();
        assert_term::<Or2<Changed<FfiChildOf>, Added<FfiChildOf>>>();
        assert_term::<Or3<With<FfiChildOf>, Without<FfiChildOf>, Changed<FfiChildOf>>>();

        assert_tuple::<Allow<FfiChildOf>>();
        assert_tuple::<Has<FfiChildOf>>();
        assert_tuple::<Or2<With<FfiChildOf>, Without<FfiChildOf>>>();
        assert_tuple::<Or3<With<FfiChildOf>, Changed<FfiChildOf>, Added<FfiChildOf>>>();
        assert_tuple::<(
            &FfiChildOf,
            Allow<FfiChildOf>,
            Has<FfiChildOf>,
            Or2<With<FfiChildOf>, Added<FfiChildOf>>,
            Or3<Without<FfiChildOf>, Changed<FfiChildOf>, Added<FfiChildOf>>,
        )>();
    }

    /// Verifies that custom `#[derive(AbiPod)]` components compose across `Allow`, `Has`, `Or2`,
    /// and `Or3` inside a multi-term [`QueryTuple`].
    #[cfg(feature = "derive")]
    #[test]
    fn derived_filter_query_tuples_typecheck() {
        use crate::prelude::*;

        /// Test `AbiPod` component fixture `A`.
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq, AbiPod)]
        struct DummyA(f32);
        impl GuestComponent for DummyA {
            const NAME: &'static core::ffi::CStr = c"filters_test::DummyA";
        }

        /// Test `AbiPod` component fixture `B`.
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq, AbiPod)]
        struct DummyB(u32);
        impl GuestComponent for DummyB {
            const NAME: &'static core::ffi::CStr = c"filters_test::DummyB";
        }

        /// Test `AbiPod` component fixture `C`.
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq, AbiPod)]
        struct DummyC(u32);
        impl GuestComponent for DummyC {
            const NAME: &'static core::ffi::CStr = c"filters_test::DummyC";
        }

        assert_tuple::<(
            &DummyA,
            Allow<DummyC>,
            Has<DummyB>,
            Or2<With<DummyA>, Added<DummyB>>,
            Or3<Without<DummyB>, Changed<DummyA>, Added<DummyC>>,
        )>();
    }
}
