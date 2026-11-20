//! Guest-side `#![forbid(unsafe_code)]`, zero-Bevy bridge for Bevy `0.20` `bevy_picking`
//! flattened observer events (`PointerPress`, `PointerRelease`, `PointerMove`, `PointerClick`,
//! `PointerOver`, `PointerOut`, `PointerDrag`) and `EntityCursor` matching
//! `bamabi_bridges::picking` (`ModPickingBridgePlugin`) bit-for-bit.

use bamabi_sys::{AccessMode, FfiEntity, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    FfiEntityCursor, FfiPointerButton, FfiPointerClick, FfiPointerDrag, FfiPointerKind,
    FfiPointerMove, FfiPointerOut, FfiPointerOver, FfiPointerPress, FfiPointerRelease,
};

use crate::{
    app::{GuestApp, QueryBuilder, TypedObserverBuilder},
    commands::GuestCommands,
    component::GuestComponent,
};

/// Canonical ABI name for bridged `PointerPress` (`FfiPointerPress`).
pub const CANONICAL_POINTER_PRESS_NAME: &str = "bevy_picking::events::PointerPress";
/// Canonical ABI name for bridged `PointerRelease` (`FfiPointerRelease`).
pub const CANONICAL_POINTER_RELEASE_NAME: &str = "bevy_picking::events::PointerRelease";
/// Canonical ABI name for bridged `PointerMove` (`FfiPointerMove`).
pub const CANONICAL_POINTER_MOVE_NAME: &str = "bevy_picking::events::PointerMove";
/// Canonical ABI name for bridged `PointerClick` (`FfiPointerClick`).
pub const CANONICAL_POINTER_CLICK_NAME: &str = "bevy_picking::events::PointerClick";
/// Canonical ABI name for bridged `PointerOver` (`FfiPointerOver`).
pub const CANONICAL_POINTER_OVER_NAME: &str = "bevy_picking::events::PointerOver";
/// Canonical ABI name for bridged `PointerOut` (`FfiPointerOut`).
pub const CANONICAL_POINTER_OUT_NAME: &str = "bevy_picking::events::PointerOut";
/// Canonical ABI name for bridged `PointerDrag` (`FfiPointerDrag`).
pub const CANONICAL_POINTER_DRAG_NAME: &str = "bevy_picking::events::PointerDrag";
/// Canonical ABI name for bridged `EntityCursor` (`FfiEntityCursor`).
pub const CANONICAL_ENTITY_CURSOR_NAME: &str = "bevy_picking::cursor::EntityCursor";

impl GuestComponent for FfiPointerPress {
    const NAME: &'static CStr = c"bevy_picking::events::PointerPress";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerRelease {
    const NAME: &'static CStr = c"bevy_picking::events::PointerRelease";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerMove {
    const NAME: &'static CStr = c"bevy_picking::events::PointerMove";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerClick {
    const NAME: &'static CStr = c"bevy_picking::events::PointerClick";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerOver {
    const NAME: &'static CStr = c"bevy_picking::events::PointerOver";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerOut {
    const NAME: &'static CStr = c"bevy_picking::events::PointerOut";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointerDrag {
    const NAME: &'static CStr = c"bevy_picking::events::PointerDrag";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiEntityCursor {
    const NAME: &'static CStr = c"bevy_picking::cursor::EntityCursor";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Extension trait on [`GuestApp`] registering typed Bevy `0.20` flattened `bevy_picking`
/// pointer observers (`PointerPress`, `PointerRelease`, `PointerMove`, `PointerClick`,
/// `PointerOver`, `PointerOut`, `PointerDrag`).
pub trait GuestAppPickingExt<'a> {
    /// Creates a [`TypedObserverBuilder`] for `PointerPress` events.
    fn observe_pointer_press(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerPress>;
    /// Creates a [`TypedObserverBuilder`] for `PointerRelease` events.
    fn observe_pointer_release(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerRelease>;
    /// Creates a [`TypedObserverBuilder`] for `PointerMove` events.
    fn observe_pointer_move(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerMove>;
    /// Creates a [`TypedObserverBuilder`] for `PointerClick` events.
    fn observe_pointer_click(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerClick>;
    /// Creates a [`TypedObserverBuilder`] for `PointerOver` events.
    fn observe_pointer_over(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerOver>;
    /// Creates a [`TypedObserverBuilder`] for `PointerOut` events.
    fn observe_pointer_out(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerOut>;
    /// Creates a [`TypedObserverBuilder`] for `PointerDrag` events.
    fn observe_pointer_drag(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerDrag>;
}

impl<'a> GuestAppPickingExt<'a> for GuestApp<'a> {
    #[inline]
    fn observe_pointer_press(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerPress> {
        self.observe_event::<FfiPointerPress>(name)
    }

    #[inline]
    fn observe_pointer_release(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerRelease> {
        self.observe_event::<FfiPointerRelease>(name)
    }

    #[inline]
    fn observe_pointer_move(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerMove> {
        self.observe_event::<FfiPointerMove>(name)
    }

    #[inline]
    fn observe_pointer_click(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerClick> {
        self.observe_event::<FfiPointerClick>(name)
    }

    #[inline]
    fn observe_pointer_over(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerOver> {
        self.observe_event::<FfiPointerOver>(name)
    }

    #[inline]
    fn observe_pointer_out(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerOut> {
        self.observe_event::<FfiPointerOut>(name)
    }

    #[inline]
    fn observe_pointer_drag(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiPointerDrag> {
        self.observe_event::<FfiPointerDrag>(name)
    }
}

/// Extension trait on [`QueryBuilder`] adding direct POD `FfiEntityCursor` query terms.
pub trait GuestPickingQueryBuilderExt {
    /// Appends a shared read (`&FfiEntityCursor`) term.
    fn read_entity_cursor(self) -> Self;
    /// Appends an exclusive write (`&mut FfiEntityCursor`) term.
    fn write_entity_cursor(self) -> Self;
}

impl GuestPickingQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_entity_cursor(self) -> Self {
        self.term_named(
            FfiEntityCursor::NAME,
            Some(FfiEntityCursor::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_entity_cursor(self) -> Self {
        self.term_named(
            FfiEntityCursor::NAME,
            Some(FfiEntityCursor::UUID),
            AccessMode::WRITE,
        )
    }
}

/// Extension trait on [`GuestCommands`] for inserting/removing Bevy `0.20`'s `EntityCursor`.
pub trait GuestCommandsPickingExt {
    /// Sets [`FfiEntityCursor`] on `entity`.
    fn set_entity_cursor(&mut self, entity: FfiEntity, cursor: FfiEntityCursor) -> FfiResult;
    /// Removes [`FfiEntityCursor`] from `entity`.
    fn remove_entity_cursor(&mut self, entity: FfiEntity) -> FfiResult;
}

impl GuestCommandsPickingExt for GuestCommands<'_> {
    #[inline]
    fn set_entity_cursor(&mut self, entity: FfiEntity, cursor: FfiEntityCursor) -> FfiResult {
        self.insert_component(entity, &cursor)
    }

    #[inline]
    fn remove_entity_cursor(&mut self, entity: FfiEntity) -> FfiResult {
        self.remove_component::<FfiEntityCursor>(entity)
    }
}
