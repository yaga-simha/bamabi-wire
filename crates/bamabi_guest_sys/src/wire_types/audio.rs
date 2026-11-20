//! Bevy `0.20` `bevy_audio` wire mirror types (`FfiAudioPlaybackSettings`, `FfiAudioPlayerSpec`,
//! `FfiSpatialAudioSinkControl`) matching `bamabi_bridges::audio` byte-for-byte.

use bamabi_sys::{FfiAssetHandle, FfiBool};

/// `PlaybackMode::Once` discriminant (`0`, default).
pub const PLAYBACK_MODE_ONCE: u8 = 0;
/// `PlaybackMode::Loop` discriminant (`1`).
pub const PLAYBACK_MODE_LOOP: u8 = 1;
/// `PlaybackMode::Despawn` discriminant (`2`).
pub const PLAYBACK_MODE_DESPAWN: u8 = 2;
/// `PlaybackMode::Remove` discriminant (`3`).
pub const PLAYBACK_MODE_REMOVE: u8 = 3;

/// `Volume::Linear` scale discriminant (`0`, default).
pub const VOLUME_LINEAR: u8 = 0;
/// `Volume::Decibels` scale discriminant (`1`).
pub const VOLUME_DECIBELS: u8 = 1;

/// No playback state transition requested (`0`).
pub const SINK_CMD_NONE: u8 = 0;
/// Resume playback (`AudioSinkPlayback::play`, `1`).
pub const SINK_CMD_PLAY: u8 = 1;
/// Pause playback (`AudioSinkPlayback::pause`, `2`).
pub const SINK_CMD_PAUSE: u8 = 2;
/// Permanently stop playback (`AudioSinkPlayback::stop`, `3`).
pub const SINK_CMD_STOP: u8 = 3;
/// Toggle paused/playing state (`AudioSinkPlayback::toggle_playback`, `4`).
pub const SINK_CMD_TOGGLE_PLAYBACK: u8 = 4;

/// No mute transition requested (`0`).
pub const MUTE_CMD_NONE: u8 = 0;
/// Mute sink (`AudioSinkPlayback::mute`, `1`).
pub const MUTE_CMD_MUTE: u8 = 1;
/// Unmute sink (`AudioSinkPlayback::unmute`, `2`).
pub const MUTE_CMD_UNMUTE: u8 = 2;
/// Toggle muted state (`AudioSinkPlayback::toggle_mute`, `3`).
pub const MUTE_CMD_TOGGLE: u8 = 3;

/// Apply `FfiSpatialAudioSinkControl::volume` (per `volume_mode`) to the sink (`1 << 0`).
pub const APPLY_VOLUME: u8 = 1 << 0;
/// Apply `FfiSpatialAudioSinkControl::speed` to the sink (`1 << 1`).
pub const APPLY_SPEED: u8 = 1 << 1;
/// Apply `FfiSpatialAudioSinkControl::emitter_position` to `SpatialAudioSink` (`1 << 2`).
pub const APPLY_EMITTER_POS: u8 = 1 << 2;
/// Apply `left_ear_position` and `right_ear_position` to `SpatialAudioSink` (`1 << 3`).
pub const APPLY_EAR_POSITIONS: u8 = 1 << 3;

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `PlaybackSettings`
/// (`"bamabi::audio::FfiAudioPlaybackSettings"`, 36 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiAudioPlaybackSettings {
    /// Volume magnitude (`Volume::Linear` when `volume_mode == VOLUME_LINEAR`, or
    /// `Volume::Decibels` when `volume_mode == VOLUME_DECIBELS`).
    pub volume: f32,
    /// Playback speed multiplier (`1.0` = normal speed).
    pub speed: f32,
    /// Custom spatial scale `[x, y, z]` applied when `has_spatial_scale` is true.
    pub spatial_scale: [f32; 3],
    /// Clip start offset in seconds when `has_start_position` is true.
    pub start_position_secs: f32,
    /// Maximum playback duration in seconds when `has_duration` is true.
    pub duration_secs: f32,
    /// Playback completion mode (`PLAYBACK_MODE_ONCE..=PLAYBACK_MODE_REMOVE`).
    pub mode: u8,
    /// Volume scale discriminant (`VOLUME_LINEAR` or `VOLUME_DECIBELS`).
    pub volume_mode: u8,
    /// Whether playback starts paused.
    pub paused: FfiBool,
    /// Whether playback starts muted.
    pub muted: FfiBool,
    /// Whether spatial 3D panning is enabled (`SpatialAudioSink`).
    pub spatial: FfiBool,
    /// Whether `spatial_scale` overrides `DefaultSpatialScale`.
    pub has_spatial_scale: FfiBool,
    /// Whether `start_position_secs` is active (`Some(Duration)`).
    pub has_start_position: FfiBool,
    /// Whether `duration_secs` is active (`Some(Duration)`).
    pub has_duration: FfiBool,
}

impl_wire_abi_pod!(
    FfiAudioPlaybackSettings,
    size = 36,
    align = 4,
    schema = [
        volume: f32 => "f32",
        speed: f32 => "f32",
        spatial_scale: [f32; 3] => "[f32;3]",
        start_position_secs: f32 => "f32",
        duration_secs: f32 => "f32",
        mode: u8 => "u8",
        volume_mode: u8 => "u8",
        paused: FfiBool => "FfiBool",
        muted: FfiBool => "FfiBool",
        spatial: FfiBool => "FfiBool",
        has_spatial_scale: FfiBool => "FfiBool",
        has_start_position: FfiBool => "FfiBool",
        has_duration: FfiBool => "FfiBool",
    ]
);

impl Default for FfiAudioPlaybackSettings {
    #[inline]
    fn default() -> Self {
        Self::once()
    }
}

impl FfiAudioPlaybackSettings {
    /// Creates `PlaybackMode::Once` settings with linear volume `1.0` and speed `1.0`.
    #[inline]
    pub const fn once() -> Self {
        Self {
            volume: 1.0,
            speed: 1.0,
            spatial_scale: [1.0, 1.0, 1.0],
            start_position_secs: 0.0,
            duration_secs: 0.0,
            mode: PLAYBACK_MODE_ONCE,
            volume_mode: VOLUME_LINEAR,
            paused: FfiBool::FALSE,
            muted: FfiBool::FALSE,
            spatial: FfiBool::FALSE,
            has_spatial_scale: FfiBool::FALSE,
            has_start_position: FfiBool::FALSE,
            has_duration: FfiBool::FALSE,
        }
    }

    /// Creates `PlaybackMode::Loop` settings.
    #[inline]
    pub const fn looped() -> Self {
        Self {
            mode: PLAYBACK_MODE_LOOP,
            ..Self::once()
        }
    }

    /// Creates `PlaybackMode::Despawn` settings.
    #[inline]
    pub const fn despawn() -> Self {
        Self {
            mode: PLAYBACK_MODE_DESPAWN,
            ..Self::once()
        }
    }

    /// Creates `PlaybackMode::Remove` settings.
    #[inline]
    pub const fn remove() -> Self {
        Self {
            mode: PLAYBACK_MODE_REMOVE,
            ..Self::once()
        }
    }

    /// Returns a copy configured with linear volume `gain` (`VOLUME_LINEAR`).
    #[inline]
    #[must_use]
    pub const fn with_volume_linear(mut self, gain: f32) -> Self {
        self.volume = gain;
        self.volume_mode = VOLUME_LINEAR;
        self
    }

    /// Returns a copy configured with decibel volume `db` (`VOLUME_DECIBELS`).
    #[inline]
    #[must_use]
    pub const fn with_volume_decibels(mut self, db: f32) -> Self {
        self.volume = db;
        self.volume_mode = VOLUME_DECIBELS;
        self
    }

    /// Returns a copy with spatial panning enabled/disabled.
    #[inline]
    #[must_use]
    pub const fn with_spatial(mut self, spatial: bool) -> Self {
        self.spatial = FfiBool::from_bool(spatial);
        self
    }

    /// Returns a copy with custom spatial scale `[x, y, z]` enabled.
    #[inline]
    #[must_use]
    pub const fn with_spatial_scale(mut self, scale: [f32; 3]) -> Self {
        self.spatial = FfiBool::TRUE;
        self.spatial_scale = scale;
        self.has_spatial_scale = FfiBool::TRUE;
        self
    }

    /// Returns a copy with playback speed multiplier `speed`.
    #[inline]
    #[must_use]
    pub const fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Converts the configured volume into linear amplitude (`>= 0.0`).
    #[inline]
    pub fn to_linear_volume(&self) -> f32 {
        if self.volume_mode == VOLUME_DECIBELS {
            if self.volume.is_finite() {
                10.0_f32.powf(self.volume / 20.0)
            } else {
                0.0
            }
        } else if self.volume.is_finite() {
            self.volume.max(0.0)
        } else {
            0.0
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of `AudioPlayer<AudioSource>` + `PlaybackSettings`
/// (`"bamabi::audio::FfiAudioPlayerSpec"`, 68 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiAudioPlayerSpec {
    /// Handle to the `AudioSource` asset (`KIND_INDEX`, `KIND_UUID`, or `KIND_NONE`).
    pub source: FfiAssetHandle,
    /// Playback configuration mirrored into `PlaybackSettings`.
    pub settings: FfiAudioPlaybackSettings,
}

impl_wire_abi_pod!(
    FfiAudioPlayerSpec,
    size = 68,
    align = 4,
    schema = [
        source: FfiAssetHandle => "FfiAssetHandle",
        settings: FfiAudioPlaybackSettings => "FfiAudioPlaybackSettings",
    ]
);

impl FfiAudioPlayerSpec {
    /// Creates an audio player specification for `source` with `settings`.
    #[inline]
    pub const fn new(source: FfiAssetHandle, settings: FfiAudioPlaybackSettings) -> Self {
        Self { source, settings }
    }
}

/// Zero-padding `#[repr(C)]` runtime control & readback component synchronized with Bevy's
/// `SpatialAudioSink` and `AudioSink` (`"bamabi::audio::FfiSpatialAudioSinkControl"`, 56 bytes,
/// align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiSpatialAudioSinkControl {
    /// Target (when `APPLY_VOLUME` is set) or observed sink volume (linear or dB per `volume_mode`).
    pub volume: f32,
    /// Target (when `APPLY_SPEED` is set) or observed sink playback speed (`1.0` = normal).
    pub speed: f32,
    /// Observed sink playback position in seconds (`AudioSinkPlayback::position`).
    pub position_secs: f32,
    /// Spatial emitter position `[x, y, z]` applied when `APPLY_EMITTER_POS` is set.
    pub emitter_position: [f32; 3],
    /// Spatial listener left-ear position `[x, y, z]` applied when `APPLY_EAR_POSITIONS` is set.
    pub left_ear_position: [f32; 3],
    /// Spatial listener right-ear position `[x, y, z]` applied when `APPLY_EAR_POSITIONS` is set.
    pub right_ear_position: [f32; 3],
    /// One-shot playback command (`SINK_CMD_NONE..=SINK_CMD_TOGGLE_PLAYBACK`).
    pub playback_command: u8,
    /// One-shot mute command (`MUTE_CMD_NONE..=MUTE_CMD_TOGGLE`).
    pub mute_command: u8,
    /// Volume scale discriminant (`VOLUME_LINEAR` or `VOLUME_DECIBELS`).
    pub volume_mode: u8,
    /// Bitmask of fields to push (`APPLY_VOLUME | APPLY_SPEED | APPLY_EMITTER_POS | APPLY_EAR_POSITIONS`).
    pub apply_flags: u8,
    /// Observed paused state (`AudioSinkPlayback::is_paused`).
    pub is_paused: FfiBool,
    /// Observed muted state (`AudioSinkPlayback::is_muted`).
    pub is_muted: FfiBool,
    /// Observed empty/drained state (`AudioSinkPlayback::empty`).
    pub is_empty: FfiBool,
    /// `true` when synchronized against `SpatialAudioSink`, `false` for `AudioSink` or detached.
    pub is_spatial: FfiBool,
}

impl_wire_abi_pod!(
    FfiSpatialAudioSinkControl,
    size = 56,
    align = 4,
    schema = [
        volume: f32 => "f32",
        speed: f32 => "f32",
        position_secs: f32 => "f32",
        emitter_position: [f32; 3] => "[f32;3]",
        left_ear_position: [f32; 3] => "[f32;3]",
        right_ear_position: [f32; 3] => "[f32;3]",
        playback_command: u8 => "u8",
        mute_command: u8 => "u8",
        volume_mode: u8 => "u8",
        apply_flags: u8 => "u8",
        is_paused: FfiBool => "FfiBool",
        is_muted: FfiBool => "FfiBool",
        is_empty: FfiBool => "FfiBool",
        is_spatial: FfiBool => "FfiBool",
    ]
);

impl Default for FfiSpatialAudioSinkControl {
    #[inline]
    fn default() -> Self {
        Self {
            volume: 1.0,
            speed: 1.0,
            position_secs: 0.0,
            emitter_position: [0.0; 3],
            left_ear_position: [-2.0, 0.0, 0.0],
            right_ear_position: [2.0, 0.0, 0.0],
            playback_command: SINK_CMD_NONE,
            mute_command: MUTE_CMD_NONE,
            volume_mode: VOLUME_LINEAR,
            apply_flags: 0,
            is_paused: FfiBool::FALSE,
            is_muted: FfiBool::FALSE,
            is_empty: FfiBool::FALSE,
            is_spatial: FfiBool::FALSE,
        }
    }
}

impl FfiSpatialAudioSinkControl {
    /// Returns `true` if any one-shot command or apply flag is pending push to the Bevy sink.
    #[inline]
    pub const fn has_pending_command(&self) -> bool {
        self.playback_command != SINK_CMD_NONE
            || self.mute_command != MUTE_CMD_NONE
            || self.apply_flags != 0
    }

    /// Requests resuming playback (`SINK_CMD_PLAY`).
    #[inline]
    pub fn request_play(&mut self) {
        self.playback_command = SINK_CMD_PLAY;
    }

    /// Requests pausing playback (`SINK_CMD_PAUSE`).
    #[inline]
    pub fn request_pause(&mut self) {
        self.playback_command = SINK_CMD_PAUSE;
    }

    /// Requests stopping playback (`SINK_CMD_STOP`).
    #[inline]
    pub fn request_stop(&mut self) {
        self.playback_command = SINK_CMD_STOP;
    }

    /// Requests toggling play/pause state (`SINK_CMD_TOGGLE_PLAYBACK`).
    #[inline]
    pub fn request_toggle_playback(&mut self) {
        self.playback_command = SINK_CMD_TOGGLE_PLAYBACK;
    }

    /// Requests muting playback (`MUTE_CMD_MUTE`).
    #[inline]
    pub fn request_mute(&mut self) {
        self.mute_command = MUTE_CMD_MUTE;
    }

    /// Requests unmuting playback (`MUTE_CMD_UNMUTE`).
    #[inline]
    pub fn request_unmute(&mut self) {
        self.mute_command = MUTE_CMD_UNMUTE;
    }

    /// Requests toggling mute state (`MUTE_CMD_TOGGLE`).
    #[inline]
    pub fn request_toggle_mute(&mut self) {
        self.mute_command = MUTE_CMD_TOGGLE;
    }

    /// Requests setting linear volume on the next host sync (`APPLY_VOLUME`, `VOLUME_LINEAR`).
    #[inline]
    pub fn set_linear_volume(&mut self, linear: f32) {
        self.volume_mode = VOLUME_LINEAR;
        self.volume = linear;
        self.apply_flags |= APPLY_VOLUME;
    }

    /// Alias for [`Self::set_linear_volume`].
    #[inline]
    pub fn set_volume_linear(&mut self, gain: f32) {
        self.set_linear_volume(gain);
    }

    /// Requests setting decibel volume on the next host sync (`APPLY_VOLUME`, `VOLUME_DECIBELS`).
    #[inline]
    pub fn set_decibel_volume(&mut self, decibels: f32) {
        self.volume_mode = VOLUME_DECIBELS;
        self.volume = decibels;
        self.apply_flags |= APPLY_VOLUME;
    }

    /// Alias for [`Self::set_decibel_volume`].
    #[inline]
    pub fn set_volume_decibels(&mut self, db: f32) {
        self.set_decibel_volume(db);
    }

    /// Requests setting playback speed on the next host sync (`APPLY_SPEED`).
    #[inline]
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.apply_flags |= APPLY_SPEED;
    }

    /// Requests setting spatial emitter position `[x, y, z]` (`APPLY_EMITTER_POS`).
    #[inline]
    pub fn set_emitter_position(&mut self, pos: [f32; 3]) {
        self.emitter_position = pos;
        self.apply_flags |= APPLY_EMITTER_POS;
    }

    /// Requests setting spatial left/right ear positions (`APPLY_EAR_POSITIONS`).
    #[inline]
    pub fn set_ear_positions(&mut self, left: [f32; 3], right: [f32; 3]) {
        self.left_ear_position = left;
        self.right_ear_position = right;
        self.apply_flags |= APPLY_EAR_POSITIONS;
    }

    /// Alias for [`Self::set_ear_positions`].
    #[inline]
    pub fn set_ears_position(&mut self, left: [f32; 3], right: [f32; 3]) {
        self.set_ear_positions(left, right);
    }
}
