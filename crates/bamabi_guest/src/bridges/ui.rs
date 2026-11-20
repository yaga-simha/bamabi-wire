//! Guest-side `#![forbid(unsafe_code)]`, zero-Bevy bridge for Bevy `0.20` UI, text, headless
//! widgets (`bevy_ui_widgets`), and `bevy_feathers` controls matching `bamabi_bridges::ui`
//! (`ModUiBridgePlugin`) bit-for-bit.

use bamabi_guest_sys::{Host, ValidatedIter, ValidatedStringVTable, resolve_component};
use bamabi_sys::{AccessMode, BamComponent, ComponentUuid, FfiEntity, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    FfiBorderColor, FfiBorderRadius, FfiCornerRadius, FfiEditableTextConfig, FfiEmSize,
    FfiFeathersColorInput, FfiFeathersDropdown, FfiFeathersListRow, FfiFeathersListView,
    FfiFeathersNumberInput, FfiFeathersScrollableList, FfiFeathersSelect, FfiFontSize,
    FfiInlineBox, FfiInlineImage, FfiRemSize, FfiRgba, FfiSelectedTab, FfiTab, FfiTabList,
    FfiThemeContext, FfiUiNode, FfiUiRect, FfiUiTextStyle, FfiVal, FfiValueChange,
};

use crate::{
    app::{GuestApp, QueryBuilder, TypedObserverBuilder},
    bridges::std_types::GuestWireBridge,
    commands::GuestCommands,
    component::GuestComponent,
    iter::{Chunk, ChunkIter},
};

/// Canonical ABI name for bridged `Text` (`HostStringBridge`, alias `"Text"`).
pub const CANONICAL_UI_TEXT_NAME: &str = "bevy_ui::widget::text::Text";
/// Canonical ABI name for bridged `TextSpan` (`HostStringBridge`, alias `"TextSpan"`).
pub const CANONICAL_TEXT_SPAN_NAME: &str = "bevy_text::text::TextSpan";
/// Canonical ABI name for bridged `BackgroundColor` (`HostWireBridge<FfiRgba>`, alias `"BackgroundColor"`).
pub const CANONICAL_BACKGROUND_COLOR_NAME: &str = "bevy_ui::ui_node::BackgroundColor";
/// Canonical ABI name for bridged `BorderColor` (`HostWireBridge<FfiBorderColor>`, alias `"BorderColor"`).
pub const CANONICAL_BORDER_COLOR_NAME: &str = "bevy_ui::ui_node::BorderColor";
/// Canonical ABI name for bridged `RemSize` resource (`HostWireBridge<FfiRemSize>`, alias `"RemSize"`).
pub const CANONICAL_REM_SIZE_NAME: &str = "bevy_text::text::RemSize";
/// Canonical ABI name for bridged `EmSize` component (`HostWireBridge<FfiEmSize>`, alias `"EmSize"`).
pub const CANONICAL_EM_SIZE_NAME: &str = "bevy_text::text::EmSize";
/// Canonical ABI name for bridged `Activate` entity event (`"bevy_ui_widgets::Activate"`).
pub const CANONICAL_ACTIVATE_EVENT_NAME: &str = "bevy_ui_widgets::Activate";
/// Deterministic [`ComponentUuid`] matching `ModUiBridgePlugin`'s
/// `register_bridged_entity_event_with::<Activate>` registration
/// (`size = size_of::<FfiEntity>()`, `align = align_of::<FfiEntity>()`, `schema = 0`).
pub const ACTIVATE_EVENT_UUID: ComponentUuid = ComponentUuid::from_parts(
    CANONICAL_ACTIVATE_EVENT_NAME,
    core::mem::size_of::<FfiEntity>(),
    core::mem::align_of::<FfiEntity>(),
    0,
);
/// Canonical ABI name for direct POD `FfiValueChange` entity event (`"bamabi::ui::FfiValueChange"`).
pub const CANONICAL_VALUE_CHANGE_EVENT_NAME: &str = "bamabi::ui::FfiValueChange";
/// Canonical ABI name for bridged `ThemeContext` (`HostWireBridge<FfiThemeContext>`, alias `"ThemeContext"`).
pub const CANONICAL_THEME_CONTEXT_NAME: &str = "bevy_feathers::theme::ThemeContext";

/// Canonical ABI name for direct POD [`FfiUiNode`].
pub const CANONICAL_UI_NODE_NAME: &str = "bamabi::ui::FfiUiNode";
/// Canonical ABI name for direct POD [`FfiBorderRadius`].
pub const CANONICAL_BORDER_RADIUS_NAME: &str = "bamabi::ui::FfiBorderRadius";
/// Canonical ABI name for direct POD [`FfiUiTextStyle`].
pub const CANONICAL_UI_TEXT_STYLE_NAME: &str = "bamabi::ui::FfiUiTextStyle";
/// Canonical ABI name for direct POD [`FfiEditableTextConfig`].
pub const CANONICAL_EDITABLE_TEXT_CONFIG_NAME: &str = "bamabi::ui::FfiEditableTextConfig";
/// Canonical ABI name for direct POD [`FfiInlineBox`].
pub const CANONICAL_INLINE_BOX_NAME: &str = "bamabi::ui::FfiInlineBox";
/// Canonical ABI name for direct POD [`FfiInlineImage`].
pub const CANONICAL_INLINE_IMAGE_NAME: &str = "bamabi::ui::FfiInlineImage";
/// Canonical ABI name for direct POD [`FfiTabList`].
pub const CANONICAL_TAB_LIST_NAME: &str = "bamabi::ui::FfiTabList";
/// Canonical ABI name for direct POD [`FfiTab`].
pub const CANONICAL_TAB_NAME: &str = "bamabi::ui::FfiTab";
/// Canonical ABI name for direct POD [`FfiSelectedTab`].
pub const CANONICAL_SELECTED_TAB_NAME: &str = "bamabi::ui::FfiSelectedTab";
/// Canonical ABI name for direct POD [`FfiThemeContext`].
pub const CANONICAL_FFI_THEME_CONTEXT_NAME: &str = "bamabi::ui::FfiThemeContext";
/// Canonical ABI name for direct POD [`FfiFeathersColorInput`].
pub const CANONICAL_FEATHERS_COLOR_INPUT_NAME: &str = "bamabi::ui::FfiFeathersColorInput";
/// Canonical ABI name for direct POD [`FfiFeathersDropdown`] / [`FfiFeathersSelect`].
pub const CANONICAL_FEATHERS_DROPDOWN_NAME: &str = "bamabi::ui::FfiFeathersDropdown";
/// Canonical ABI name for direct POD [`FfiFeathersScrollableList`] / [`FfiFeathersListView`].
pub const CANONICAL_FEATHERS_SCROLLABLE_LIST_NAME: &str = "bamabi::ui::FfiFeathersScrollableList";
/// Canonical ABI name for direct POD [`FfiFeathersListRow`].
pub const CANONICAL_FEATHERS_LIST_ROW_NAME: &str = "bamabi::ui::FfiFeathersListRow";
/// Canonical ABI name for direct POD [`FfiFeathersNumberInput`].
pub const CANONICAL_FEATHERS_NUMBER_INPUT_NAME: &str = "bamabi::ui::FfiFeathersNumberInput";

impl GuestComponent for FfiVal {
    const NAME: &'static CStr = c"bamabi::ui::FfiVal";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiUiRect {
    const NAME: &'static CStr = c"bamabi::ui::FfiUiRect";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiCornerRadius {
    const NAME: &'static CStr = c"bamabi::ui::FfiCornerRadius";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiBorderRadius {
    const NAME: &'static CStr = c"bamabi::ui::FfiBorderRadius";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiRgba {
    const NAME: &'static CStr = c"bamabi::ui::FfiRgba";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiBorderColor {
    const NAME: &'static CStr = c"bamabi::ui::FfiBorderColor";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiUiNode {
    const NAME: &'static CStr = c"bamabi::ui::FfiUiNode";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFontSize {
    const NAME: &'static CStr = c"bamabi::ui::FfiFontSize";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiUiTextStyle {
    const NAME: &'static CStr = c"bamabi::ui::FfiUiTextStyle";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiEditableTextConfig {
    const NAME: &'static CStr = c"bamabi::ui::FfiEditableTextConfig";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiInlineBox {
    const NAME: &'static CStr = c"bamabi::ui::FfiInlineBox";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiInlineImage {
    const NAME: &'static CStr = c"bamabi::ui::FfiInlineImage";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiTabList {
    const NAME: &'static CStr = c"bamabi::ui::FfiTabList";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiTab {
    const NAME: &'static CStr = c"bamabi::ui::FfiTab";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiSelectedTab {
    const NAME: &'static CStr = c"bamabi::ui::FfiSelectedTab";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiValueChange {
    const NAME: &'static CStr = c"bamabi::ui::FfiValueChange";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiThemeContext {
    const NAME: &'static CStr = c"bamabi::ui::FfiThemeContext";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFeathersColorInput {
    const NAME: &'static CStr = c"bamabi::ui::FfiFeathersColorInput";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFeathersDropdown {
    const NAME: &'static CStr = c"bamabi::ui::FfiFeathersDropdown";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFeathersScrollableList {
    const NAME: &'static CStr = c"bamabi::ui::FfiFeathersScrollableList";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFeathersListRow {
    const NAME: &'static CStr = c"bamabi::ui::FfiFeathersListRow";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiFeathersNumberInput {
    const NAME: &'static CStr = c"bamabi::ui::FfiFeathersNumberInput";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Safe guest wrapper around a `HostStringBridge` `MappedStringVTable` (used for `Text` and
/// `TextSpan` registered by `ModUiBridgePlugin`).
#[derive(Clone, Copy)]
pub struct GuestMappedStringBridge {
    vtable: ValidatedStringVTable,
}

impl GuestMappedStringBridge {
    /// Resolves a host `MappedStringVTable` by primary canonical symbol with optional short alias.
    pub fn resolve(
        host: &Host,
        canonical_name: &str,
        primary_symbol: &CStr,
        fallback_symbol: Option<&CStr>,
    ) -> Result<Self, FfiResult> {
        let uuid = ComponentUuid::from_canonical_name(canonical_name);
        let vtable = match host.resolve_mapped_string_vtable(primary_symbol, uuid) {
            Ok(vt) => vt,
            Err(err) => match fallback_symbol {
                Some(fb) => host.resolve_mapped_string_vtable(fb, uuid)?,
                None => return Err(err),
            },
        };
        Ok(Self { vtable })
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_ui::widget::text::Text` string VTable.
    pub fn resolve_ui_text(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_UI_TEXT_NAME,
            c"bam.v0.bevy_ui::widget::text::Text.vtable",
            Some(c"bam.v0.Text.vtable"),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_text::text::TextSpan` string VTable.
    pub fn resolve_text_span(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_TEXT_SPAN_NAME,
            c"bam.v0.bevy_text::text::TextSpan.vtable",
            Some(c"bam.v0.TextSpan.vtable"),
        )
    }

    /// Returns the underlying validated [`ValidatedStringVTable`].
    #[inline]
    pub const fn vtable(&self) -> &ValidatedStringVTable {
        &self.vtable
    }

    /// Reads the UTF-8 text at `(col, row)` of `chunk`.
    #[inline]
    pub fn read_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        chunk.iter.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `(col, row)` of `chunk`.
    #[inline]
    pub fn write_chunk_row(
        &self,
        chunk: &mut Chunk<'_, '_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        chunk
            .iter
            .write_mapped_string_row(col, row, &self.vtable, text)
    }

    /// Reads the UTF-8 text at `(col, row)` of the active chunk in `iter`.
    #[inline]
    pub fn read_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        iter.inner.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `(col, row)` of the active chunk in `iter`.
    #[inline]
    pub fn write_row(
        &self,
        iter: &mut ChunkIter<'_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        iter.inner
            .write_mapped_string_row(col, row, &self.vtable, text)
    }

    /// Reads the UTF-8 text at `(col, row)` directly from a [`ValidatedIter`].
    #[inline]
    pub fn read_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
    ) -> Result<Option<String>, FfiResult> {
        iter.read_mapped_string_row(col, row, &self.vtable)
    }

    /// Writes `text` into `(col, row)` directly on a [`ValidatedIter`].
    #[inline]
    pub fn write_validated_row(
        &self,
        iter: &mut ValidatedIter<'_>,
        col: usize,
        row: usize,
        text: &str,
    ) -> FfiResult {
        iter.write_mapped_string_row(col, row, &self.vtable, text)
    }
}

/// Guest string bridge for `bevy_ui::widget::text::Text`.
pub type GuestUiTextBridge = GuestMappedStringBridge;
/// Guest string bridge for `bevy_text::text::TextSpan`.
pub type GuestTextSpanBridge = GuestMappedStringBridge;

/// Guest wire bridge for `bevy_ui::ui_node::BackgroundColor` (`MappedWireVTable<FfiRgba>`).
pub type GuestBackgroundColorBridge = GuestWireBridge<FfiRgba>;
/// Guest wire bridge for `bevy_ui::ui_node::BorderColor` (`MappedWireVTable<FfiBorderColor>`).
pub type GuestBorderColorBridge = GuestWireBridge<FfiBorderColor>;
/// Guest wire bridge for `bevy_text::text::RemSize` resource (`MappedWireVTable<FfiRemSize>`).
pub type GuestRemSizeBridge = GuestWireBridge<FfiRemSize>;
/// Guest wire bridge for `bevy_text::text::EmSize` component (`MappedWireVTable<FfiEmSize>`).
pub type GuestEmSizeBridge = GuestWireBridge<FfiEmSize>;
/// Guest wire bridge for `bevy_feathers::theme::ThemeContext` (`MappedWireVTable<FfiThemeContext>`).
pub type GuestThemeContextBridge = GuestWireBridge<FfiThemeContext>;

impl GuestBackgroundColorBridge {
    /// Resolves `ModUiBridgePlugin`'s `BackgroundColor` VTable (`"bam.v0.BackgroundColor.vtable"`).
    pub fn resolve_background_color(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_BACKGROUND_COLOR_NAME,
            c"bam.v0.bevy_ui::ui_node::BackgroundColor.vtable",
            Some(c"bam.v0.BackgroundColor.vtable"),
        )
    }
}

impl GuestBorderColorBridge {
    /// Resolves `ModUiBridgePlugin`'s `BorderColor` VTable (`"bam.v0.BorderColor.vtable"`).
    pub fn resolve_border_color(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_BORDER_COLOR_NAME,
            c"bam.v0.bevy_ui::ui_node::BorderColor.vtable",
            Some(c"bam.v0.BorderColor.vtable"),
        )
    }
}

impl GuestRemSizeBridge {
    /// Resolves `ModUiBridgePlugin`'s `RemSize` resource VTable (`"bam.v0.RemSize.vtable"`).
    pub fn resolve_rem_size(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_REM_SIZE_NAME,
            c"bam.v0.bevy_text::text::RemSize.vtable",
            Some(c"bam.v0.RemSize.vtable"),
        )
    }
}

impl GuestEmSizeBridge {
    /// Resolves `ModUiBridgePlugin`'s `EmSize` component VTable (`"bam.v0.EmSize.vtable"`).
    pub fn resolve_em_size(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_EM_SIZE_NAME,
            c"bam.v0.bevy_text::text::EmSize.vtable",
            Some(c"bam.v0.EmSize.vtable"),
        )
    }
}

impl GuestThemeContextBridge {
    /// Resolves `ModUiBridgePlugin`'s `ThemeContext` VTable (`"bam.v0.ThemeContext.vtable"`).
    pub fn resolve_theme_context(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_THEME_CONTEXT_NAME,
            c"bam.v0.bevy_feathers::theme::ThemeContext.vtable",
            Some(c"bam.v0.ThemeContext.vtable"),
        )
    }
}

#[inline]
fn resolve_bridged_ui_handle(
    host: &Host,
    canonical_cstr: &CStr,
    alias_cstr: &CStr,
    uuid: ComponentUuid,
) -> Result<BamComponent, FfiResult> {
    match resolve_component(host, Some(canonical_cstr), Some(uuid)) {
        Ok(handle) => Ok(handle),
        Err(FfiResult::NOT_FOUND) => resolve_component(host, Some(alias_cstr), Some(uuid)),
        Err(err) => Err(err),
    }
}

/// Pre-resolved [`BamComponent`] handles for host-bridged Bevy `0.20` UI components and events
/// registered by `ModUiBridgePlugin` (`default_inserter_for` + `bevy_ui_widgets::Activate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestUiHandles {
    /// Handle for bridged `bevy_ui::widget::text::Text` (`default_inserter` supported).
    pub text: BamComponent,
    /// Handle for bridged `bevy_text::text::TextSpan` (`default_inserter` supported).
    pub text_span: BamComponent,
    /// Handle for bridged `bevy_ui::ui_node::BackgroundColor` (`default_inserter` supported).
    pub background_color: BamComponent,
    /// Handle for bridged `bevy_ui::ui_node::BorderColor` (`default_inserter` supported).
    pub border_color: BamComponent,
    /// Handle for bridged `bevy_text::text::EmSize` (`default_inserter` supported).
    pub em_size: BamComponent,
    /// Optional handle for bridged `bevy_feathers::theme::ThemeContext` (`default_inserter`
    /// supported; `None` when the host is built without `bridge-feathers`).
    pub theme_context: Option<BamComponent>,
    /// Handle for bridged `bevy_ui_widgets::Activate` entity event.
    pub activate: BamComponent,
}

impl GuestUiHandles {
    /// Resolves all bridged UI component handles and the `Activate` entity event from `host`.
    ///
    /// `theme_context` is resolved optionally (`None` if the host did not enable `bridge-feathers`),
    /// while non-`NOT_FOUND` errors (such as `TYPE_MISMATCH`) propagate immediately.
    pub fn resolve(host: &Host) -> Result<Self, FfiResult> {
        let text = Self::resolve_text(host)?;
        let text_span = Self::resolve_text_span(host)?;
        let background_color = Self::resolve_background_color(host)?;
        let border_color = Self::resolve_border_color(host)?;
        let em_size = Self::resolve_em_size(host)?;
        let theme_context = match Self::resolve_theme_context(host) {
            Ok(handle) => Some(handle),
            Err(FfiResult::NOT_FOUND) => None,
            Err(err) => return Err(err),
        };
        let activate = Self::resolve_activate(host)?;
        Ok(Self {
            text,
            text_span,
            background_color,
            border_color,
            em_size,
            theme_context,
            activate,
        })
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_ui::widget::text::Text` handle.
    #[inline]
    pub fn resolve_text(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_ui::widget::text::Text",
            c"Text",
            ComponentUuid::from_canonical_name(CANONICAL_UI_TEXT_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_text::text::TextSpan` handle.
    #[inline]
    pub fn resolve_text_span(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_text::text::TextSpan",
            c"TextSpan",
            ComponentUuid::from_canonical_name(CANONICAL_TEXT_SPAN_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_ui::ui_node::BackgroundColor` handle.
    #[inline]
    pub fn resolve_background_color(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_ui::ui_node::BackgroundColor",
            c"BackgroundColor",
            ComponentUuid::from_canonical_name(CANONICAL_BACKGROUND_COLOR_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_ui::ui_node::BorderColor` handle.
    #[inline]
    pub fn resolve_border_color(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_ui::ui_node::BorderColor",
            c"BorderColor",
            ComponentUuid::from_canonical_name(CANONICAL_BORDER_COLOR_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_text::text::RemSize` resource handle.
    #[inline]
    pub fn resolve_rem_size(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_text::text::RemSize",
            c"RemSize",
            ComponentUuid::from_canonical_name(CANONICAL_REM_SIZE_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_text::text::EmSize` handle.
    #[inline]
    pub fn resolve_em_size(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_text::text::EmSize",
            c"EmSize",
            ComponentUuid::from_canonical_name(CANONICAL_EM_SIZE_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s optional `bevy_feathers::theme::ThemeContext` handle.
    #[inline]
    pub fn resolve_theme_context(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_feathers::theme::ThemeContext",
            c"ThemeContext",
            ComponentUuid::from_canonical_name(CANONICAL_THEME_CONTEXT_NAME),
        )
    }

    /// Resolves `ModUiBridgePlugin`'s `bevy_ui_widgets::Activate` entity event handle.
    #[inline]
    pub fn resolve_activate(host: &Host) -> Result<BamComponent, FfiResult> {
        resolve_bridged_ui_handle(
            host,
            c"bevy_ui_widgets::Activate",
            c"Activate",
            ACTIVATE_EVENT_UUID,
        )
    }
}

/// Extension trait on [`GuestApp`] registering Bevy `0.20` UI widget observers (`FfiValueChange`
/// and `Activate`).
pub trait GuestAppUiExt<'a> {
    /// Creates a [`TypedObserverBuilder`] for unified headless widget `FfiValueChange` events.
    fn observe_value_change(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiValueChange>;

    /// Creates a [`TypedObserverBuilder`] for `bevy_ui_widgets::Activate` entity events (payload
    /// is the target `FfiEntity` under `register_bridged_entity_event_with`).
    fn observe_activate(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiEntity>;
}

impl<'a> GuestAppUiExt<'a> for GuestApp<'a> {
    #[inline]
    fn observe_value_change(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiValueChange> {
        self.observe_event::<FfiValueChange>(name)
    }

    fn observe_activate(
        &mut self,
        name: impl Into<String>,
    ) -> TypedObserverBuilder<'_, 'a, FfiEntity> {
        match GuestUiHandles::resolve_activate(self.host()) {
            Ok(handle) => self.observe_event_handle::<FfiEntity>(name, handle),
            Err(err) => TypedObserverBuilder::with_error(self, name, err),
        }
    }
}

/// Extension trait on [`QueryBuilder`] adding Bevy `0.20` UI and Feathers query terms.
pub trait GuestUiQueryBuilderExt {
    /// Appends a shared read (`&FfiUiNode`) term.
    fn read_ui_node(self) -> Self;
    /// Appends an exclusive write (`&mut FfiUiNode`) term.
    fn write_ui_node(self) -> Self;
    /// Appends a shared read (`&FfiUiTextStyle`) term.
    fn read_ui_text_style(self) -> Self;
    /// Appends an exclusive write (`&mut FfiUiTextStyle`) term.
    fn write_ui_text_style(self) -> Self;
    /// Appends a shared read (`&Text` string-bridged) term.
    fn read_ui_text(self) -> Self;
    /// Appends an exclusive write (`&mut Text` string-bridged) term.
    fn write_ui_text(self) -> Self;
    /// Appends a shared read (`&TextSpan` string-bridged) term.
    fn read_text_span(self) -> Self;
    /// Appends an exclusive write (`&mut TextSpan` string-bridged) term.
    fn write_text_span(self) -> Self;
    /// Appends a shared read (`&BackgroundColor` wire-bridged) term.
    fn read_background_color(self) -> Self;
    /// Appends an exclusive write (`&mut BackgroundColor` wire-bridged) term.
    fn write_background_color(self) -> Self;
    /// Appends a shared read (`&BorderColor` wire-bridged) term.
    fn read_border_color(self) -> Self;
    /// Appends an exclusive write (`&mut BorderColor` wire-bridged) term.
    fn write_border_color(self) -> Self;
    /// Appends a shared resource read (`Res<RemSize>` wire-bridged) term.
    fn read_rem_size(self) -> Self;
    /// Appends an exclusive resource write (`ResMut<RemSize>` wire-bridged) term.
    fn write_rem_size(self) -> Self;
    /// Appends a shared read (`&EmSize` wire-bridged) term.
    fn read_em_size(self) -> Self;
    /// Appends an exclusive write (`&mut EmSize` wire-bridged) term.
    fn write_em_size(self) -> Self;
    /// Appends a shared read (`&FfiThemeContext` POD mirror) term.
    fn read_ffi_theme_context(self) -> Self;
    /// Appends a shared read (`&ThemeContext` native wire-bridged) term.
    fn read_theme_context(self) -> Self;
    /// Appends an exclusive write (`&mut ThemeContext` native wire-bridged) term.
    fn write_theme_context(self) -> Self;
}

impl GuestUiQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_ui_node(self) -> Self {
        self.term_named(FfiUiNode::NAME, Some(FfiUiNode::UUID), AccessMode::READ)
    }

    #[inline]
    fn write_ui_node(self) -> Self {
        self.term_named(FfiUiNode::NAME, Some(FfiUiNode::UUID), AccessMode::WRITE)
    }

    #[inline]
    fn read_ui_text_style(self) -> Self {
        self.term_named(
            FfiUiTextStyle::NAME,
            Some(FfiUiTextStyle::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_ui_text_style(self) -> Self {
        self.term_named(
            FfiUiTextStyle::NAME,
            Some(FfiUiTextStyle::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_ui_text(self) -> Self {
        self.term_named(
            c"bevy_ui::widget::text::Text",
            Some(ComponentUuid::from_canonical_name(CANONICAL_UI_TEXT_NAME)),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_ui_text(self) -> Self {
        self.term_named(
            c"bevy_ui::widget::text::Text",
            Some(ComponentUuid::from_canonical_name(CANONICAL_UI_TEXT_NAME)),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_text_span(self) -> Self {
        self.term_named(
            c"bevy_text::text::TextSpan",
            Some(ComponentUuid::from_canonical_name(CANONICAL_TEXT_SPAN_NAME)),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_text_span(self) -> Self {
        self.term_named(
            c"bevy_text::text::TextSpan",
            Some(ComponentUuid::from_canonical_name(CANONICAL_TEXT_SPAN_NAME)),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_background_color(self) -> Self {
        self.term_named(
            c"bevy_ui::ui_node::BackgroundColor",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_BACKGROUND_COLOR_NAME,
            )),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_background_color(self) -> Self {
        self.term_named(
            c"bevy_ui::ui_node::BackgroundColor",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_BACKGROUND_COLOR_NAME,
            )),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_border_color(self) -> Self {
        self.term_named(
            c"bevy_ui::ui_node::BorderColor",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_BORDER_COLOR_NAME,
            )),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_border_color(self) -> Self {
        self.term_named(
            c"bevy_ui::ui_node::BorderColor",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_BORDER_COLOR_NAME,
            )),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_rem_size(self) -> Self {
        self.term_named(
            c"bevy_text::text::RemSize",
            Some(ComponentUuid::from_canonical_name(CANONICAL_REM_SIZE_NAME)),
            AccessMode::RESOURCE_READ,
        )
    }

    #[inline]
    fn write_rem_size(self) -> Self {
        self.term_named(
            c"bevy_text::text::RemSize",
            Some(ComponentUuid::from_canonical_name(CANONICAL_REM_SIZE_NAME)),
            AccessMode::RESOURCE_WRITE,
        )
    }

    #[inline]
    fn read_em_size(self) -> Self {
        self.term_named(
            c"bevy_text::text::EmSize",
            Some(ComponentUuid::from_canonical_name(CANONICAL_EM_SIZE_NAME)),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_em_size(self) -> Self {
        self.term_named(
            c"bevy_text::text::EmSize",
            Some(ComponentUuid::from_canonical_name(CANONICAL_EM_SIZE_NAME)),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_ffi_theme_context(self) -> Self {
        self.term_named(
            FfiThemeContext::NAME,
            Some(FfiThemeContext::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn read_theme_context(self) -> Self {
        self.term_named(
            c"bevy_feathers::theme::ThemeContext",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_THEME_CONTEXT_NAME,
            )),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_theme_context(self) -> Self {
        self.term_named(
            c"bevy_feathers::theme::ThemeContext",
            Some(ComponentUuid::from_canonical_name(
                CANONICAL_THEME_CONTEXT_NAME,
            )),
            AccessMode::WRITE,
        )
    }
}

/// Extension trait on [`GuestCommands`] for inserting Bevy `0.20` UI, text, headless widget, and
/// `bevy_feathers` components.
pub trait GuestCommandsUiExt {
    /// Inserts [`FfiUiNode`] onto `entity`.
    fn insert_ui_node(&mut self, entity: FfiEntity, node: &FfiUiNode) -> FfiResult;
    /// Inserts [`FfiBorderRadius`] onto `entity`.
    fn insert_border_radius(&mut self, entity: FfiEntity, radius: &FfiBorderRadius) -> FfiResult;
    /// Inserts [`FfiUiTextStyle`] onto `entity`.
    fn insert_ui_text_style(&mut self, entity: FfiEntity, style: &FfiUiTextStyle) -> FfiResult;
    /// Inserts [`FfiEditableTextConfig`] onto `entity`.
    fn insert_editable_text(
        &mut self,
        entity: FfiEntity,
        config: &FfiEditableTextConfig,
    ) -> FfiResult;
    /// Inserts [`FfiInlineBox`] onto `entity`.
    fn insert_inline_box(&mut self, entity: FfiEntity, inline_box: &FfiInlineBox) -> FfiResult;
    /// Inserts [`FfiInlineImage`] onto `entity`.
    fn insert_inline_image(
        &mut self,
        entity: FfiEntity,
        inline_image: &FfiInlineImage,
    ) -> FfiResult;
    /// Inserts [`FfiTabList`] onto `entity`.
    fn insert_tab_list(&mut self, entity: FfiEntity, tab_list: &FfiTabList) -> FfiResult;
    /// Inserts [`FfiTab`] onto `entity`.
    fn insert_tab(&mut self, entity: FfiEntity, tab: &FfiTab) -> FfiResult;
    /// Inserts [`FfiSelectedTab`] onto `entity`.
    fn insert_selected_tab(&mut self, entity: FfiEntity, selected: &FfiSelectedTab) -> FfiResult;
    /// Inserts [`FfiThemeContext`] onto `entity`.
    fn insert_theme_context(&mut self, entity: FfiEntity, theme: FfiThemeContext) -> FfiResult;
    /// Inserts [`FfiFeathersColorInput`] onto `entity`.
    fn insert_feathers_color_input(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersColorInput,
    ) -> FfiResult;
    /// Inserts [`FfiFeathersDropdown`] onto `entity`.
    fn insert_feathers_dropdown(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersDropdown,
    ) -> FfiResult;
    /// Inserts [`FfiFeathersScrollableList`] onto `entity`.
    fn insert_feathers_scrollable_list(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersScrollableList,
    ) -> FfiResult;
    /// Inserts [`FfiFeathersListRow`] onto `entity`.
    fn insert_feathers_list_row(
        &mut self,
        entity: FfiEntity,
        row: &FfiFeathersListRow,
    ) -> FfiResult;
    /// Inserts [`FfiFeathersNumberInput`] onto `entity`.
    fn insert_feathers_number_input(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersNumberInput,
    ) -> FfiResult;
    /// Queues host-defaulted `bevy_ui::widget::text::Text::default()` insertion onto `entity`.
    fn insert_default_ui_text(&mut self, entity: FfiEntity, handles: &GuestUiHandles) -> FfiResult;
    /// Queues host-defaulted `bevy_text::text::TextSpan::default()` insertion onto `entity`.
    fn insert_default_text_span(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult;
    /// Queues host-defaulted `bevy_ui::ui_node::BackgroundColor::default()` insertion onto `entity`.
    fn insert_default_background_color(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult;
    /// Queues host-defaulted `bevy_ui::ui_node::BorderColor::default()` insertion onto `entity`.
    fn insert_default_border_color(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult;
    /// Queues host-defaulted `bevy_text::text::EmSize::default()` insertion onto `entity`.
    fn insert_default_em_size(&mut self, entity: FfiEntity, handles: &GuestUiHandles) -> FfiResult;
    /// Queues host-defaulted `bevy_feathers::theme::ThemeContext::default()` insertion onto `entity`,
    /// returning [`FfiResult::NOT_FOUND`] if `handles.theme_context` is `None`.
    fn insert_default_theme_context(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult;
    /// Resolves `ModUiBridgePlugin`'s `bevy_ui_widgets::Activate` event and triggers it on `target`.
    fn trigger_activate(&mut self, target: FfiEntity) -> Result<(), FfiResult>;
    /// Triggers `bevy_ui_widgets::Activate` on `target` using pre-resolved `handles.activate`.
    fn trigger_activate_with(
        &mut self,
        handles: &GuestUiHandles,
        target: FfiEntity,
    ) -> Result<(), FfiResult>;
}

impl GuestCommandsUiExt for GuestCommands<'_> {
    #[inline]
    fn insert_ui_node(&mut self, entity: FfiEntity, node: &FfiUiNode) -> FfiResult {
        self.insert_component(entity, node)
    }

    #[inline]
    fn insert_border_radius(&mut self, entity: FfiEntity, radius: &FfiBorderRadius) -> FfiResult {
        self.insert_component(entity, radius)
    }

    #[inline]
    fn insert_ui_text_style(&mut self, entity: FfiEntity, style: &FfiUiTextStyle) -> FfiResult {
        self.insert_component(entity, style)
    }

    #[inline]
    fn insert_editable_text(
        &mut self,
        entity: FfiEntity,
        config: &FfiEditableTextConfig,
    ) -> FfiResult {
        self.insert_component(entity, config)
    }

    #[inline]
    fn insert_inline_box(&mut self, entity: FfiEntity, inline_box: &FfiInlineBox) -> FfiResult {
        self.insert_component(entity, inline_box)
    }

    #[inline]
    fn insert_inline_image(
        &mut self,
        entity: FfiEntity,
        inline_image: &FfiInlineImage,
    ) -> FfiResult {
        self.insert_component(entity, inline_image)
    }

    #[inline]
    fn insert_tab_list(&mut self, entity: FfiEntity, tab_list: &FfiTabList) -> FfiResult {
        self.insert_component(entity, tab_list)
    }

    #[inline]
    fn insert_tab(&mut self, entity: FfiEntity, tab: &FfiTab) -> FfiResult {
        self.insert_component(entity, tab)
    }

    #[inline]
    fn insert_selected_tab(&mut self, entity: FfiEntity, selected: &FfiSelectedTab) -> FfiResult {
        self.insert_component(entity, selected)
    }

    #[inline]
    fn insert_theme_context(&mut self, entity: FfiEntity, theme: FfiThemeContext) -> FfiResult {
        self.insert_component(entity, &theme)
    }

    #[inline]
    fn insert_feathers_color_input(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersColorInput,
    ) -> FfiResult {
        self.insert_component(entity, widget)
    }

    #[inline]
    fn insert_feathers_dropdown(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersDropdown,
    ) -> FfiResult {
        self.insert_component(entity, widget)
    }

    #[inline]
    fn insert_feathers_scrollable_list(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersScrollableList,
    ) -> FfiResult {
        self.insert_component(entity, widget)
    }

    #[inline]
    fn insert_feathers_list_row(
        &mut self,
        entity: FfiEntity,
        row: &FfiFeathersListRow,
    ) -> FfiResult {
        self.insert_component(entity, row)
    }

    #[inline]
    fn insert_feathers_number_input(
        &mut self,
        entity: FfiEntity,
        widget: &FfiFeathersNumberInput,
    ) -> FfiResult {
        self.insert_component(entity, widget)
    }

    #[inline]
    fn insert_default_ui_text(&mut self, entity: FfiEntity, handles: &GuestUiHandles) -> FfiResult {
        self.insert_default(entity, handles.text)
    }

    #[inline]
    fn insert_default_text_span(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult {
        self.insert_default(entity, handles.text_span)
    }

    #[inline]
    fn insert_default_background_color(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult {
        self.insert_default(entity, handles.background_color)
    }

    #[inline]
    fn insert_default_border_color(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult {
        self.insert_default(entity, handles.border_color)
    }

    #[inline]
    fn insert_default_em_size(&mut self, entity: FfiEntity, handles: &GuestUiHandles) -> FfiResult {
        self.insert_default(entity, handles.em_size)
    }

    #[inline]
    fn insert_default_theme_context(
        &mut self,
        entity: FfiEntity,
        handles: &GuestUiHandles,
    ) -> FfiResult {
        match handles.theme_context {
            Some(handle) => self.insert_default(entity, handle),
            None => FfiResult::NOT_FOUND,
        }
    }

    #[inline]
    fn trigger_activate(&mut self, target: FfiEntity) -> Result<(), FfiResult> {
        if !self.is_valid() {
            return Err(FfiResult::NULL_POINTER);
        }
        if target.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        let handle = GuestUiHandles::resolve_activate(self.host())?;
        self.trigger_handle(handle, target, &target)
    }

    #[inline]
    fn trigger_activate_with(
        &mut self,
        handles: &GuestUiHandles,
        target: FfiEntity,
    ) -> Result<(), FfiResult> {
        if !self.is_valid() {
            return Err(FfiResult::NULL_POINTER);
        }
        if target.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        self.trigger_handle(handles.activate, target, &target)
    }
}
