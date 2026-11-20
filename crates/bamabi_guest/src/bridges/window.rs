//! Host-opt-in `#![forbid(unsafe_code)]`, zero-Bevy guest bridge conventions for Bevy `0.20`
//! `bevy_window` (`Window`, `WindowMode`, `PrimaryWindow`, `WindowResized`,
//! `WindowCloseRequested`) and `bevy_app::AppExit`.
//!
//! **Host-opt-in convention:** These wire types require the host game to opt in to exposing
//! `"bevy_window::Window"` (`HostWireBridge<FfiWindow>`) or the direct POD mirror/message types
//! below.

use bamabi_guest_sys::Host;
use bamabi_sys::{AccessMode, ComponentUuid, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    FfiAppExit, FfiWindow, FfiWindowCloseRequested, FfiWindowMode, FfiWindowResized,
};

use crate::{
    app::{QueryBuilder, SystemContext},
    bridges::std_types::GuestWireBridge,
    component::GuestComponent,
};

/// Canonical ABI name for host-bridged native `bevy_window::Window` (`GuestWindowBridge`).
pub const CANONICAL_WINDOW_NAME: &str = "bevy_window::Window";
/// Canonical ABI name for direct POD mirror [`FfiWindow`].
pub const CANONICAL_FFI_WINDOW_NAME: &str = "bamabi::window::FfiWindow";
/// Canonical ABI name for `bevy_window::PrimaryWindow` marker component.
pub const CANONICAL_PRIMARY_WINDOW_NAME: &str = "bevy_window::PrimaryWindow";
/// Canonical ABI name for `bevy_window::WindowResized` message/event.
pub const CANONICAL_WINDOW_RESIZED_NAME: &str = "bevy_window::WindowResized";
/// Canonical ABI name for `bevy_window::WindowCloseRequested` message/event.
pub const CANONICAL_WINDOW_CLOSE_REQUESTED_NAME: &str = "bevy_window::WindowCloseRequested";
/// Canonical ABI name for `bevy_app::AppExit` message/event.
pub const CANONICAL_APP_EXIT_NAME: &str = "bevy_app::AppExit";

impl GuestComponent for FfiWindow {
    const NAME: &'static CStr = c"bamabi::window::FfiWindow";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiWindowResized {
    const NAME: &'static CStr = c"bevy_window::WindowResized";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiWindowCloseRequested {
    const NAME: &'static CStr = c"bevy_window::WindowCloseRequested";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiAppExit {
    const NAME: &'static CStr = c"bevy_app::AppExit";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Guest wire VTable bridge for host-opt-in `bevy_window::Window`.
pub type GuestWindowBridge = GuestWireBridge<FfiWindow>;

impl GuestWindowBridge {
    /// Resolves a host-opt-in `Window` VTable (`"bam.v0.Window.vtable"`).
    pub fn resolve_window(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_WINDOW_NAME,
            c"bam.v0.bevy_window::Window.vtable",
            Some(c"bam.v0.Window.vtable"),
        )
    }
}

/// Extension trait on [`QueryBuilder`] adding host-opt-in Bevy `0.20` window query terms.
pub trait GuestWindowQueryBuilderExt {
    /// Appends a shared read (`&Window` wire-bridged) term.
    fn read_window(self) -> Self;
    /// Appends an exclusive write (`&mut Window` wire-bridged) term.
    fn write_window(self) -> Self;
    /// Appends a `With<PrimaryWindow>` filter term.
    fn with_primary_window(self) -> Self;
}

impl GuestWindowQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_window(self) -> Self {
        self.term_named(
            c"bevy_window::Window",
            Some(ComponentUuid::from_canonical_name(CANONICAL_WINDOW_NAME)),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_window(self) -> Self {
        self.term_named(
            c"bevy_window::Window",
            Some(ComponentUuid::from_canonical_name(CANONICAL_WINDOW_NAME)),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn with_primary_window(self) -> Self {
        self.term_named(
            c"bevy_window::PrimaryWindow",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_PRIMARY_WINDOW_NAME,
            )),
            AccessMode::FILTER_ONLY,
        )
    }
}

/// Extension trait on [`SystemContext`] for emitting `AppExit` messages through a declared
/// `message_write::<FfiAppExit>()` stream.
pub trait GuestAppExitExt {
    /// Writes `exit` into the `AppExit` message stream declared at `decl_index`.
    fn send_app_exit(&mut self, decl_index: usize, exit: FfiAppExit) -> FfiResult;
    /// Writes [`FfiAppExit::SUCCESS`] into the `AppExit` message stream declared at `decl_index`.
    fn send_app_exit_success(&mut self, decl_index: usize) -> FfiResult;
    /// Writes [`FfiAppExit::error`] with `code` into the `AppExit` message stream declared at `decl_index`.
    fn send_app_exit_error(&mut self, decl_index: usize, code: u8) -> FfiResult;
}

impl GuestAppExitExt for SystemContext<'_> {
    #[inline]
    fn send_app_exit(&mut self, decl_index: usize, exit: FfiAppExit) -> FfiResult {
        self.send_message(decl_index, &exit)
    }

    #[inline]
    fn send_app_exit_success(&mut self, decl_index: usize) -> FfiResult {
        self.send_message(decl_index, &FfiAppExit::SUCCESS)
    }

    #[inline]
    fn send_app_exit_error(&mut self, decl_index: usize, code: u8) -> FfiResult {
        self.send_message(decl_index, &FfiAppExit::error(code))
    }
}
