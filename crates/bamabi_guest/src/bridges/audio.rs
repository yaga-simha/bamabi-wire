//! Guest-side `#![forbid(unsafe_code)]`, zero-Bevy bridge for Bevy `0.20` `bevy_audio`
//! (`FfiAudioPlaybackSettings`, `FfiAudioPlayerSpec`, `FfiSpatialAudioSinkControl`, and
//! `AudioSource` asset handles) matching `bamabi_bridges::audio` (`ModAudioBridgePlugin`)
//! bit-for-bit.

use bamabi_guest_sys::{Host, SpawnEntry, resolve_component};
use bamabi_sys::{AccessMode, BamComponent, FfiAssetHandle, FfiEntity, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    APPLY_EAR_POSITIONS, APPLY_EMITTER_POS, APPLY_SPEED, APPLY_VOLUME, FfiAudioPlaybackSettings,
    FfiAudioPlayerSpec, FfiSpatialAudioSinkControl, MUTE_CMD_MUTE, MUTE_CMD_NONE, MUTE_CMD_TOGGLE,
    MUTE_CMD_UNMUTE, PLAYBACK_MODE_DESPAWN, PLAYBACK_MODE_LOOP, PLAYBACK_MODE_ONCE,
    PLAYBACK_MODE_REMOVE, SINK_CMD_NONE, SINK_CMD_PAUSE, SINK_CMD_PLAY, SINK_CMD_STOP,
    SINK_CMD_TOGGLE_PLAYBACK, VOLUME_DECIBELS, VOLUME_LINEAR,
};

use crate::{app::QueryBuilder, commands::GuestCommands, component::GuestComponent};

/// Canonical ABI name for [`FfiAudioPlaybackSettings`].
pub const CANONICAL_AUDIO_PLAYBACK_SETTINGS_NAME: &str = "bamabi::audio::FfiAudioPlaybackSettings";
/// Canonical ABI name for [`FfiAudioPlayerSpec`].
pub const CANONICAL_AUDIO_PLAYER_SPEC_NAME: &str = "bamabi::audio::FfiAudioPlayerSpec";
/// Canonical ABI name for [`FfiSpatialAudioSinkControl`].
pub const CANONICAL_SPATIAL_AUDIO_SINK_CONTROL_NAME: &str =
    "bamabi::audio::FfiSpatialAudioSinkControl";
/// Canonical Rust type name for Bevy's `AudioSource` asset registered by `ModAudioBridgePlugin`.
pub const CANONICAL_AUDIO_SOURCE_NAME: &str = "bevy_audio::audio_source::AudioSource";
/// Short alias registered for `AudioSource` in `ModRegistry`.
pub const AUDIO_SOURCE_ASSET_ALIAS: &str = "AudioSource";

impl GuestComponent for FfiAudioPlaybackSettings {
    const NAME: &'static CStr = c"bamabi::audio::FfiAudioPlaybackSettings";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiAudioPlayerSpec {
    const NAME: &'static CStr = c"bamabi::audio::FfiAudioPlayerSpec";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiSpatialAudioSinkControl {
    const NAME: &'static CStr = c"bamabi::audio::FfiSpatialAudioSinkControl";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Resolves the host-registered `AudioSource` asset type handle (`"AudioSource"` alias or
/// `"bevy_audio::audio_source::AudioSource"`).
#[inline]
pub fn resolve_audio_source_type(host: &Host) -> Result<BamComponent, FfiResult> {
    resolve_component(host, Some(c"bevy_audio::audio_source::AudioSource"), None)
        .or_else(|_| resolve_component(host, Some(c"AudioSource"), None))
}

/// Extension trait on [`QueryBuilder`] adding direct POD Bevy `0.20` audio query terms.
pub trait GuestAudioQueryBuilderExt {
    /// Appends a shared read (`&FfiAudioPlaybackSettings`) term.
    fn read_audio_playback_settings(self) -> Self;
    /// Appends an exclusive write (`&mut FfiAudioPlaybackSettings`) term.
    fn write_audio_playback_settings(self) -> Self;
    /// Appends a shared read (`&FfiAudioPlayerSpec`) term.
    fn read_audio_player_spec(self) -> Self;
    /// Appends an exclusive write (`&mut FfiAudioPlayerSpec`) term.
    fn write_audio_player_spec(self) -> Self;
    /// Appends a shared read (`&FfiSpatialAudioSinkControl`) term.
    fn read_spatial_audio_sink_control(self) -> Self;
    /// Appends an exclusive write (`&mut FfiSpatialAudioSinkControl`) term.
    fn write_spatial_audio_sink_control(self) -> Self;
}

impl GuestAudioQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_audio_playback_settings(self) -> Self {
        self.term_named(
            FfiAudioPlaybackSettings::NAME,
            Some(FfiAudioPlaybackSettings::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_audio_playback_settings(self) -> Self {
        self.term_named(
            FfiAudioPlaybackSettings::NAME,
            Some(FfiAudioPlaybackSettings::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_audio_player_spec(self) -> Self {
        self.term_named(
            FfiAudioPlayerSpec::NAME,
            Some(FfiAudioPlayerSpec::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_audio_player_spec(self) -> Self {
        self.term_named(
            FfiAudioPlayerSpec::NAME,
            Some(FfiAudioPlayerSpec::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_spatial_audio_sink_control(self) -> Self {
        self.term_named(
            FfiSpatialAudioSinkControl::NAME,
            Some(FfiSpatialAudioSinkControl::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_spatial_audio_sink_control(self) -> Self {
        self.term_named(
            FfiSpatialAudioSinkControl::NAME,
            Some(FfiSpatialAudioSinkControl::UUID),
            AccessMode::WRITE,
        )
    }
}

/// Extension trait on [`GuestCommands`] for spawning and controlling Bevy `0.20` audio playback.
pub trait GuestCommandsAudioExt {
    /// Resolves [`FfiAudioPlayerSpec`] on the host and atomically spawns a new mod-scoped audio
    /// entity carrying it (without leaving an orphan entity queued if resolution or spawning fails).
    fn spawn_audio(
        &mut self,
        source: FfiAssetHandle,
        settings: FfiAudioPlaybackSettings,
    ) -> Result<FfiEntity, FfiResult>;
    /// Inserts [`FfiAudioPlayerSpec`] onto `entity`.
    fn insert_audio_player(&mut self, entity: FfiEntity, spec: &FfiAudioPlayerSpec) -> FfiResult;
    /// Inserts [`FfiAudioPlaybackSettings`] onto `entity`.
    fn insert_audio_playback_settings(
        &mut self,
        entity: FfiEntity,
        settings: &FfiAudioPlaybackSettings,
    ) -> FfiResult;
    /// Inserts [`FfiSpatialAudioSinkControl`] onto `entity`.
    fn insert_spatial_audio_sink_control(
        &mut self,
        entity: FfiEntity,
        control: &FfiSpatialAudioSinkControl,
    ) -> FfiResult;
}

impl GuestCommandsAudioExt for GuestCommands<'_> {
    fn spawn_audio(
        &mut self,
        source: FfiAssetHandle,
        settings: FfiAudioPlaybackSettings,
    ) -> Result<FfiEntity, FfiResult> {
        if !self.is_valid() {
            return Err(FfiResult::NULL_POINTER);
        }
        let comp = self.resolve_component(
            Some(FfiAudioPlayerSpec::NAME),
            Some(FfiAudioPlayerSpec::UUID),
        )?;
        let spec = FfiAudioPlayerSpec::new(source, settings);
        match self.spawn(&[SpawnEntry::new(comp, &spec)]) {
            Ok(entity) => Ok(entity),
            Err(FfiResult::UNSUPPORTED) => {
                // ABI 0.2.0 host without `component_layout_info`: fall back to `spawn_empty` +
                // `insert`, despawning the entity on error so no orphan entity leaks.
                let entity = self.spawn_empty()?;
                let status = self.insert(entity, comp, &spec);
                if status.is_ok() {
                    Ok(entity)
                } else {
                    let _ = self.despawn(entity);
                    Err(status)
                }
            }
            Err(err) => Err(err),
        }
    }

    #[inline]
    fn insert_audio_player(&mut self, entity: FfiEntity, spec: &FfiAudioPlayerSpec) -> FfiResult {
        self.insert_component(entity, spec)
    }

    #[inline]
    fn insert_audio_playback_settings(
        &mut self,
        entity: FfiEntity,
        settings: &FfiAudioPlaybackSettings,
    ) -> FfiResult {
        self.insert_component(entity, settings)
    }

    #[inline]
    fn insert_spatial_audio_sink_control(
        &mut self,
        entity: FfiEntity,
        control: &FfiSpatialAudioSinkControl,
    ) -> FfiResult {
        self.insert_component(entity, control)
    }
}
