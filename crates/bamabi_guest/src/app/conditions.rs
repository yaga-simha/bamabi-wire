//! Composable Bevy-idiomatic run conditions (`bevy_ecs::schedule::common_conditions` parity)
//! for [`SystemBuilder::run_if`](crate::SystemBuilder::run_if) and
//! [`ObserverBuilder::run_if`](crate::ObserverBuilder::run_if).

use core::ffi::CStr;

use crate::{app::RunConditionContext, component::GuestComponent};

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
