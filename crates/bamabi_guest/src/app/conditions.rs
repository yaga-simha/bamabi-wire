//! Composable Bevy-idiomatic run conditions (`bevy_ecs::schedule::common_conditions` parity)
//! for [`SystemBuilder::run_if`](crate::SystemBuilder::run_if) and
//! [`ObserverBuilder::run_if`](crate::ObserverBuilder::run_if).

use core::ffi::CStr;

use bamabi_guest_sys::Host;
use bamabi_sys::{AccessMode, BamTermDesc, FfiResult};

use crate::{
    app::{RunConditionContext, RunConditionSystemContext},
    component::{GuestComponent, resolve_guest_component},
};

/// Returns a run condition that evaluates to `true` on its first invocation and `false` on every
/// subsequent invocation (mirroring `bevy_ecs::schedule::common_conditions::run_once`).
#[inline]
pub fn run_once() -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static {
    let mut ran = false;
    move |_ctx| {
        if ran {
            false
        } else {
            ran = true;
            true
        }
    }
}

/// Negates run condition `condition` (mirroring `common_conditions::not`).
#[inline]
pub fn not<F>(mut condition: F) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static
where
    F: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
{
    move |ctx| !condition(ctx)
}

/// Composes two run conditions with short-circuit logical AND (`a && b`).
#[inline]
pub fn and<A, B>(
    mut a: A,
    mut b: B,
) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static
where
    A: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
    B: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
{
    move |ctx| a(ctx) && b(ctx)
}

/// Composes two run conditions with short-circuit logical OR (`a || b`).
#[inline]
pub fn or<A, B>(mut a: A, mut b: B) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static
where
    A: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
    B: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
{
    move |ctx| a(ctx) || b(ctx)
}

/// Composes two run conditions with logical XOR (`a ^ b`), evaluating both predicates.
#[inline]
pub fn xor<A, B>(
    mut a: A,
    mut b: B,
) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static
where
    A: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
    B: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
{
    move |ctx| a(ctx) ^ b(ctx)
}

/// Returns a run condition that evaluates to `true` once every `interval` evaluations (ticks `0`,
/// `interval`, `2 * interval`, ...). Returns `false` permanently when `interval == 0`.
#[inline]
pub fn every_n_ticks(
    interval: u64,
) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static {
    let mut tick: u64 = 0;
    move |_ctx| {
        if interval == 0 {
            return false;
        }
        let fire = tick.is_multiple_of(interval);
        tick = tick.wrapping_add(1);
        fire
    }
}

/// Returns a run condition that evaluates to `true` iff `T: GuestComponent` is currently registered
/// in the host `World` (analogous to `resource_exists` / component registration check).
#[inline]
pub fn component_registered<T: GuestComponent>()
-> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static {
    move |ctx| ctx.resolve_guest_component::<T>().is_ok()
}

/// Returns a run condition that evaluates to `true` iff a component or resource named `name` is
/// registered on the host `World`.
#[inline]
pub fn component_registered_named(
    name: &'static CStr,
) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static {
    move |ctx| ctx.resolve_component(Some(name), None).is_ok()
}

/// Returns a run condition that evaluates to `true` iff custom host symbol `symbol`
/// (`"bam.v0.<name>"`) is exported by the host.
#[inline]
pub fn symbol_exists(
    symbol: &'static CStr,
) -> impl FnMut(&RunConditionContext<'_>) -> bool + Send + 'static {
    move |ctx| ctx.resolve_custom_symbol(symbol).is_ok()
}

/// Boxed term-builder closure resolving the read-only [`BamTermDesc`] list for an [`EcsRunCondition`].
type EcsConditionTermsBuilder = Box<dyn FnOnce(&Host, &str) -> Result<Vec<BamTermDesc>, FfiResult>>;

/// Boxed evaluator closure for an [`EcsRunCondition`] receiving `(ctx, base_term_offset)`.
type EcsConditionEval =
    Box<dyn FnMut(&mut RunConditionSystemContext<'_>, usize) -> bool + Send + 'static>;

/// Type-erased read-only ECS run condition pairing its [`BamTermDesc`] query terms with a
/// predicate over [`RunConditionSystemContext`] (receiving its base `term_offset` inside the
/// combined `run_if_query`).
///
/// Pass to [`SystemBuilder::run_if_condition`](crate::SystemBuilder::run_if_condition). Multiple
/// calls to `.run_if_condition(...)` merge their read-only terms into a single `run_if_query`
/// and evaluate with short-circuit logical AND semantics.
pub struct EcsRunCondition {
    pub(crate) build_terms: EcsConditionTermsBuilder,
    pub(crate) eval: EcsConditionEval,
}

impl EcsRunCondition {
    /// Creates an [`EcsRunCondition`] from a term-builder closure and a predicate receiving
    /// `(&mut RunConditionSystemContext<'_>, base_term_offset)`.
    pub fn new<B, F>(build_terms: B, eval: F) -> Self
    where
        B: FnOnce(&Host, &str) -> Result<Vec<BamTermDesc>, FfiResult> + 'static,
        F: FnMut(&mut RunConditionSystemContext<'_>, usize) -> bool + Send + 'static,
    {
        Self {
            build_terms: Box::new(build_terms),
            eval: Box::new(eval),
        }
    }

    /// Negates this ECS run condition (`!self`).
    #[inline]
    pub fn negated(self) -> Self {
        let mut inner = self.eval;
        Self {
            build_terms: self.build_terms,
            eval: Box::new(move |ctx, offset| !inner(ctx, offset)),
        }
    }
}

/// Returns an ECS-aware run condition that evaluates to `true` iff singleton resource
/// `R: GuestComponent` currently exists in the host `World`
/// (mirroring `bevy_ecs::schedule::common_conditions::resource_exists`).
pub fn resource_exists<R: GuestComponent>() -> EcsRunCondition {
    EcsRunCondition::new(
        |host, manifest| {
            let comp = resolve_guest_component::<R>(host, manifest)?;
            Ok(vec![BamTermDesc::new(comp, AccessMode::RESOURCE_READ)])
        },
        |ctx, offset| ctx.has_resource::<R>(offset),
    )
}

/// Returns an ECS-aware run condition that evaluates to `true` iff singleton resource
/// `R: GuestComponent` exists and has changed since this condition last ran
/// (mirroring `bevy_ecs::schedule::common_conditions::resource_changed`).
pub fn resource_changed<R: GuestComponent>() -> EcsRunCondition {
    EcsRunCondition::new(
        |host, manifest| {
            let comp = resolve_guest_component::<R>(host, manifest)?;
            Ok(vec![BamTermDesc::new(comp, AccessMode::RESOURCE_READ)])
        },
        |ctx, offset| ctx.is_resource_changed::<R>(offset),
    )
}

/// Returns an ECS-aware run condition that evaluates to `true` iff singleton resource
/// `R: GuestComponent` was newly added since this condition last ran
/// (mirroring `bevy_ecs::schedule::common_conditions::resource_added`).
pub fn resource_added<R: GuestComponent>() -> EcsRunCondition {
    EcsRunCondition::new(
        |host, manifest| {
            let comp = resolve_guest_component::<R>(host, manifest)?;
            Ok(vec![BamTermDesc::new(comp, AccessMode::RESOURCE_READ)])
        },
        |ctx, offset| ctx.is_resource_added::<R>(offset),
    )
}

/// Returns an ECS-aware run condition that evaluates to `true` iff singleton resource
/// `R: GuestComponent + PartialEq` exists and equals `expected`
/// (mirroring `bevy_ecs::schedule::common_conditions::resource_equals`).
pub fn resource_equals<R: GuestComponent + PartialEq + Send + 'static>(
    expected: R,
) -> EcsRunCondition {
    EcsRunCondition::new(
        |host, manifest| {
            let comp = resolve_guest_component::<R>(host, manifest)?;
            Ok(vec![BamTermDesc::new(comp, AccessMode::RESOURCE_READ)])
        },
        move |ctx, offset| ctx.res::<R>(offset).is_some_and(|cur| *cur == expected),
    )
}

/// Returns an ECS-aware run condition that evaluates to `true` iff at least one unread buffered
/// message `M: GuestComponent` is available (mirroring `common_conditions::on_message`).
pub fn on_message<M: GuestComponent>() -> EcsRunCondition {
    EcsRunCondition::new(
        |host, manifest| {
            let comp = resolve_guest_component::<M>(host, manifest)?;
            Ok(vec![BamTermDesc::new(comp, AccessMode::MESSAGE_READ)])
        },
        |ctx, offset| ctx.has_messages::<M>(offset),
    )
}

/// Returns an ECS-aware run condition that evaluates to `true` iff singleton state resource
/// `S: GuestComponent + PartialEq` exists and equals `expected`
/// (mirroring `bevy_state::condition::in_state`).
pub fn in_state<S: GuestComponent + PartialEq + Send + 'static>(expected: S) -> EcsRunCondition {
    resource_equals(expected)
}
