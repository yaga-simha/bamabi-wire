//! Panic containment helpers preventing Rust unwinds across `extern "C"` boundaries.
//!
//! Under `panic = "unwind"`, every `extern "C"` guest entry point and callback trampoline wraps
//! user code in [`catch_guest_result`], [`catch_guest_bool`], or [`catch_guest_unit`]. Any caught
//! panic payload (`Box<dyn Any + Send>`) is explicitly dropped on the guest's own heap before
//! returning across the C ABI boundary so host and guest allocators never cross.

#[cfg(not(kani))]
use std::panic::{AssertUnwindSafe, catch_unwind};

use bamabi_sys::{FfiBool, FfiResult};

/// Drops a caught panic payload on the guest heap while guarding against double-panics in custom
/// panic payload destructors.
#[cfg(not(kani))]
#[inline]
fn drop_panic_payload_safely(payload: Box<dyn core::any::Any + Send + 'static>) {
    if let Err(double_panic) = catch_unwind(AssertUnwindSafe(move || {
        drop(payload);
    })) {
        // Forgetting the secondary payload is preferable to unwinding across `extern "C"`, which
        // would be immediate undefined behavior.
        core::mem::forget(double_panic);
    }
}

/// Executes `f`, converting any caught Rust panic into [`FfiResult::PANIC_CAUGHT`] after dropping
/// the panic payload on the guest heap.
#[inline]
pub fn catch_guest_result<F>(f: F) -> FfiResult
where
    F: FnOnce() -> FfiResult,
{
    #[cfg(not(kani))]
    {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(result) => result,
            Err(payload) => {
                drop_panic_payload_safely(payload);
                FfiResult::PANIC_CAUGHT
            }
        }
    }
    // Kani always compiles with `-C panic=abort` (where `catch_unwind` cannot catch unwinds, and
    // Kani 0.68 ICEs lowering `core::intrinsics::catch_unwind`). Executing `f()` directly under
    // `cfg(kani)` proves strict panic-freedom of the guarded body; unwind containment is verified
    // by Miri and runtime panic-catch tests.
    #[cfg(kani)]
    {
        f()
    }
}

/// Executes `f`, converting any caught Rust panic into [`FfiBool::FALSE`] after dropping the
/// panic payload on the guest heap.
#[inline]
pub fn catch_guest_bool<F>(f: F) -> FfiBool
where
    F: FnOnce() -> FfiBool,
{
    #[cfg(not(kani))]
    {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(flag) => flag,
            Err(payload) => {
                drop_panic_payload_safely(payload);
                FfiBool::FALSE
            }
        }
    }
    #[cfg(kani)]
    {
        f()
    }
}

/// Executes `f`, swallowing any caught Rust panic after dropping the panic payload on the guest
/// heap (used for void `extern "C"` destructors and `bamabi_shutdown`).
#[inline]
pub fn catch_guest_unit<F>(f: F)
where
    F: FnOnce(),
{
    #[cfg(not(kani))]
    {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(f)) {
            drop_panic_payload_safely(payload);
        }
    }
    #[cfg(kani)]
    {
        f();
    }
}
