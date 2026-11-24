//! Unified system execution context (`SystemContext`) and run-condition context (`RunConditionContext`).

use core::{
    ffi::{CStr, c_void},
    mem::MaybeUninit,
};

use bamabi_guest_sys::{
    DrainOptions, Host, LazyCommands, TypedComponent, ValidatedCommands, ValidatedIter,
    ValidatedSystemInvocation, ValidatedSystemRef, ValidatedSystemSlice, resolve_component,
};
use bamabi_sys::{
    AbiPod, AccessMode, BamChangeTicks, BamComponent, BamHostApi, BamSymbol, BamSystemCtx,
    BamTimeInfo, ComponentUuid, FfiEntity, FfiResult, MAX_TERMS_PER_QUERY, ModComponent,
};

use crate::{
    app::builder::IntoSystemResult,
    commands::GuestCommands,
    component::{
        GuestComponent, guest_uuid_filter, qualify_component_identity, resolve_guest_component,
    },
    iter::{Chunk, ChunkIter, QueryTuple, TypedChunkIter},
};

/// Type alias for a boxed ECS-aware `.run_if_with` / `EcsRunCondition` predicate.
pub(crate) type RunIfCtxPredicateFn =
    Box<dyn FnMut(&mut RunConditionSystemContext<'_>, usize) -> bool + Send + 'static>;

/// Safe context passed to `.run_if(...)` condition closures.
///
/// Note that Bevy evaluates run conditions before constructing a per-system `BamSystemCtx`, so
/// `RunConditionContext` exposes world-scoped [`Host`] diagnostics, handle resolution, and
/// `"bam.v0.*"` custom symbol queries rather than entity iterators or deferred commands.
pub struct RunConditionContext<'a> {
    host: &'a Host,
}

impl<'a> RunConditionContext<'a> {
    /// Wraps a validated world-scoped [`Host`] reference for a single run-condition evaluation.
    #[inline]
    pub(crate) const fn new(host: &'a Host) -> Self {
        Self { host }
    }

    /// Returns the cached [`Host`] wrapper for this mod instance.
    #[inline]
    pub const fn host(&self) -> &'a Host {
        self.host
    }

    /// Returns the validated [`BamHostApi`] function table.
    #[inline]
    pub const fn host_api(&self) -> &'a BamHostApi {
        self.host.api()
    }

    /// Reads the most recent human-readable UTF-8 diagnostic recorded on this world's host context.
    #[inline]
    pub fn last_error(&self) -> String {
        self.host.last_error()
    }

    /// Resolves a component or resource handle by `name`, `uuid`, or both.
    #[inline]
    pub fn resolve_component(
        &self,
        name: Option<&CStr>,
        uuid: Option<ComponentUuid>,
    ) -> Result<BamComponent, FfiResult> {
        resolve_component(self.host, name, uuid)
    }

    /// Resolves `T: GuestComponent` (including manifest-qualified names) on this world.
    #[inline]
    pub fn resolve_guest_component<T: GuestComponent>(&self) -> Result<BamComponent, FfiResult> {
        resolve_guest_component::<T>(self.host, self.host.manifest_name())
    }

    /// Resolves a custom `"bam.v0.<name>"` host symbol pointer.
    #[inline]
    pub fn resolve_custom_symbol(&self, symbol: &CStr) -> Result<*const c_void, FfiResult> {
        self.host.resolve_custom_symbol(symbol)
    }
}

/// Lightweight read-only ECS execution context passed to `.run_if_with(...)` and
/// `.run_if_condition(...)` predicates backed by `BamSystemDesc::run_if_ctx` and `run_if_query`.
///
/// Exposes read-only access to the single `run_if_query` declared for the run condition
/// (resources with change ticks, message streams, and read-only entity queries) alongside the
/// world-scoped [`Host`].
pub struct RunConditionSystemContext<'a> {
    host: &'a Host,
    query: Option<ChunkIter<'a>>,
}

impl<'a> RunConditionSystemContext<'a> {
    /// Builds a [`RunConditionSystemContext`] from an active [`Host`] and optional [`ChunkIter`].
    #[inline]
    pub(crate) fn from_parts(host: &'a Host, query: Option<ChunkIter<'a>>) -> Self {
        Self { host, query }
    }

    /// Builds a [`RunConditionSystemContext`] from a trampoline [`ValidatedSystemInvocation`],
    /// taking slot 0 (`run_if_query`) if present.
    #[inline]
    pub(crate) fn from_invocation(inv: &mut ValidatedSystemInvocation<'a>) -> Self {
        let host = inv.host();
        let query = inv.take_raw_query_slot(0).map(ChunkIter::from_validated);
        Self { host, query }
    }

    /// Returns the active [`Host`] table for this mod instance.
    #[inline]
    pub const fn host(&self) -> &'a Host {
        self.host
    }

    /// Returns the validated [`BamHostApi`] function table.
    #[inline]
    pub const fn host_api(&self) -> &'a BamHostApi {
        self.host.api()
    }

    /// Borrows the read-only [`ChunkIter`] backing `run_if_query`, if any terms were declared.
    #[inline]
    pub fn query(&mut self) -> Option<&mut ChunkIter<'a>> {
        self.query.as_mut()
    }

    /// Borrows singleton resource `R: GuestComponent` declared at `decl_index` in `run_if_query`,
    /// validating `R`'s UUID and layout.
    #[inline]
    pub fn res<R: GuestComponent>(&self, decl_index: usize) -> Option<&R> {
        self.query.as_ref()?.component_resource::<R>(decl_index)
    }

    /// Borrows singleton POD resource `R: AbiPod` declared at `decl_index` in `run_if_query`.
    #[inline]
    pub fn res_pod<R: AbiPod>(&self, decl_index: usize) -> Option<&R> {
        self.query.as_ref()?.resource::<R>(decl_index)
    }

    /// Borrows singleton resource `R: GuestComponent` wrapped with its [`BamChangeTicks`].
    #[inline]
    pub fn res_with_ticks<R: GuestComponent>(
        &self,
        decl_index: usize,
    ) -> Result<GuestRes<'_, R>, FfiResult> {
        let Some(query) = self.query.as_ref() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.component_resource_with_ticks::<R>(decl_index)
    }

    /// Returns `true` iff singleton resource `R: GuestComponent` at `decl_index` currently exists
    /// in the host `World`.
    #[inline]
    pub fn has_resource<R: GuestComponent>(&self, decl_index: usize) -> bool {
        self.res::<R>(decl_index).is_some()
    }

    /// Returns `true` iff singleton resource `R: GuestComponent` at `decl_index` exists and was
    /// changed since this run condition's last evaluation.
    #[inline]
    pub fn is_resource_changed<R: GuestComponent>(&self, decl_index: usize) -> bool {
        self.res_with_ticks::<R>(decl_index)
            .is_ok_and(|res| res.is_changed())
    }

    /// Returns `true` iff singleton resource `R: GuestComponent` at `decl_index` exists and was
    /// newly added since this run condition's last evaluation.
    #[inline]
    pub fn is_resource_added<R: GuestComponent>(&self, decl_index: usize) -> bool {
        self.res_with_ticks::<R>(decl_index)
            .is_ok_and(|res| res.is_added())
    }

    /// Drains unread messages (`M: GuestComponent`) from `decl_index` in `run_if_query`.
    #[inline]
    pub fn read_messages<M: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_messages::<M>(decl_index)
    }

    /// Drains unread POD messages (`M: AbiPod`) from `decl_index` in `run_if_query`.
    #[inline]
    pub fn read_pod_messages<M: AbiPod>(&mut self, decl_index: usize) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_pod_messages::<M>(decl_index)
    }

    /// Reads up to `out.len()` unread messages (`M: GuestComponent`) from `decl_index` in
    /// `run_if_query` directly into uninitialized slice `out` with zero heap allocations.
    #[inline]
    pub fn read_messages_into_uninit<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut [MaybeUninit<M>],
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_messages_into_uninit::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread POD messages (`M: AbiPod`) from `decl_index` in
    /// `run_if_query` directly into uninitialized slice `out` with zero heap allocations.
    #[inline]
    pub fn read_pod_messages_into_uninit<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut [MaybeUninit<M>],
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_pod_messages_into_uninit::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread messages (`M: GuestComponent`) from `decl_index` in
    /// `run_if_query` directly into `out` with zero heap allocations.
    #[inline]
    pub fn read_messages_into<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut [M],
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_messages_into::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread POD messages (`M: AbiPod`) from `decl_index` in
    /// `run_if_query` directly into `out` with zero heap allocations.
    #[inline]
    pub fn read_pod_messages_into<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut [M],
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_pod_messages_into::<M>(decl_index, out)
    }

    /// Appends up to `max_count` unread messages (`M: GuestComponent`) from `decl_index` in
    /// `run_if_query` directly into `out`.
    #[inline]
    pub fn read_messages_into_vec<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        max_count: usize,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_messages_into_vec::<M>(decl_index, out, max_count)
    }

    /// Appends up to `max_count` unread POD messages (`M: AbiPod`) from `decl_index` in
    /// `run_if_query` directly into `out`.
    #[inline]
    pub fn read_pod_messages_into_vec<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        max_count: usize,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_pod_messages_into_vec::<M>(decl_index, out, max_count)
    }

    /// Reads up to `max_count` unread messages (`M: GuestComponent`) from `decl_index` in
    /// `run_if_query`.
    #[inline]
    pub fn read_messages_batch<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_messages_batch::<M>(decl_index, max_count)
    }

    /// Reads up to `max_count` unread POD messages (`M: AbiPod`) from `decl_index` in
    /// `run_if_query`.
    #[inline]
    pub fn read_pod_messages_batch<M: AbiPod>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_pod_messages_batch::<M>(decl_index, max_count)
    }

    /// Drains all unread messages (`M: GuestComponent`) from `decl_index` in `run_if_query`
    /// using custom [`DrainOptions`].
    #[inline]
    pub fn drain_messages_with_options<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_messages_with_options::<M>(decl_index, opts)
    }

    /// Drains all unread POD messages (`M: AbiPod`) from `decl_index` in `run_if_query` using
    /// custom [`DrainOptions`].
    #[inline]
    pub fn drain_pod_messages_with_options<M: AbiPod>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_pod_messages_with_options::<M>(decl_index, opts)
    }

    /// Drains all unread messages (`M: GuestComponent`) from `decl_index` in `run_if_query`
    /// directly into `out`, returning the number of appended messages.
    #[inline]
    pub fn drain_messages_into_vec<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        opts: DrainOptions,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_messages_into_vec::<M>(decl_index, out, opts)
    }

    /// Drains all unread POD messages (`M: AbiPod`) from `decl_index` in `run_if_query`
    /// directly into `out`, returning the number of appended messages.
    #[inline]
    pub fn drain_pod_messages_into_vec<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        opts: DrainOptions,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_pod_messages_into_vec::<M>(decl_index, out, opts)
    }

    /// Drains the unread message cursor (`M: GuestComponent`) at `decl_index` in `run_if_query`
    /// with zero heap allocations, returning the drained message count.
    #[inline]
    pub fn drain_messages_count<M: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_messages_count::<M>(decl_index)
    }

    /// Drains the unread POD message cursor (`M: AbiPod`) at `decl_index` in `run_if_query` with
    /// zero heap allocations, returning the drained message count.
    #[inline]
    pub fn drain_pod_messages_count<M: AbiPod>(
        &mut self,
        decl_index: usize,
    ) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_pod_messages_count::<M>(decl_index)
    }

    /// Drains unread VTable-backed non-POD messages (`M: ModComponent + Default`) from
    /// `decl_index` in `run_if_query`.
    #[inline]
    pub fn read_non_pod_messages<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_non_pod_messages::<M>(decl_index)
    }

    /// Reads up to `max_count` unread VTable-backed non-POD messages (`M: ModComponent + Default`)
    /// from `decl_index` in `run_if_query`.
    #[inline]
    pub fn read_non_pod_messages_batch<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.read_non_pod_messages_batch::<M>(decl_index, max_count)
    }

    /// Drains unread VTable-backed non-POD messages (`M: ModComponent + Default`) from
    /// `decl_index` in `run_if_query` using custom [`DrainOptions`].
    #[inline]
    pub fn drain_non_pod_messages_with_options<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Err(FfiResult::NOT_FOUND);
        };
        query.drain_non_pod_messages_with_options::<M>(decl_index, opts)
    }

    /// Returns `true` iff at least one unread message `M: GuestComponent` is queued at
    /// `decl_index` in `run_if_query`, draining the cursor completely with zero heap allocations.
    #[inline]
    pub fn has_messages<M: GuestComponent>(&mut self, decl_index: usize) -> bool {
        self.query
            .as_mut()
            .is_some_and(|q| q.drain_messages_count::<M>(decl_index).is_ok_and(|n| n > 0))
    }

    /// Returns `true` iff at least one unread POD message `M: AbiPod` is queued at `decl_index`
    /// in `run_if_query`, draining the cursor completely with zero heap allocations.
    #[inline]
    pub fn has_pod_messages<M: AbiPod>(&mut self, decl_index: usize) -> bool {
        self.query.as_mut().is_some_and(|q| {
            q.drain_pod_messages_count::<M>(decl_index)
                .is_ok_and(|n| n > 0)
        })
    }

    /// Non-destructively counts matching entities in `run_if_query` via `iter_count` (slot 57).
    #[inline]
    pub fn query_count(&mut self) -> Result<usize, FfiResult> {
        let Some(query) = self.query.as_mut() else {
            return Ok(0);
        };
        query.count()
    }

    /// Non-destructively returns `true` iff zero entities match `run_if_query`.
    #[inline]
    pub fn query_is_empty(&mut self) -> Result<bool, FfiResult> {
        Ok(self.query_count()? == 0)
    }
}

/// Unified per-tick execution context passed to every guest system built with
/// [`SystemBuilder::build`](crate::SystemBuilder::build).
///
/// Supports zero-query, single-query (`iter`, `iter_and_commands`), multi-query (`query_mut`,
/// `queries_2_mut`, `queries_2_and_commands`), declaration-indexed singleton resources
/// (`resource`, `resource_mut`), and handle/batched [`GuestCommands`].
pub struct SystemContext<'a> {
    pub(crate) sys_ref: ValidatedSystemRef<'a>,
    user_query_count: usize,
    queries: &'a mut [Option<ChunkIter<'a>>],
    resource_query_index: Option<usize>,
    empty_iter: Option<ChunkIter<'a>>,
    commands: Option<GuestCommands<'a>>,
    lazy_commands: Option<LazyCommands<'a>>,
}

/// Disjoint split view returned by [`SystemContext::split`] enabling simultaneous mutable access
/// to declared user queries, the resource/message stream iterator, [`GuestCommands`], and
/// [`ValidatedSystemRef`] without borrow-checker conflicts.
pub struct SplitSystemContext<'b, 'a> {
    /// Mutable slice of declared user queries `0..query_count`.
    pub queries: &'b mut [Option<ChunkIter<'a>>],
    /// Optional resource/message/removed-stream iterator slot (disjoint from `queries`).
    pub resource_iter: Option<&'b mut ChunkIter<'a>>,
    /// Deferred [`GuestCommands`] buffer (disjoint from `queries` and `sys`).
    pub commands: &'b mut GuestCommands<'a>,
    /// Shared [`ValidatedSystemRef`] for host diagnostics, ticks, time, symbols, and relations.
    pub sys: &'b ValidatedSystemRef<'a>,
}

/// Lazily resolves `commands` from `lazy_commands` on first access so systems that never touch
/// deferred commands avoid the `BamHostApi::sys_commands` FFI call.
#[inline]
fn resolve_commands_slot<'b, 'a>(
    commands: &'b mut Option<GuestCommands<'a>>,
    lazy_commands: &mut Option<LazyCommands<'a>>,
    base_host: &'a Host,
) -> &'b mut GuestCommands<'a> {
    commands.get_or_insert_with(|| {
        let validated = match lazy_commands.take() {
            Some(lazy) => lazy.resolve(),
            None => ValidatedCommands::empty(base_host),
        };
        GuestCommands::from_validated(validated)
    })
}

/// Splits two distinct mutable query slots (`idx_a != idx_b`) out of `queries`.
#[inline]
fn split_queries_2_mut<'b, 'a>(
    queries: &'b mut [Option<ChunkIter<'a>>],
    user_query_count: usize,
    idx_a: usize,
    idx_b: usize,
) -> Option<(&'b mut ChunkIter<'a>, &'b mut ChunkIter<'a>)> {
    if idx_a == idx_b
        || idx_a >= user_query_count
        || idx_b >= user_query_count
        || idx_a >= queries.len()
        || idx_b >= queries.len()
    {
        return None;
    }
    if idx_a < idx_b {
        let (left, right) = queries.split_at_mut(idx_b);
        Some((left[idx_a].as_mut()?, right[0].as_mut()?))
    } else {
        let (left, right) = queries.split_at_mut(idx_a);
        Some((right[0].as_mut()?, left[idx_b].as_mut()?))
    }
}

/// Borrowed triple of three distinct mutable [`ChunkIter`] slots.
type QueryTripleMut<'b, 'a> = (
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
);

/// Borrowed triple of three distinct mutable [`ChunkIter`] slots paired with [`GuestCommands`].
pub type QueryTripleAndCommands<'b, 'a> = (
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
    &'b mut GuestCommands<'a>,
);

/// Borrowed triple of three distinct typed [`TypedChunkIter`] slots.
pub type TypedQueryTriple<'b, 'a, Q1, Q2, Q3> = (
    TypedChunkIter<'b, 'a, Q1>,
    TypedChunkIter<'b, 'a, Q2>,
    TypedChunkIter<'b, 'a, Q3>,
);

/// Splits three distinct mutable query slots (`idx_a`, `idx_b`, `idx_c`) out of `queries`.
fn split_queries_3_mut<'b, 'a>(
    queries: &'b mut [Option<ChunkIter<'a>>],
    user_query_count: usize,
    idx_a: usize,
    idx_b: usize,
    idx_c: usize,
) -> Option<QueryTripleMut<'b, 'a>> {
    if idx_a == idx_b
        || idx_b == idx_c
        || idx_a == idx_c
        || idx_a >= user_query_count
        || idx_b >= user_query_count
        || idx_c >= user_query_count
        || idx_a >= queries.len()
        || idx_b >= queries.len()
        || idx_c >= queries.len()
    {
        return None;
    }
    let mut order = [(idx_a, 0u8), (idx_b, 1u8), (idx_c, 2u8)];
    order.sort_unstable_by_key(|&(idx, _)| idx);
    let (i0, p0) = order[0];
    let (i1, p1) = order[1];
    let (i2, p2) = order[2];
    let (s0, rest1) = queries.split_at_mut(i1);
    let (s1, s2) = rest1.split_at_mut(i2 - i1);
    let (r0, r1, r2) = (s0[i0].as_mut()?, s1[0].as_mut()?, s2[0].as_mut()?);
    let ordered = match (p0, p1, p2) {
        (0, 1, 2) => (r0, r1, r2),
        (0, 2, 1) => (r0, r2, r1),
        (1, 0, 2) => (r1, r0, r2),
        (1, 2, 0) => (r2, r0, r1),
        (2, 0, 1) => (r1, r2, r0),
        (2, 1, 0) => (r2, r1, r0),
        _ => return None,
    };
    Some(ordered)
}

/// Safe read-only singleton resource guard carrying Bevy change-detection [`BamChangeTicks`].
pub struct GuestRes<'a, R> {
    value: &'a R,
    ticks: BamChangeTicks,
}

impl<'a, R> GuestRes<'a, R> {
    /// Wraps a validated resource reference and its Bevy change-detection ticks.
    #[inline]
    pub const fn new(value: &'a R, ticks: BamChangeTicks) -> Self {
        Self { value, ticks }
    }

    /// Returns raw [`BamChangeTicks`] (`added`, `changed`, `last_run`, `this_run`).
    #[inline]
    pub const fn ticks(&self) -> BamChangeTicks {
        self.ticks
    }

    /// Returns `true` if the resource was added since this system last ran.
    #[inline]
    pub const fn is_added(&self) -> bool {
        self.ticks.is_added()
    }

    /// Returns `true` if the resource was added or changed since this system last ran.
    #[inline]
    pub const fn is_changed(&self) -> bool {
        self.ticks.is_changed()
    }
}

impl<R> core::ops::Deref for GuestRes<'_, R> {
    type Target = R;

    /// Borrows the underlying singleton resource value `&R`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

/// Safe exclusive mutable singleton resource guard carrying prior [`BamChangeTicks`].
pub struct GuestResMut<'a, R> {
    value: &'a mut R,
    ticks: BamChangeTicks,
}

impl<'a, R> GuestResMut<'a, R> {
    /// Wraps a validated mutable resource reference and its prior Bevy change-detection ticks.
    #[inline]
    pub const fn new(value: &'a mut R, ticks: BamChangeTicks) -> Self {
        Self { value, ticks }
    }

    /// Returns raw [`BamChangeTicks`] captured before this mutable borrow stamped `changed`.
    #[inline]
    pub const fn ticks(&self) -> BamChangeTicks {
        self.ticks
    }

    /// Returns `true` if the resource was added since this system last ran.
    #[inline]
    pub const fn is_added(&self) -> bool {
        self.ticks.is_added()
    }

    /// Returns `true` if the resource was changed by an earlier system since this system last ran.
    #[inline]
    pub const fn is_changed(&self) -> bool {
        self.ticks.is_changed()
    }
}

impl<R> core::ops::Deref for GuestResMut<'_, R> {
    type Target = R;

    /// Borrows the underlying singleton resource value `&R`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

impl<R> core::ops::DerefMut for GuestResMut<'_, R> {
    /// Mutably borrows the underlying singleton resource value `&mut R`.
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

impl<'a> SystemContext<'a> {
    /// Constructs a slice-backed [`SystemContext`] from a bucketed [`ValidatedSystemSlice`] and
    /// caller-owned stack slot slice `queries`.
    #[inline(always)]
    pub(crate) fn from_slice_invocation<'b>(
        inv: &mut ValidatedSystemSlice<'b>,
        queries: &'a mut [Option<ChunkIter<'a>>],
    ) -> Self
    where
        'b: 'a,
    {
        let base_host = inv.base_host();
        let sys_ref: ValidatedSystemRef<'a> = inv
            .take_system_ref()
            .unwrap_or_else(|| ValidatedSystemRef::empty(base_host));
        let slot_cap = queries.len();
        let user_query_count = inv.query_count().min(slot_cap);
        let mut idx = 0usize;
        while idx < user_query_count {
            queries[idx] = inv.take_raw_query_slot(idx).map(ChunkIter::from_validated);
            idx += 1;
        }
        let resource_query_index = match inv.resource_query_index() {
            Some(res_idx) if res_idx < slot_cap => {
                queries[res_idx] = inv
                    .take_raw_query_slot(res_idx)
                    .map(ChunkIter::from_validated);
                Some(res_idx)
            }
            _ => None,
        };
        let lazy_commands: Option<LazyCommands<'a>> = Some(inv.take_lazy_commands());
        Self {
            sys_ref,
            user_query_count,
            queries,
            resource_query_index,
            empty_iter: None,
            commands: None,
            lazy_commands,
        }
    }

    /// Borrows query slot `index` immutably.
    #[inline]
    fn query_ref(&self, index: usize) -> Option<&ChunkIter<'a>> {
        if index >= self.user_query_count {
            return None;
        }
        self.queries.get(index)?.as_ref()
    }

    /// Evaluates trailing `run_if_with` / `run_if_condition` predicates attached to a context-aware
    /// observer using the hidden trailing query slot `cond_query_index` and hides that trailing
    /// slot from `self.query_count()` for the subsequent user observer callback.
    #[inline]
    pub(crate) fn eval_observer_run_if_with(
        &mut self,
        cond_query_index: usize,
        preds: &mut [(usize, RunIfCtxPredicateFn)],
    ) -> bool {
        let host = self.sys_ref.host();
        let cond_iter = self
            .queries
            .get_mut(cond_query_index)
            .and_then(Option::take);
        if cond_query_index < self.user_query_count {
            self.user_query_count = cond_query_index;
        }
        let mut cond_ctx = RunConditionSystemContext::from_parts(host, cond_iter);
        preds
            .iter_mut()
            .all(|(offset, pred)| pred(&mut cond_ctx, *offset))
    }

    /// Borrows query slot `index` mutably.
    #[inline]
    fn query_slot_mut(&mut self, index: usize) -> Option<&mut ChunkIter<'a>> {
        if index >= self.user_query_count {
            return None;
        }
        self.queries.get_mut(index)?.as_mut()
    }

    /// Returns the active [`Host`] table for this mod instance, lazily resolving
    /// `BamHostApi::sys_host_ctx` on first access.
    #[inline]
    pub fn host(&self) -> &'a Host {
        self.sys_ref.host()
    }

    /// Returns the validated [`BamHostApi`] function table.
    #[inline]
    pub const fn host_api(&self) -> &'a BamHostApi {
        self.sys_ref.base_host().api()
    }

    /// Returns the raw `*mut BamSystemCtx` pointer for low-level FFI helpers.
    #[inline]
    pub const fn as_raw_ptr(&self) -> *mut BamSystemCtx {
        self.sys_ref.as_raw_sys_ctx()
    }

    /// Reads the most recent human-readable UTF-8 diagnostic recorded on this world's host context.
    #[inline]
    pub fn last_error(&self) -> String {
        self.sys_ref.last_error()
    }

    /// Returns the number of explicit `.query(...)` clauses declared on this system.
    #[inline]
    pub const fn query_count(&self) -> usize {
        self.user_query_count
    }

    /// Borrows primary query `0` mutably (yielding an exhausted empty iterator if no `.query(...)`
    /// clause was declared).
    #[inline]
    pub fn iter(&mut self) -> &mut ChunkIter<'a> {
        if self.user_query_count > 0 {
            if let Some(Some(q0)) = self.queries.get_mut(0) {
                return q0;
            }
        }
        let base_host = self.sys_ref.base_host();
        self.empty_iter
            .get_or_insert_with(|| ChunkIter::from_validated(ValidatedIter::empty(base_host)))
    }

    /// Borrows declared query `index` (`0..query_count()`) mutably (`None` if out of range).
    #[inline]
    pub fn query(&mut self, index: usize) -> Option<&mut ChunkIter<'a>> {
        self.query_mut(index)
    }

    /// Borrows declared query `index` (`0..query_count()`) mutably (`None` if out of range).
    #[inline]
    pub fn query_mut(&mut self, index: usize) -> Option<&mut ChunkIter<'a>> {
        self.query_slot_mut(index)
    }

    /// Simultaneously borrows two distinct queries `idx_a` and `idx_b` (`None` if `idx_a == idx_b`
    /// or either index is out of range).
    pub fn queries_2_mut(
        &mut self,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(&mut ChunkIter<'a>, &mut ChunkIter<'a>)> {
        split_queries_2_mut(self.queries, self.user_query_count, idx_a, idx_b)
    }

    /// Simultaneously borrows three distinct queries `idx_a`, `idx_b`, and `idx_c` (`None` if any
    /// two indices are equal or any index is out of range).
    pub fn queries_3_mut(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<(&mut ChunkIter<'a>, &mut ChunkIter<'a>, &mut ChunkIter<'a>)> {
        split_queries_3_mut(self.queries, self.user_query_count, idx_a, idx_b, idx_c)
    }

    /// Borrows the deferred [`GuestCommands`] buffer mutably.
    #[inline]
    pub fn commands(&mut self) -> &mut GuestCommands<'a> {
        resolve_commands_slot(
            &mut self.commands,
            &mut self.lazy_commands,
            self.sys_ref.base_host(),
        )
    }

    /// Simultaneously borrows primary query `0` and [`GuestCommands`].
    #[inline]
    pub fn iter_and_commands(&mut self) -> (&mut ChunkIter<'a>, &mut GuestCommands<'a>) {
        let base_host = self.sys_ref.base_host();
        let cmds = resolve_commands_slot(&mut self.commands, &mut self.lazy_commands, base_host);
        let iter = if self.user_query_count > 0 {
            if let Some(Some(q0)) = self.queries.get_mut(0) {
                q0
            } else {
                self.empty_iter.get_or_insert_with(|| {
                    ChunkIter::from_validated(ValidatedIter::empty(base_host))
                })
            }
        } else {
            self.empty_iter
                .get_or_insert_with(|| ChunkIter::from_validated(ValidatedIter::empty(base_host)))
        };
        (iter, cmds)
    }

    /// Simultaneously borrows two distinct queries `idx_a` and `idx_b` alongside [`GuestCommands`].
    pub fn queries_2_and_commands(
        &mut self,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(
        &mut ChunkIter<'a>,
        &mut ChunkIter<'a>,
        &mut GuestCommands<'a>,
    )> {
        let (a, b) = split_queries_2_mut(self.queries, self.user_query_count, idx_a, idx_b)?;
        let cmds = resolve_commands_slot(
            &mut self.commands,
            &mut self.lazy_commands,
            self.sys_ref.base_host(),
        );
        Some((a, b, cmds))
    }

    /// Simultaneously borrows three distinct queries `idx_a`, `idx_b`, and `idx_c` alongside
    /// [`GuestCommands`].
    pub fn queries_3_and_commands(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<QueryTripleAndCommands<'_, 'a>> {
        let (a, b, c) =
            split_queries_3_mut(self.queries, self.user_query_count, idx_a, idx_b, idx_c)?;
        let cmds = resolve_commands_slot(
            &mut self.commands,
            &mut self.lazy_commands,
            self.sys_ref.base_host(),
        );
        Some((a, b, c, cmds))
    }

    /// Mutably borrows the dedicated system-level resource query iterator, if one was registered.
    #[inline]
    pub(crate) fn resource_query_mut(&mut self) -> Option<&mut ChunkIter<'a>> {
        let idx = self.resource_query_index?;
        self.queries.get_mut(idx)?.as_mut()
    }

    /// Returns the iterator holding system-level resources/messages (preferring the dedicated
    /// `resource_query` slot, or falling back to primary query `0`).
    #[inline]
    fn resource_iter(&self) -> Option<&ChunkIter<'a>> {
        let idx = match self.resource_query_index {
            Some(idx) => idx,
            None => {
                if self.user_query_count > 0 {
                    0
                } else {
                    return None;
                }
            }
        };
        self.queries.get(idx)?.as_ref()
    }

    /// Mutable variant of [`Self::resource_iter`] for exclusive resource and message-writer access.
    #[inline]
    fn resource_iter_mut(&mut self) -> Option<&mut ChunkIter<'a>> {
        let idx = match self.resource_query_index {
            Some(idx) => idx,
            None => {
                if self.user_query_count > 0 {
                    0
                } else {
                    return None;
                }
            }
        };
        self.queries.get_mut(idx)?.as_mut()
    }

    /// Borrows singleton resource `R` (`R: AbiPod`) by declaration index `decl_index`.
    ///
    /// Routes to the system-level resource bundle when `.resource_read` / `.resource_write` was
    /// called on [`SystemBuilder`](crate::SystemBuilder), or to query `0` when resources were
    /// declared inside `.query(|q| ...)`.
    #[inline]
    pub fn resource<R: AbiPod>(&self, decl_index: usize) -> Option<&R> {
        self.resource_iter()?.resource::<R>(decl_index)
    }

    /// Borrows singleton resource `R` (`R: GuestComponent`) by declaration index `decl_index`,
    /// validating `R::UUID`.
    #[inline]
    pub fn component_resource<R: GuestComponent>(&self, decl_index: usize) -> Option<&R> {
        self.resource_iter()?.component_resource::<R>(decl_index)
    }

    /// Mutably borrows singleton resource `R` (`R: AbiPod`) declared with `RESOURCE_WRITE` at
    /// `decl_index`.
    #[inline]
    pub fn resource_mut<R: AbiPod>(&mut self, decl_index: usize) -> Option<&mut R> {
        self.resource_iter_mut()?.resource_mut::<R>(decl_index)
    }

    /// Mutably borrows singleton resource `R` (`R: GuestComponent`) at `decl_index`, validating
    /// `R::UUID`.
    #[inline]
    pub fn component_resource_mut<R: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Option<&mut R> {
        self.resource_iter_mut()?
            .component_resource_mut::<R>(decl_index)
    }

    /// Looks up a shared singleton resource `R` wrapped in [`GuestRes`] by pre-resolved
    /// [`BamComponent`] handle (`sys_res_ptr`), returning `None` if undeclared or absent.
    #[inline]
    pub fn res_by_handle<R: AbiPod>(&self, handle: BamComponent) -> Option<GuestRes<'_, R>> {
        self.sys_ref
            .res_ptr_by_handle_checked::<R>(handle)
            .map(|(val, ticks)| GuestRes::new(val, ticks))
    }

    /// Looks up an exclusive mutable singleton resource `R` wrapped in [`GuestResMut`] by
    /// pre-resolved [`BamComponent`] handle (`sys_res_mut_ptr`), returning `None` if undeclared
    /// with `ResourceWrite`, immutable, or absent.
    #[inline]
    pub fn res_mut_by_handle<R: AbiPod>(
        &mut self,
        handle: BamComponent,
    ) -> Option<GuestResMut<'_, R>> {
        self.sys_ref
            .res_mut_ptr_by_handle_checked::<R>(handle)
            .map(|(val, ticks)| GuestResMut::new(val, ticks))
    }

    /// Looks up a shared singleton resource `R` wrapped in [`GuestRes`] by an init-validated
    /// [`TypedComponent<R>`] handle, returning `None` if undeclared or absent.
    #[inline]
    pub fn res_typed_by_handle<R: AbiPod>(
        &self,
        handle: TypedComponent<R>,
    ) -> Option<GuestRes<'_, R>> {
        self.res_by_handle(handle.handle())
    }

    /// Looks up an exclusive mutable singleton resource `R` wrapped in [`GuestResMut`] by an
    /// init-validated [`TypedComponent<R>`] handle, returning `None` if undeclared with
    /// `ResourceWrite`, immutable, or absent.
    #[inline]
    pub fn res_mut_typed_by_handle<R: AbiPod>(
        &mut self,
        handle: TypedComponent<R>,
    ) -> Option<GuestResMut<'_, R>> {
        self.res_mut_by_handle(handle.handle())
    }

    /// Reads Bevy change-detection ticks for the singleton resource declared at `decl_index`.
    #[inline]
    pub fn resource_ticks(&self, decl_index: usize) -> Result<BamChangeTicks, FfiResult> {
        self.resource_iter()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .resource_ticks(decl_index)
    }

    /// Borrows singleton resource `R` (`R: GuestComponent`) with [`BamChangeTicks`] at
    /// `decl_index`.
    #[inline]
    pub fn res_with_ticks<R: GuestComponent>(
        &self,
        decl_index: usize,
    ) -> Result<GuestRes<'_, R>, FfiResult> {
        self.resource_iter()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .component_resource_with_ticks::<R>(decl_index)
    }

    /// Mutably borrows singleton resource `R` (`R: GuestComponent`) with prior [`BamChangeTicks`]
    /// at `decl_index`.
    #[inline]
    pub fn res_mut_with_ticks<R: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<GuestResMut<'_, R>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .component_resource_mut_with_ticks::<R>(decl_index)
    }

    /// Processes primary query `0` chunks in parallel across the host `ComputeTaskPool`.
    #[inline]
    pub fn par_chunks<F>(&mut self, f: F) -> FfiResult
    where
        F: Fn(Chunk<'_, '_>) + Sync,
    {
        self.iter().par_for_each(f)
    }

    /// Fallible variant of [`Self::par_chunks`] propagating non-`OK` [`IntoSystemResult`] returns.
    #[inline]
    pub fn par_try_chunks<F, R>(&mut self, f: F) -> FfiResult
    where
        F: Fn(Chunk<'_, '_>) -> R + Sync,
        R: IntoSystemResult,
    {
        self.iter().par_try_for_each(f)
    }

    /// Borrows primary query `0` as a [`TypedChunkIter`] for [`QueryTuple`] `Q` without magic
    /// column index literals.
    #[inline]
    pub fn single_query<Q: QueryTuple>(&mut self) -> TypedChunkIter<'_, 'a, Q> {
        self.iter().typed::<Q>()
    }

    /// Simultaneously borrows primary query `0` as a [`TypedChunkIter`] alongside [`GuestCommands`].
    #[inline]
    pub fn single_query_and_commands<Q: QueryTuple>(
        &mut self,
    ) -> (TypedChunkIter<'_, 'a, Q>, &mut GuestCommands<'a>) {
        let (iter, cmds) = self.iter_and_commands();
        (iter.typed::<Q>(), cmds)
    }

    /// Borrows declared query `index` (`0..query_count()`) as a [`TypedChunkIter`] for [`QueryTuple`] `Q`.
    #[inline]
    pub fn query_typed<Q: QueryTuple>(
        &mut self,
        index: usize,
    ) -> Option<TypedChunkIter<'_, 'a, Q>> {
        Some(self.query_mut(index)?.typed::<Q>())
    }

    /// Simultaneously borrows two distinct queries as [`TypedChunkIter`]s.
    #[inline]
    pub fn queries_2_typed<Q1: QueryTuple, Q2: QueryTuple>(
        &mut self,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(TypedChunkIter<'_, 'a, Q1>, TypedChunkIter<'_, 'a, Q2>)> {
        let (qa, qb) = self.queries_2_mut(idx_a, idx_b)?;
        Some((qa.typed::<Q1>(), qb.typed::<Q2>()))
    }

    /// Simultaneously borrows three distinct queries as [`TypedChunkIter`]s.
    #[inline]
    pub fn queries_3_typed<Q1: QueryTuple, Q2: QueryTuple, Q3: QueryTuple>(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<TypedQueryTriple<'_, 'a, Q1, Q2, Q3>> {
        let (qa, qb, qc) = self.queries_3_mut(idx_a, idx_b, idx_c)?;
        Some((qa.typed::<Q1>(), qb.typed::<Q2>(), qc.typed::<Q3>()))
    }

    /// Looks up a shared `&T` (`T: GuestComponent`) on `entity` using declared access on
    /// `(query_index, term_index)`, verifying `T`'s UUID.
    #[inline]
    pub fn get<T: GuestComponent>(
        &self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&T> {
        let q = self.query_ref(query_index)?;
        let filter = guest_uuid_filter::<T>(self.sys_ref.base_host().manifest_name());
        self.sys_ref.entity_get_checked::<T>(
            &q.inner,
            query_index,
            term_index,
            entity,
            Some(filter),
        )
    }

    /// Looks up an exclusive `&mut T` (`T: GuestComponent`) on `entity` using declared writable
    /// access on `(query_index, term_index)`, marking Bevy change ticks and verifying `T`'s UUID.
    #[inline]
    pub fn get_mut<T: GuestComponent>(
        &mut self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&mut T> {
        let filter = guest_uuid_filter::<T>(self.sys_ref.base_host().manifest_name());
        if query_index >= self.user_query_count {
            return None;
        }
        let q = self.queries.get_mut(query_index)?.as_mut()?;
        self.sys_ref.entity_get_mut_checked::<T>(
            &mut q.inner,
            query_index,
            term_index,
            entity,
            Some(filter),
        )
    }

    /// Looks up a shared `&T` (`T: AbiPod`) on `entity` using declared access on
    /// `(query_index, term_index)`.
    #[inline]
    pub fn get_pod<T: AbiPod>(
        &self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&T> {
        let q = self.query_ref(query_index)?;
        self.sys_ref
            .entity_get_checked::<T>(&q.inner, query_index, term_index, entity, None)
    }

    /// Looks up an exclusive `&mut T` (`T: AbiPod`) on `entity` using declared writable access on
    /// `(query_index, term_index)`.
    #[inline]
    pub fn get_pod_mut<T: AbiPod>(
        &mut self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&mut T> {
        if query_index >= self.user_query_count {
            return None;
        }
        let q = self.queries.get_mut(query_index)?.as_mut()?;
        self.sys_ref.entity_get_mut_checked::<T>(
            &mut q.inner,
            query_index,
            term_index,
            entity,
            None,
        )
    }

    /// Simultaneously borrows `N` disjoint mutable references `[&mut T; N]` (`T: GuestComponent`)
    /// for `entities` using declared writable access on `(query_index, term_index)`, mirroring
    /// Bevy's `Query::get_many_mut`.
    ///
    /// Rejects null entities (`INVALID_ENTITY`), duplicate entities or overlapping host pointers
    /// (`ACCESS_CONFLICT`), out-of-range query indices (`OUT_OF_RANGE`), missing entities
    /// (`NOT_FOUND`), layout/UUID mismatches (`TYPE_MISMATCH`), and non-writable columns
    /// (`PERMISSION_DENIED`).
    #[inline]
    pub fn get_many_mut<const N: usize, T: GuestComponent>(
        &mut self,
        query_index: usize,
        term_index: usize,
        entities: [FfiEntity; N],
    ) -> Result<[&mut T; N], FfiResult> {
        let filter = guest_uuid_filter::<T>(self.sys_ref.base_host().manifest_name());
        if query_index >= self.user_query_count {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        let q = self
            .queries
            .get_mut(query_index)
            .and_then(Option::as_mut)
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        self.sys_ref.entity_get_many_mut_checked::<N, T>(
            &mut q.inner,
            query_index,
            term_index,
            entities,
            Some(filter),
        )
    }

    /// Simultaneously borrows `N` disjoint mutable POD references `[&mut T; N]` (`T: AbiPod`)
    /// for `entities` using declared writable access on `(query_index, term_index)`.
    #[inline]
    pub fn get_many_pod_mut<const N: usize, T: AbiPod>(
        &mut self,
        query_index: usize,
        term_index: usize,
        entities: [FfiEntity; N],
    ) -> Result<[&mut T; N], FfiResult> {
        if query_index >= self.user_query_count {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        let q = self
            .queries
            .get_mut(query_index)
            .and_then(Option::as_mut)
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        self.sys_ref.entity_get_many_mut_checked::<N, T>(
            &mut q.inner,
            query_index,
            term_index,
            entities,
            None,
        )
    }

    /// Looks up a shared `&T` for a same-mod dynamic [`ModComponent`] on `entity` using declared
    /// access on `(query_index, term_index)`, verifying layout, UUID, and same-mod VTable
    /// provenance.
    #[inline]
    pub fn get_dynamic<T: ModComponent>(
        &self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&T> {
        let q = self.query_ref(query_index)?;
        self.sys_ref
            .entity_get_dynamic_checked::<T>(&q.inner, query_index, term_index, entity)
    }

    /// Looks up an exclusive `&mut T` for a same-mod dynamic [`ModComponent`] on `entity` using
    /// declared writable access on `(query_index, term_index)`, marking Bevy change ticks.
    #[inline]
    pub fn get_dynamic_mut<T: ModComponent>(
        &mut self,
        query_index: usize,
        term_index: usize,
        entity: FfiEntity,
    ) -> Option<&mut T> {
        if query_index >= self.user_query_count {
            return None;
        }
        let q = self.queries.get_mut(query_index)?.as_mut()?;
        self.sys_ref.entity_get_dynamic_mut_checked::<T>(
            &mut q.inner,
            query_index,
            term_index,
            entity,
        )
    }

    /// Looks up shared `&T` (`T: GuestComponent`) references for `N` entities simultaneously
    /// (mirroring Bevy's `Query::get_many`), returning `None` if any entity does not carry `T`.
    pub fn get_many<const N: usize, T: GuestComponent>(
        &self,
        query_index: usize,
        term_index: usize,
        entities: [FfiEntity; N],
    ) -> Option<[&T; N]> {
        if N == 0 {
            let empty: &[&T] = &[];
            return empty.try_into().ok();
        }
        let first = self.get::<T>(query_index, term_index, entities[0])?;
        let mut out = [first; N];
        for i in 1..N {
            out[i] = self.get::<T>(query_index, term_index, entities[i])?;
        }
        Some(out)
    }

    /// Drains unread messages (`M: GuestComponent`) declared at `decl_index`, validating `M`'s UUID.
    #[inline]
    pub fn read_messages<M: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages::<M>(decl_index)
    }

    /// Drains unread POD messages (`M: AbiPod`) declared at `decl_index`.
    #[inline]
    pub fn read_pod_messages<M: AbiPod>(&mut self, decl_index: usize) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages::<M>(decl_index)
    }

    /// Reads up to `out.len()` unread messages (`M: GuestComponent`) declared at `decl_index`
    /// directly into uninitialized slice `out` with zero heap allocations, validating `M`'s UUID.
    #[inline]
    pub fn read_messages_into_uninit<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut [MaybeUninit<M>],
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages_into_uninit::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread POD messages (`M: AbiPod`) declared at `decl_index` directly
    /// into uninitialized slice `out` with zero heap allocations.
    #[inline]
    pub fn read_pod_messages_into_uninit<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut [MaybeUninit<M>],
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages_into_uninit::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread messages (`M: GuestComponent`) declared at `decl_index`
    /// directly into `out` with zero heap allocations, validating `M`'s UUID.
    #[inline]
    pub fn read_messages_into<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut [M],
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages_into::<M>(decl_index, out)
    }

    /// Reads up to `out.len()` unread POD messages (`M: AbiPod`) declared at `decl_index` directly
    /// into `out` with zero heap allocations.
    #[inline]
    pub fn read_pod_messages_into<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut [M],
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages_into::<M>(decl_index, out)
    }

    /// Appends up to `max_count` unread messages (`M: GuestComponent`) declared at `decl_index`
    /// into `out`, reusing spare capacity when available and validating `M`'s UUID.
    #[inline]
    pub fn read_messages_into_vec<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        max_count: usize,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages_into_vec::<M>(decl_index, out, max_count)
    }

    /// Appends up to `max_count` unread POD messages (`M: AbiPod`) declared at `decl_index` into
    /// `out`, reusing spare capacity when available.
    #[inline]
    pub fn read_pod_messages_into_vec<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        max_count: usize,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages_into_vec::<M>(decl_index, out, max_count)
    }

    /// Reads up to `max_count` unread messages (`M: GuestComponent`) declared at `decl_index`,
    /// validating `M`'s UUID.
    #[inline]
    pub fn read_messages_batch<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages_batch::<M>(decl_index, max_count)
    }

    /// Reads up to `max_count` unread POD messages (`M: AbiPod`) declared at `decl_index`.
    #[inline]
    pub fn read_pod_messages_batch<M: AbiPod>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages_batch::<M>(decl_index, max_count)
    }

    /// Drains all unread messages (`M: GuestComponent`) declared at `decl_index` using custom
    /// [`DrainOptions`], validating `M`'s UUID.
    #[inline]
    pub fn drain_messages_with_options<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_messages_with_options::<M>(decl_index, opts)
    }

    /// Drains all unread POD messages (`M: AbiPod`) declared at `decl_index` using custom
    /// [`DrainOptions`].
    #[inline]
    pub fn drain_pod_messages_with_options<M: AbiPod>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_pod_messages_with_options::<M>(decl_index, opts)
    }

    /// Drains all unread messages (`M: GuestComponent`) declared at `decl_index` directly into
    /// `out`, validating `M`'s UUID and returning the number of appended messages.
    #[inline]
    pub fn drain_messages_into_vec<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        opts: DrainOptions,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_messages_into_vec::<M>(decl_index, out, opts)
    }

    /// Drains all unread POD messages (`M: AbiPod`) declared at `decl_index` directly into `out`,
    /// returning the number of appended messages.
    #[inline]
    pub fn drain_pod_messages_into_vec<M: AbiPod>(
        &mut self,
        decl_index: usize,
        out: &mut Vec<M>,
        opts: DrainOptions,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_pod_messages_into_vec::<M>(decl_index, out, opts)
    }

    /// Drains the unread message cursor (`M: GuestComponent`) declared at `decl_index` with zero
    /// heap allocations, validating `M`'s UUID and returning the drained message count.
    #[inline]
    pub fn drain_messages_count<M: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_messages_count::<M>(decl_index)
    }

    /// Drains the unread POD message cursor (`M: AbiPod`) declared at `decl_index` with zero heap
    /// allocations, returning the drained message count.
    #[inline]
    pub fn drain_pod_messages_count<M: AbiPod>(
        &mut self,
        decl_index: usize,
    ) -> Result<usize, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_pod_messages_count::<M>(decl_index)
    }

    /// Appends a message (`M: GuestComponent`) to the stream declared with `MESSAGE_WRITE` at
    /// `decl_index`, validating `M`'s UUID.
    #[inline]
    pub fn send_message<M: GuestComponent>(&mut self, decl_index: usize, msg: &M) -> FfiResult {
        match self.resource_iter_mut() {
            Some(q) => q.send_message::<M>(decl_index, msg),
            None => FfiResult::OUT_OF_RANGE,
        }
    }

    /// Appends a POD message (`M: AbiPod`) to the stream declared with `MESSAGE_WRITE` at
    /// `decl_index`.
    #[inline]
    pub fn send_pod_message<M: AbiPod>(&mut self, decl_index: usize, msg: &M) -> FfiResult {
        match self.resource_iter_mut() {
            Some(q) => q.send_pod_message::<M>(decl_index, msg),
            None => FfiResult::OUT_OF_RANGE,
        }
    }

    /// Batch-appends `messages` (`M: GuestComponent`) into the stream declared with `MESSAGE_WRITE`
    /// at `decl_index`.
    #[inline]
    pub fn send_messages<M: GuestComponent>(
        &mut self,
        decl_index: usize,
        messages: &[M],
    ) -> Result<(), FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .send_messages::<M>(decl_index, messages)
    }

    /// Batch-appends POD `messages` (`M: AbiPod`) into the stream declared with `MESSAGE_WRITE`
    /// at `decl_index`.
    #[inline]
    pub fn send_pod_messages<M: AbiPod>(
        &mut self,
        decl_index: usize,
        messages: &[M],
    ) -> Result<(), FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .send_pod_messages::<M>(decl_index, messages)
    }

    /// Drains unread VTable-backed non-POD messages (`M: ModComponent + Default`) declared at
    /// `decl_index` (e.g. via [`SystemBuilder::message_read_dynamic`](crate::SystemBuilder::message_read_dynamic)).
    #[inline]
    pub fn read_non_pod_messages<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_non_pod_messages::<M>(decl_index)
    }

    /// Drains unread VTable-backed non-POD messages (`M: ModComponent + Default`) declared at
    /// `decl_index` using custom [`DrainOptions`].
    #[inline]
    pub fn drain_non_pod_messages_with_options<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
        opts: DrainOptions,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .drain_non_pod_messages_with_options::<M>(decl_index, opts)
    }

    /// Reads up to `max_count` unread VTable-backed non-POD messages (`M: ModComponent + Default`)
    /// declared at `decl_index`.
    #[inline]
    pub fn read_non_pod_messages_batch<M: ModComponent + Default>(
        &mut self,
        decl_index: usize,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_non_pod_messages_batch::<M>(decl_index, max_count)
    }

    /// Sends a VTable-backed non-POD message (`M: ModComponent`) to the stream declared with
    /// `MESSAGE_WRITE` at `decl_index`.
    #[inline]
    pub fn send_non_pod_message<M: ModComponent>(
        &mut self,
        decl_index: usize,
        msg: &M,
    ) -> FfiResult {
        match self.resource_iter_mut() {
            Some(q) => q.send_non_pod_message::<M>(decl_index, msg),
            None => FfiResult::OUT_OF_RANGE,
        }
    }

    /// Splits `&mut self` into disjoint mutable handles for declared queries, the resource/message
    /// stream iterator, [`GuestCommands`], and [`ValidatedSystemRef`].
    #[inline]
    pub fn split(&mut self) -> SplitSystemContext<'_, 'a> {
        let commands = resolve_commands_slot(
            &mut self.commands,
            &mut self.lazy_commands,
            self.sys_ref.base_host(),
        );
        let user_count = self.user_query_count.min(self.queries.len());
        let (user_queries, resource_iter) = match self.resource_query_index {
            Some(res_idx) if res_idx >= user_count && res_idx < self.queries.len() => {
                let (left, right) = self.queries.split_at_mut(user_count);
                let res_offset = res_idx - user_count;
                (left, right[res_offset].as_mut())
            }
            _ => (&mut self.queries[..user_count], None),
        };
        SplitSystemContext {
            queries: user_queries,
            resource_iter,
            commands,
            sys: &self.sys_ref,
        }
    }

    /// Resolves and layout-validates `T: GuestComponent` into a cached [`TypedComponent<T>`]
    /// stored inside the owning [`bamabi_guest_sys::GuestInstance`] (subsequent calls hit the
    /// in-memory cache with zero FFI round-trips).
    #[inline]
    pub fn cached_component<T: GuestComponent>(&self) -> Result<TypedComponent<T>, FfiResult> {
        let base_host = self.sys_ref.base_host();
        let (_, uuid) = qualify_component_identity::<T>(base_host.manifest_name())?;
        base_host.cached_typed_component::<T>(uuid, || {
            resolve_guest_component::<T>(self.sys_ref.host(), base_host.manifest_name())
        })
    }

    /// Returns `(last_run, this_run)` Bevy change-detection ticks for the active system invocation
    /// via `BamHostApi::sys_current_ticks` (slot 79).
    #[inline]
    pub fn current_ticks(&self) -> Result<(u32, u32), FfiResult> {
        self.sys_ref.current_ticks()
    }

    /// Returns frame/fixed-step timing metadata ([`BamTimeInfo`]) for the active system invocation
    /// via `BamHostApi::sys_time_info` (slot 80).
    #[inline]
    pub fn time_info(&self) -> Result<BamTimeInfo, FfiResult> {
        self.sys_ref.time_info()
    }

    /// Resolves a canonical non-empty UTF-8 lookup `name` to its bound [`FfiEntity`] via
    /// `BamHostApi::resolve_named_object` (slot 75).
    #[inline]
    pub fn resolve_named_object(&self, name: &str) -> Result<FfiEntity, FfiResult> {
        self.sys_ref.resolve_named_object(name)
    }

    /// Resolves an interned [`BamSymbol`] handle back to its owned UTF-8 string via
    /// `BamHostApi::resolve_symbol_str` (slot 76).
    #[inline]
    pub fn resolve_symbol_str(&self, symbol: BamSymbol) -> Result<String, FfiResult> {
        self.sys_ref.resolve_symbol_str(symbol)
    }

    /// Queries all outgoing target entities linked to `entity` via relationship handle `relation`
    /// using `BamHostApi::sys_relation_targets` (slot 82).
    #[inline]
    pub fn relation_targets(
        &self,
        entity: FfiEntity,
        relation: BamComponent,
    ) -> Result<&[FfiEntity], FfiResult> {
        self.sys_ref.relation_targets(entity, relation)
    }

    /// Typed convenience variant of [`Self::relation_targets`] resolving `Rel: GuestComponent` via
    /// the per-instance component handle cache.
    #[inline]
    pub fn relation_targets_of<Rel: GuestComponent>(
        &self,
        entity: FfiEntity,
    ) -> Result<&[FfiEntity], FfiResult> {
        let rel = self.cached_component::<Rel>()?.handle();
        self.sys_ref.relation_targets(entity, rel)
    }

    /// Queries all incoming source entities targeting `target` via relationship handle `relation`
    /// using `BamHostApi::sys_relation_sources` (slot 83).
    #[inline]
    pub fn relation_sources(
        &self,
        target: FfiEntity,
        relation: BamComponent,
    ) -> Result<&[FfiEntity], FfiResult> {
        self.sys_ref.relation_sources(target, relation)
    }

    /// Typed convenience variant of [`Self::relation_sources`] resolving `Rel: GuestComponent` via
    /// the per-instance component handle cache.
    #[inline]
    pub fn relation_sources_of<Rel: GuestComponent>(
        &self,
        target: FfiEntity,
    ) -> Result<&[FfiEntity], FfiResult> {
        let rel = self.cached_component::<Rel>()?.handle();
        self.sys_ref.relation_sources(target, rel)
    }

    /// Borrows raw payload bytes for relationship edge `(entity, relation, target)` via
    /// `BamHostApi::sys_relation_edge_data` (slot 84).
    #[inline]
    pub fn relation_edge_bytes(
        &self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> Result<&[u8], FfiResult> {
        self.sys_ref.relation_edge_bytes(entity, relation, target)
    }

    /// Borrows typed POD payload `&T` for relationship edge `(entity, relation, target)` via
    /// `BamHostApi::sys_relation_edge_data` (slot 84).
    #[inline]
    pub fn relation_edge_data<T: AbiPod>(
        &self,
        entity: FfiEntity,
        relation: BamComponent,
        target: FfiEntity,
    ) -> Result<&T, FfiResult> {
        self.sys_ref
            .relation_edge_data::<T>(entity, relation, target)
    }

    /// Borrows a contiguous `&[M]` slice of unread messages (`M: GuestComponent`) declared at
    /// `decl_index` with zero allocations and zero copies via `BamHostApi::msg_read_slice` (slot 88).
    #[inline]
    pub fn read_messages_slice<M: GuestComponent>(
        &mut self,
        decl_index: usize,
    ) -> Result<&[M], FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_messages_slice::<M>(decl_index)
    }

    /// Borrows a contiguous `&[M]` slice of unread POD messages (`M: AbiPod`) declared at
    /// `decl_index` via `BamHostApi::msg_read_slice` (slot 88).
    #[inline]
    pub fn read_pod_messages_slice<M: AbiPod>(
        &mut self,
        decl_index: usize,
    ) -> Result<&[M], FfiResult> {
        self.resource_iter_mut()
            .ok_or(FfiResult::OUT_OF_RANGE)?
            .read_pod_messages_slice::<M>(decl_index)
    }

    /// Resolves the `decl_index` inside `resource_iter` corresponding to pre-resolved message
    /// stream handle `handle`.
    fn find_message_term_index(
        &self,
        handle: BamComponent,
        write_required: bool,
    ) -> Result<usize, FfiResult> {
        if !handle.is_valid() {
            return Err(FfiResult::INVALID_COMPONENT);
        }
        let res_iter = self.resource_iter().ok_or(FfiResult::NOT_FOUND)?;
        for idx in 0..MAX_TERMS_PER_QUERY {
            let Ok(info) = res_iter.column_info(idx) else {
                break;
            };
            if info.component == handle {
                if write_required {
                    if info.mode == AccessMode::MESSAGE_WRITE {
                        return Ok(idx);
                    }
                } else if matches!(
                    info.mode,
                    AccessMode::MESSAGE_READ | AccessMode::MESSAGE_WRITE
                ) {
                    return Ok(idx);
                }
            }
        }
        Err(FfiResult::NOT_FOUND)
    }

    /// Drains unread messages (`M: GuestComponent`) for pre-resolved handle `handle`, locating its
    /// declared message stream term automatically.
    #[inline]
    pub fn read_messages_by_handle<M: GuestComponent>(
        &mut self,
        handle: BamComponent,
    ) -> Result<Vec<M>, FfiResult> {
        let decl_index = self.find_message_term_index(handle, false)?;
        self.read_messages::<M>(decl_index)
    }

    /// Drains unread POD messages (`M: AbiPod`) for pre-resolved handle `handle`.
    #[inline]
    pub fn read_pod_messages_by_handle<M: AbiPod>(
        &mut self,
        handle: BamComponent,
    ) -> Result<Vec<M>, FfiResult> {
        let decl_index = self.find_message_term_index(handle, false)?;
        self.read_pod_messages::<M>(decl_index)
    }

    /// Borrows a zero-copy `&[M]` slice of unread messages (`M: GuestComponent`) for pre-resolved
    /// handle `handle` via `BamHostApi::msg_read_slice` (slot 88).
    #[inline]
    pub fn read_messages_slice_by_handle<M: GuestComponent>(
        &mut self,
        handle: BamComponent,
    ) -> Result<&[M], FfiResult> {
        let decl_index = self.find_message_term_index(handle, false)?;
        self.read_messages_slice::<M>(decl_index)
    }

    /// Borrows a zero-copy `&[M]` slice of unread POD messages (`M: AbiPod`) for pre-resolved
    /// handle `handle` via `BamHostApi::msg_read_slice` (slot 88).
    #[inline]
    pub fn read_pod_messages_slice_by_handle<M: AbiPod>(
        &mut self,
        handle: BamComponent,
    ) -> Result<&[M], FfiResult> {
        let decl_index = self.find_message_term_index(handle, false)?;
        self.read_pod_messages_slice::<M>(decl_index)
    }

    /// Appends a message (`M: GuestComponent`) into the declared `MESSAGE_WRITE` stream matching
    /// pre-resolved handle `handle`.
    #[inline]
    pub fn send_message_by_handle<M: GuestComponent>(
        &mut self,
        handle: BamComponent,
        msg: &M,
    ) -> FfiResult {
        let decl_index = match self.find_message_term_index(handle, true) {
            Ok(idx) => idx,
            Err(err) => return err,
        };
        self.send_message::<M>(decl_index, msg)
    }

    /// Appends a POD message (`M: AbiPod`) into the declared `MESSAGE_WRITE` stream matching
    /// pre-resolved handle `handle`.
    #[inline]
    pub fn send_pod_message_by_handle<M: AbiPod>(
        &mut self,
        handle: BamComponent,
        msg: &M,
    ) -> FfiResult {
        let decl_index = match self.find_message_term_index(handle, true) {
            Ok(idx) => idx,
            Err(err) => return err,
        };
        self.send_pod_message::<M>(decl_index, msg)
    }

    /// Batch-appends `messages` (`M: GuestComponent`) into the declared `MESSAGE_WRITE` stream
    /// matching pre-resolved handle `handle`.
    #[inline]
    pub fn send_messages_by_handle<M: GuestComponent>(
        &mut self,
        handle: BamComponent,
        messages: &[M],
    ) -> Result<(), FfiResult> {
        let decl_index = self.find_message_term_index(handle, true)?;
        self.send_messages::<M>(decl_index, messages)
    }

    /// Batch-appends POD `messages` (`M: AbiPod`) into the declared `MESSAGE_WRITE` stream
    /// matching pre-resolved handle `handle`.
    #[inline]
    pub fn send_pod_messages_by_handle<M: AbiPod>(
        &mut self,
        handle: BamComponent,
        messages: &[M],
    ) -> Result<(), FfiResult> {
        let decl_index = self.find_message_term_index(handle, true)?;
        self.send_pod_messages::<M>(decl_index, messages)
    }
}
