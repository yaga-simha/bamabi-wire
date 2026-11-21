//! Unified system execution context (`SystemContext`) and run-condition context (`RunConditionContext`).

use core::ffi::{CStr, c_void};

use bamabi_guest_sys::{
    Host, LazyCommands, TypedComponent, ValidatedCommands, ValidatedIter,
    ValidatedSystemInvocation, ValidatedSystemRef, resolve_component,
};
use bamabi_sys::{
    AbiPod, BamChangeTicks, BamComponent, BamHostApi, BamSystemCtx, ComponentUuid, FfiEntity,
    FfiResult, MAX_QUERIES_PER_SYSTEM, ModComponent,
};

use crate::{
    app::builder::IntoSystemResult,
    commands::GuestCommands,
    component::{GuestComponent, guest_uuid_filter, resolve_guest_component},
    iter::{Chunk, ChunkIter, QueryTuple, TypedChunkIter},
};

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

/// Unified per-tick execution context passed to every guest system built with
/// [`SystemBuilder::build`](crate::SystemBuilder::build).
///
/// Supports zero-query, single-query (`iter`, `iter_and_commands`), multi-query (`query_mut`,
/// `queries_2_mut`, `queries_2_and_commands`), declaration-indexed singleton resources
/// (`resource`, `resource_mut`), and handle/batched [`GuestCommands`].
pub struct SystemContext<'a> {
    pub(crate) sys_ref: ValidatedSystemRef<'a>,
    user_query_count: usize,
    query0: Option<ChunkIter<'a>>,
    extra_queries: Option<[Option<ChunkIter<'a>>; MAX_QUERIES_PER_SYSTEM - 1]>,
    resource_query: Option<ChunkIter<'a>>,
    empty_iter: Option<ChunkIter<'a>>,
    commands: Option<GuestCommands<'a>>,
    lazy_commands: Option<LazyCommands<'a>>,
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

/// Splits two distinct mutable query slots (`idx_a != idx_b`) out of `query0` and `extra_queries`.
#[inline]
fn split_queries_2_mut<'b, 'a>(
    query0: &'b mut Option<ChunkIter<'a>>,
    extra_queries: &'b mut Option<[Option<ChunkIter<'a>>; MAX_QUERIES_PER_SYSTEM - 1]>,
    user_query_count: usize,
    idx_a: usize,
    idx_b: usize,
) -> Option<(&'b mut ChunkIter<'a>, &'b mut ChunkIter<'a>)> {
    if idx_a == idx_b || idx_a >= user_query_count || idx_b >= user_query_count {
        return None;
    }
    if idx_a == 0 {
        let extra = extra_queries.as_mut()?;
        return Some((query0.as_mut()?, extra.get_mut(idx_b - 1)?.as_mut()?));
    }
    if idx_b == 0 {
        let extra = extra_queries.as_mut()?;
        return Some((extra.get_mut(idx_a - 1)?.as_mut()?, query0.as_mut()?));
    }
    let extra = extra_queries.as_mut()?;
    let ea = idx_a - 1;
    let eb = idx_b - 1;
    if ea < eb {
        let (left, right) = extra.split_at_mut(eb);
        Some((left[ea].as_mut()?, right[0].as_mut()?))
    } else {
        let (left, right) = extra.split_at_mut(ea);
        Some((right[0].as_mut()?, left[eb].as_mut()?))
    }
}

/// Splits three distinct mutable query slots (`idx_a`, `idx_b`, `idx_c`) out of `query0` and
/// `extra_queries`.
#[allow(clippy::type_complexity)]
fn split_queries_3_mut<'b, 'a>(
    query0: &'b mut Option<ChunkIter<'a>>,
    extra_queries: &'b mut Option<[Option<ChunkIter<'a>>; MAX_QUERIES_PER_SYSTEM - 1]>,
    user_query_count: usize,
    idx_a: usize,
    idx_b: usize,
    idx_c: usize,
) -> Option<(
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
    &'b mut ChunkIter<'a>,
)> {
    if idx_a == idx_b
        || idx_b == idx_c
        || idx_a == idx_c
        || idx_a >= user_query_count
        || idx_b >= user_query_count
        || idx_c >= user_query_count
    {
        return None;
    }
    let mut order = [(idx_a, 0u8), (idx_b, 1u8), (idx_c, 2u8)];
    order.sort_unstable_by_key(|&(idx, _)| idx);
    let (i0, p0) = order[0];
    let (i1, p1) = order[1];
    let (i2, p2) = order[2];
    let extra = extra_queries.as_mut()?;
    let (r0, r1, r2) = if i0 == 0 {
        let (s1, s2) = extra.split_at_mut(i2 - 1);
        (query0.as_mut()?, s1[i1 - 1].as_mut()?, s2[0].as_mut()?)
    } else {
        let e0 = i0 - 1;
        let e1 = i1 - 1;
        let e2 = i2 - 1;
        let (s0, rest1) = extra.split_at_mut(e1);
        let (s1, s2) = rest1.split_at_mut(e2 - e1);
        (s0[e0].as_mut()?, s1[0].as_mut()?, s2[0].as_mut()?)
    };
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
    /// Constructs a [`SystemContext`] by extracting the validated system reference, declared query
    /// slots, optional system-level resource query slot, and lazy deferred command token from `inv`.
    #[inline(always)]
    pub(crate) fn from_invocation(inv: &mut ValidatedSystemInvocation<'a>) -> Self {
        let base_host = inv.base_host();
        let sys_ref = inv
            .take_system_ref()
            .unwrap_or_else(|| ValidatedSystemRef::empty(base_host));
        let user_query_count = inv.query_count();
        let (query0, extra_queries) = if user_query_count > 0 {
            let q0 = inv.take_raw_query_slot(0).map(ChunkIter::from_validated);
            let extra = if user_query_count > 1 {
                let mut rest: [Option<ChunkIter<'a>>; MAX_QUERIES_PER_SYSTEM - 1] =
                    [const { None }; MAX_QUERIES_PER_SYSTEM - 1];
                let mut offset = 0usize;
                let extra_count = user_query_count - 1;
                while offset < extra_count {
                    rest[offset] = inv
                        .take_raw_query_slot(offset + 1)
                        .map(ChunkIter::from_validated);
                    offset += 1;
                }
                Some(rest)
            } else {
                None
            };
            (q0, extra)
        } else {
            (None, None)
        };
        let resource_query = if let Some(res_idx) = inv.resource_query_index() {
            inv.take_raw_query_slot(res_idx)
                .map(ChunkIter::from_validated)
        } else {
            None
        };
        let lazy_commands = Some(inv.take_lazy_commands());
        Self {
            sys_ref,
            user_query_count,
            query0,
            extra_queries,
            resource_query,
            empty_iter: None,
            commands: None,
            lazy_commands,
        }
    }

    /// Borrows query slot `index` immutably without touching `extra_queries` for `index == 0`.
    #[inline]
    fn query_ref(&self, index: usize) -> Option<&ChunkIter<'a>> {
        if index >= self.user_query_count {
            return None;
        }
        if index == 0 {
            self.query0.as_ref()
        } else {
            self.extra_queries.as_ref()?.get(index - 1)?.as_ref()
        }
    }

    /// Borrows query slot `index` mutably without touching `extra_queries` for `index == 0`.
    #[inline]
    fn query_slot_mut(&mut self, index: usize) -> Option<&mut ChunkIter<'a>> {
        if index >= self.user_query_count {
            return None;
        }
        if index == 0 {
            self.query0.as_mut()
        } else {
            self.extra_queries.as_mut()?.get_mut(index - 1)?.as_mut()
        }
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
        self.host().last_error()
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
        if self.user_query_count > 0
            && let Some(q0) = self.query0.as_mut()
        {
            return q0;
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
        split_queries_2_mut(
            &mut self.query0,
            &mut self.extra_queries,
            self.user_query_count,
            idx_a,
            idx_b,
        )
    }

    /// Simultaneously borrows three distinct queries `idx_a`, `idx_b`, and `idx_c` (`None` if any
    /// two indices are equal or any index is out of range).
    pub fn queries_3_mut(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<(&mut ChunkIter<'a>, &mut ChunkIter<'a>, &mut ChunkIter<'a>)> {
        split_queries_3_mut(
            &mut self.query0,
            &mut self.extra_queries,
            self.user_query_count,
            idx_a,
            idx_b,
            idx_c,
        )
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
        let iter = if self.user_query_count > 0
            && let Some(q0) = self.query0.as_mut()
        {
            q0
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
        let (a, b) = split_queries_2_mut(
            &mut self.query0,
            &mut self.extra_queries,
            self.user_query_count,
            idx_a,
            idx_b,
        )?;
        let cmds = resolve_commands_slot(
            &mut self.commands,
            &mut self.lazy_commands,
            self.sys_ref.base_host(),
        );
        Some((a, b, cmds))
    }

    /// Simultaneously borrows three distinct queries `idx_a`, `idx_b`, and `idx_c` alongside
    /// [`GuestCommands`].
    #[allow(clippy::type_complexity)]
    pub fn queries_3_and_commands(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<(
        &mut ChunkIter<'a>,
        &mut ChunkIter<'a>,
        &mut ChunkIter<'a>,
        &mut GuestCommands<'a>,
    )> {
        let (a, b, c) = split_queries_3_mut(
            &mut self.query0,
            &mut self.extra_queries,
            self.user_query_count,
            idx_a,
            idx_b,
            idx_c,
        )?;
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
        self.resource_query.as_mut()
    }

    /// Returns the iterator holding system-level resources/messages (preferring the dedicated
    /// `resource_query` slot, or falling back to primary query `0`).
    #[inline]
    fn resource_iter(&self) -> Option<&ChunkIter<'a>> {
        self.resource_query.as_ref().or(self.query0.as_ref())
    }

    /// Mutable variant of [`Self::resource_iter`] for exclusive resource and message-writer access.
    #[inline]
    fn resource_iter_mut(&mut self) -> Option<&mut ChunkIter<'a>> {
        self.resource_query.as_mut().or(self.query0.as_mut())
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
    #[allow(clippy::type_complexity)]
    pub fn queries_3_typed<Q1: QueryTuple, Q2: QueryTuple, Q3: QueryTuple>(
        &mut self,
        idx_a: usize,
        idx_b: usize,
        idx_c: usize,
    ) -> Option<(
        TypedChunkIter<'_, 'a, Q1>,
        TypedChunkIter<'_, 'a, Q2>,
        TypedChunkIter<'_, 'a, Q3>,
    )> {
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
        let q = if query_index >= self.user_query_count {
            return None;
        } else if query_index == 0 {
            self.query0.as_mut()?
        } else {
            self.extra_queries
                .as_mut()?
                .get_mut(query_index - 1)?
                .as_mut()?
        };
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
        let q = if query_index >= self.user_query_count {
            return None;
        } else if query_index == 0 {
            self.query0.as_mut()?
        } else {
            self.extra_queries
                .as_mut()?
                .get_mut(query_index - 1)?
                .as_mut()?
        };
        self.sys_ref.entity_get_mut_checked::<T>(
            &mut q.inner,
            query_index,
            term_index,
            entity,
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
        let q = if query_index >= self.user_query_count {
            return None;
        } else if query_index == 0 {
            self.query0.as_mut()?
        } else {
            self.extra_queries
                .as_mut()?
                .get_mut(query_index - 1)?
                .as_mut()?
        };
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
}
