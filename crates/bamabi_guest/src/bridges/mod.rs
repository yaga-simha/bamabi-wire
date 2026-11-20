//! Ergonomic `#![forbid(unsafe_code)]`, zero-Bevy guest wrappers for bridged Bevy `0.20` standard
//! components, resources, picking, camera/lighting, audio, UI/Feathers, 2D sprites/WESL materials,
//! windowing/AppExit, gizmos, isolated mod assets, state machines, reflection/BRP/telemetry, and
//! BSN scenes.

pub mod asset;
pub mod audio;
pub mod bsn;
pub mod camera_light;
pub mod gizmos;
pub mod picking;
pub mod reflect;
pub mod render;
pub mod state;
pub mod std_types;
pub mod ui;
pub mod window;

use bamabi_guest_sys::Host;
use bamabi_sys::FfiResult;

use crate::app::{GuestApp, SystemContext};

pub use asset::{
    FfiAssetEvent, FfiAssetEventKind, GuestAssetHandle, GuestAssetHostExt, GuestAssetServer,
    OVERLAY_NAMESPACE, TypedAssetHandle, mod_asset_path, mod_overlay_path,
};
pub use audio::{
    APPLY_EAR_POSITIONS, APPLY_EMITTER_POS, APPLY_SPEED, APPLY_VOLUME, AUDIO_SOURCE_ASSET_ALIAS,
    CANONICAL_AUDIO_SOURCE_NAME, FfiAudioPlaybackSettings, FfiAudioPlayerSpec,
    FfiSpatialAudioSinkControl, GuestAudioQueryBuilderExt, GuestCommandsAudioExt, MUTE_CMD_MUTE,
    MUTE_CMD_NONE, MUTE_CMD_TOGGLE, MUTE_CMD_UNMUTE, PLAYBACK_MODE_DESPAWN, PLAYBACK_MODE_LOOP,
    PLAYBACK_MODE_ONCE, PLAYBACK_MODE_REMOVE, SINK_CMD_NONE, SINK_CMD_PAUSE, SINK_CMD_PLAY,
    SINK_CMD_STOP, SINK_CMD_TOGGLE_PLAYBACK, VOLUME_DECIBELS, VOLUME_LINEAR,
    resolve_audio_source_type,
};
pub use bsn::{GuestCommandsBsnExt, GuestScene, resolve_scene_patch_type};
pub use camera_light::{
    CLEAR_COLOR_CUSTOM, CLEAR_COLOR_DEFAULT, CLEAR_COLOR_NONE, DEPTH_LOAD_OP_CLEAR,
    DEPTH_LOAD_OP_LOAD, FfiCamera2d, FfiCamera3d, FfiDirectionalLight, FfiPanOrbitCamera,
    FfiPointLight, FfiSolariLighting, FfiSpotLight, GuestCameraLightQueryBuilderExt,
    GuestCommandsCameraLightExt, ORBIT_CONSTRAINT_FIXED, ORBIT_CONSTRAINT_FREE,
    TONEMAPPING_ACES_FITTED, TONEMAPPING_AGX, TONEMAPPING_BLENDER_FILMIC,
    TONEMAPPING_KHRONOS_PBR_NEUTRAL, TONEMAPPING_LINEAR, TONEMAPPING_NONE, TONEMAPPING_REINHARD,
    TONEMAPPING_REINHARD_LUMINANCE, TONEMAPPING_SOMEWHAT_BORING_DISPLAY_TRANSFORM,
    TONEMAPPING_TONY_MC_MAPFACE, TRANSMISSION_QUALITY_HIGH, TRANSMISSION_QUALITY_LOW,
    TRANSMISSION_QUALITY_MEDIUM, TRANSMISSION_QUALITY_ULTRA,
};
pub use gizmos::{
    FfiGizmoArrow, FfiGizmoCircle, FfiGizmoCuboid, FfiGizmoLine, FfiGizmoRay, FfiGizmoRect,
    FfiGizmoSphere, GuestCommandsGizmosExt,
};
pub use picking::{
    FfiEntityCursor, FfiPointerButton, FfiPointerClick, FfiPointerDrag, FfiPointerKind,
    FfiPointerMove, FfiPointerOut, FfiPointerOver, FfiPointerPress, FfiPointerRelease,
    GuestAppPickingExt, GuestCommandsPickingExt, GuestPickingQueryBuilderExt,
};
pub use reflect::{
    BRP_METHOD_BAM_COMPONENT_SCHEMAS, BRP_METHOD_BAM_HOT_RELOAD, BRP_METHOD_BAM_LIST_MODS,
    BRP_METHOD_BAM_SYSTEM_METRICS, BRP_METHOD_GET_COMPONENTS, BRP_METHOD_GET_RESOURCES,
    BRP_METHOD_INSERT_COMPONENTS, BRP_METHOD_LIST_COMPONENTS, BRP_METHOD_MUTATE_COMPONENTS,
    BRP_METHOD_MUTATE_RESOURCES, BRP_METHOD_REGISTRY_SCHEMA, BRP_METHOD_RPC_DISCOVER,
    BRP_METHOD_WORLD_DESPAWN_ENTITY, BRP_METHOD_WORLD_GET_COMPONENTS,
    BRP_METHOD_WORLD_GET_RESOURCES, BRP_METHOD_WORLD_INSERT_COMPONENTS,
    BRP_METHOD_WORLD_INSERT_RESOURCES, BRP_METHOD_WORLD_LIST_COMPONENTS,
    BRP_METHOD_WORLD_LIST_RESOURCES, BRP_METHOD_WORLD_MUTATE_COMPONENTS,
    BRP_METHOD_WORLD_MUTATE_RESOURCES, BRP_METHOD_WORLD_QUERY, BRP_METHOD_WORLD_REMOVE_COMPONENTS,
    BRP_METHOD_WORLD_REMOVE_RESOURCES, BRP_METHOD_WORLD_SPAWN_ENTITY, SYSTEM_TELEMETRY_DIGEST_SIZE,
    SYSTEM_TELEMETRY_DIGEST_VERSION, SYSTEM_TELEMETRY_MAGIC, SystemTelemetryDigest64,
    TELEMETRY_FLAG_BUDGET_BREACHED, TELEMETRY_FLAG_FAULTED, TELEMETRY_FLAG_QUARANTINED,
    TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE, TelemetryFrameError, fnv1a_64_hash_system,
    resolve_reflected_handle, resolve_settings_handle,
};
pub use render::{
    FfiMesh2d, FfiMesh3d, FfiSprite, FfiSpriteMaterialSpec, FfiStandardMaterial,
    FfiWeslMaterialSpec, GuestCommandsRenderExt, GuestRenderQueryBuilderExt, GuestSpriteBridge,
    GuestStandardMaterialBridge,
};
pub use state::{
    FfiStateTransitionEvent, GuestCommandsStateExt, GuestStateBridge, GuestStateHandles,
    GuestStateQueryBuilderExt, GuestStateSlabContextExt, GuestStateSystemBuilderExt,
    MOD_STATE_FLAG_ACTIVE, MOD_STATE_FLAG_COMPUTED, MOD_STATE_FLAG_MUTABLE,
    MOD_STATE_FLAG_SUB_STATE, ModDespawnOnEnter, ModDespawnOnExit, ModStateEntry, ModStateHandle,
    ModStateTransitionRequest, STATE_ABSENT_DISCRIMINANT, on_enter_schedule, on_exit_schedule,
    on_transition_schedule, state_set_name,
};
pub use std_types::{
    FfiColor, FfiGamepadButton, FfiGamepadRumble, FfiGamepadState, FfiGlobalTransform,
    FfiInheritedVisibility, FfiKeyCode, FfiKeyboardInput, FfiMouseButton, FfiMouseInput,
    FfiMouseMotion, FfiMouseScroll, FfiTimeFixed, FfiTimeReal, FfiTimeVirtual, FfiTransform,
    FfiViewVisibility, FfiVisibility, GuestDisabledBridge, GuestGamepadRumbleExt,
    GuestGlobalTransformBridge, GuestInheritedVisibilityBridge, GuestKeyboardInputBridge,
    GuestMouseInputBridge, GuestNameBridge, GuestStdCommandsExt, GuestStdHandles,
    GuestStdQueryBuilderExt, GuestStdSystemBuilderExt, GuestStdTypes, GuestTimeFixedBridge,
    GuestTimeRealBridge, GuestTimeVirtualBridge, GuestTransformBridge, GuestViewVisibilityBridge,
    GuestVisibilityBridge, GuestWireBridge,
};
pub use ui::{
    ACTIVATE_EVENT_UUID, FfiBorderColor, FfiBorderRadius, FfiCornerRadius, FfiEditableTextConfig,
    FfiEmSize, FfiFeathersColorInput, FfiFeathersDropdown, FfiFeathersListRow, FfiFeathersListView,
    FfiFeathersNumberInput, FfiFeathersScrollableList, FfiFeathersSelect, FfiFontSize,
    FfiInlineBox, FfiInlineImage, FfiRemSize, FfiRgba, FfiSelectedTab, FfiTab, FfiTabList,
    FfiThemeContext, FfiUiNode, FfiUiRect, FfiUiTextStyle, FfiVal, FfiValueChange, GuestAppUiExt,
    GuestBackgroundColorBridge, GuestBorderColorBridge, GuestCommandsUiExt, GuestEmSizeBridge,
    GuestMappedStringBridge, GuestRemSizeBridge, GuestTextSpanBridge, GuestThemeContextBridge,
    GuestUiHandles, GuestUiQueryBuilderExt, GuestUiTextBridge,
};
pub use window::{
    FfiAppExit, FfiWindow, FfiWindowCloseRequested, FfiWindowMode, FfiWindowResized,
    GuestAppExitExt, GuestWindowBridge, GuestWindowQueryBuilderExt,
};

/// Ergonomic cross-bridge resolution helpers on [`Host`], [`GuestApp`], and [`SystemContext`].
pub trait GuestBridgesHostExt {
    /// Resolves the primary host-exposed state bridge (`slot = 0`).
    fn state_bridge(&self) -> Result<GuestStateBridge, FfiResult>;
    /// Resolves a host-exposed state bridge at `slot`.
    fn state_bridge_slot(&self, slot: usize) -> Result<GuestStateBridge, FfiResult>;
    /// Resolves the `Name` string bridge.
    fn std_name_bridge(&self) -> Result<GuestNameBridge, FfiResult>;
    /// Resolves all standard Bevy wire/string bridges (`Transform`, `GlobalTransform`,
    /// `Visibility`, `Name`, `Time<*>`, `ButtonInput<*>`).
    fn std_bridges(&self) -> Result<GuestStdTypes, FfiResult>;
    /// Resolves `StdTypesBridgePlugin`'s `Disabled` wire bridge.
    fn disabled_bridge(&self) -> Result<GuestDisabledBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `bevy_ui::widget::text::Text` string bridge.
    fn ui_text_bridge(&self) -> Result<GuestUiTextBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `bevy_text::text::TextSpan` string bridge.
    fn text_span_bridge(&self) -> Result<GuestTextSpanBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `BackgroundColor` wire bridge.
    fn background_color_bridge(&self) -> Result<GuestBackgroundColorBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `BorderColor` wire bridge.
    fn border_color_bridge(&self) -> Result<GuestBorderColorBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `RemSize` resource wire bridge.
    fn rem_size_bridge(&self) -> Result<GuestRemSizeBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `EmSize` component wire bridge.
    fn em_size_bridge(&self) -> Result<GuestEmSizeBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s `ThemeContext` wire bridge.
    fn theme_context_bridge(&self) -> Result<GuestThemeContextBridge, FfiResult>;
    /// Resolves `ModUiBridgePlugin`'s pre-resolved bridged component and `Activate` event handles.
    fn ui_handles(&self) -> Result<GuestUiHandles, FfiResult>;
    /// Resolves a host-opt-in `Window` wire bridge.
    fn window_bridge(&self) -> Result<GuestWindowBridge, FfiResult>;
    /// Resolves a host-opt-in `Sprite` wire bridge.
    fn sprite_bridge(&self) -> Result<GuestSpriteBridge, FfiResult>;
}

impl GuestBridgesHostExt for Host {
    #[inline]
    fn state_bridge(&self) -> Result<GuestStateBridge, FfiResult> {
        GuestStateBridge::resolve_primary(self)
    }

    #[inline]
    fn state_bridge_slot(&self, slot: usize) -> Result<GuestStateBridge, FfiResult> {
        GuestStateBridge::resolve_slot(self, slot)
    }

    #[inline]
    fn std_name_bridge(&self) -> Result<GuestNameBridge, FfiResult> {
        GuestNameBridge::resolve(self)
    }

    #[inline]
    fn std_bridges(&self) -> Result<GuestStdTypes, FfiResult> {
        GuestStdTypes::resolve_all(self)
    }

    #[inline]
    fn disabled_bridge(&self) -> Result<GuestDisabledBridge, FfiResult> {
        GuestDisabledBridge::resolve_std(self)
    }

    #[inline]
    fn ui_text_bridge(&self) -> Result<GuestUiTextBridge, FfiResult> {
        GuestUiTextBridge::resolve_ui_text(self)
    }

    #[inline]
    fn text_span_bridge(&self) -> Result<GuestTextSpanBridge, FfiResult> {
        GuestTextSpanBridge::resolve_text_span(self)
    }

    #[inline]
    fn background_color_bridge(&self) -> Result<GuestBackgroundColorBridge, FfiResult> {
        GuestBackgroundColorBridge::resolve_background_color(self)
    }

    #[inline]
    fn border_color_bridge(&self) -> Result<GuestBorderColorBridge, FfiResult> {
        GuestBorderColorBridge::resolve_border_color(self)
    }

    #[inline]
    fn rem_size_bridge(&self) -> Result<GuestRemSizeBridge, FfiResult> {
        GuestRemSizeBridge::resolve_rem_size(self)
    }

    #[inline]
    fn em_size_bridge(&self) -> Result<GuestEmSizeBridge, FfiResult> {
        GuestEmSizeBridge::resolve_em_size(self)
    }

    #[inline]
    fn theme_context_bridge(&self) -> Result<GuestThemeContextBridge, FfiResult> {
        GuestThemeContextBridge::resolve_theme_context(self)
    }

    #[inline]
    fn ui_handles(&self) -> Result<GuestUiHandles, FfiResult> {
        GuestUiHandles::resolve(self)
    }

    #[inline]
    fn window_bridge(&self) -> Result<GuestWindowBridge, FfiResult> {
        GuestWindowBridge::resolve_window(self)
    }

    #[inline]
    fn sprite_bridge(&self) -> Result<GuestSpriteBridge, FfiResult> {
        GuestSpriteBridge::resolve_sprite(self)
    }
}

impl GuestBridgesHostExt for GuestApp<'_> {
    #[inline]
    fn state_bridge(&self) -> Result<GuestStateBridge, FfiResult> {
        self.host().state_bridge()
    }

    #[inline]
    fn state_bridge_slot(&self, slot: usize) -> Result<GuestStateBridge, FfiResult> {
        self.host().state_bridge_slot(slot)
    }

    #[inline]
    fn std_name_bridge(&self) -> Result<GuestNameBridge, FfiResult> {
        self.host().std_name_bridge()
    }

    #[inline]
    fn std_bridges(&self) -> Result<GuestStdTypes, FfiResult> {
        self.host().std_bridges()
    }

    #[inline]
    fn disabled_bridge(&self) -> Result<GuestDisabledBridge, FfiResult> {
        self.host().disabled_bridge()
    }

    #[inline]
    fn ui_text_bridge(&self) -> Result<GuestUiTextBridge, FfiResult> {
        self.host().ui_text_bridge()
    }

    #[inline]
    fn text_span_bridge(&self) -> Result<GuestTextSpanBridge, FfiResult> {
        self.host().text_span_bridge()
    }

    #[inline]
    fn background_color_bridge(&self) -> Result<GuestBackgroundColorBridge, FfiResult> {
        self.host().background_color_bridge()
    }

    #[inline]
    fn border_color_bridge(&self) -> Result<GuestBorderColorBridge, FfiResult> {
        self.host().border_color_bridge()
    }

    #[inline]
    fn rem_size_bridge(&self) -> Result<GuestRemSizeBridge, FfiResult> {
        self.host().rem_size_bridge()
    }

    #[inline]
    fn em_size_bridge(&self) -> Result<GuestEmSizeBridge, FfiResult> {
        self.host().em_size_bridge()
    }

    #[inline]
    fn theme_context_bridge(&self) -> Result<GuestThemeContextBridge, FfiResult> {
        self.host().theme_context_bridge()
    }

    #[inline]
    fn ui_handles(&self) -> Result<GuestUiHandles, FfiResult> {
        self.host().ui_handles()
    }

    #[inline]
    fn window_bridge(&self) -> Result<GuestWindowBridge, FfiResult> {
        self.host().window_bridge()
    }

    #[inline]
    fn sprite_bridge(&self) -> Result<GuestSpriteBridge, FfiResult> {
        self.host().sprite_bridge()
    }
}

impl GuestBridgesHostExt for SystemContext<'_> {
    #[inline]
    fn state_bridge(&self) -> Result<GuestStateBridge, FfiResult> {
        self.host().state_bridge()
    }

    #[inline]
    fn state_bridge_slot(&self, slot: usize) -> Result<GuestStateBridge, FfiResult> {
        self.host().state_bridge_slot(slot)
    }

    #[inline]
    fn std_name_bridge(&self) -> Result<GuestNameBridge, FfiResult> {
        self.host().std_name_bridge()
    }

    #[inline]
    fn std_bridges(&self) -> Result<GuestStdTypes, FfiResult> {
        self.host().std_bridges()
    }

    #[inline]
    fn disabled_bridge(&self) -> Result<GuestDisabledBridge, FfiResult> {
        self.host().disabled_bridge()
    }

    #[inline]
    fn ui_text_bridge(&self) -> Result<GuestUiTextBridge, FfiResult> {
        self.host().ui_text_bridge()
    }

    #[inline]
    fn text_span_bridge(&self) -> Result<GuestTextSpanBridge, FfiResult> {
        self.host().text_span_bridge()
    }

    #[inline]
    fn background_color_bridge(&self) -> Result<GuestBackgroundColorBridge, FfiResult> {
        self.host().background_color_bridge()
    }

    #[inline]
    fn border_color_bridge(&self) -> Result<GuestBorderColorBridge, FfiResult> {
        self.host().border_color_bridge()
    }

    #[inline]
    fn rem_size_bridge(&self) -> Result<GuestRemSizeBridge, FfiResult> {
        self.host().rem_size_bridge()
    }

    #[inline]
    fn em_size_bridge(&self) -> Result<GuestEmSizeBridge, FfiResult> {
        self.host().em_size_bridge()
    }

    #[inline]
    fn theme_context_bridge(&self) -> Result<GuestThemeContextBridge, FfiResult> {
        self.host().theme_context_bridge()
    }

    #[inline]
    fn ui_handles(&self) -> Result<GuestUiHandles, FfiResult> {
        self.host().ui_handles()
    }

    #[inline]
    fn window_bridge(&self) -> Result<GuestWindowBridge, FfiResult> {
        self.host().window_bridge()
    }

    #[inline]
    fn sprite_bridge(&self) -> Result<GuestSpriteBridge, FfiResult> {
        self.host().sprite_bridge()
    }
}
