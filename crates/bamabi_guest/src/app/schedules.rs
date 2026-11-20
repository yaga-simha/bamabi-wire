//! Bevy `0.20` standard schedule layout helper (`BevySchedules`) mapping all 16 built-in Bevy
//! schedules onto ABI `0.3.0`'s frozen built-in [`ScheduleId`]s (`0..=5`) and deterministic
//! custom schedule offsets (`>= ScheduleId::CUSTOM_BASE`).

use bamabi_sys::ScheduleId;

/// Deterministic mapping of all Bevy `0.20` standard app schedules onto ABI `0.3.0` [`ScheduleId`]s.
///
/// The six primary schedules (`Update`, `Startup`, `PreUpdate`, `PostUpdate`, `FixedUpdate`,
/// `Last`) use ABI `0.3.0`'s built-in IDs (`0..=5`). The remaining ten Bevy `0.20` schedules
/// (`PreStartup`, `PostStartup`, `First`, `StateTransition`, `RunFixedMainLoop`, `FixedFirst`,
/// `FixedPreUpdate`, `FixedPostUpdate`, `FixedLast`, `SpawnScene`) map to deterministic offsets
/// starting at `custom_base` (which must be `>= ScheduleId::CUSTOM_BASE = 1024`), preserving full
/// wire compatibility with frozen ABI `0.3.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BevySchedules {
    custom_base: u32,
}

impl BevySchedules {
    /// Default custom base for extended Bevy `0.20` schedules (`ScheduleId::CUSTOM_BASE` = `1024`).
    pub const DEFAULT: Self = Self {
        custom_base: ScheduleId::CUSTOM_BASE,
    };

    /// Offset from `custom_base` for `PreStartup` (`0`).
    pub const PRE_STARTUP_OFFSET: u32 = 0;
    /// Offset from `custom_base` for `PostStartup` (`1`).
    pub const POST_STARTUP_OFFSET: u32 = 1;
    /// Offset from `custom_base` for `First` (`2`).
    pub const FIRST_OFFSET: u32 = 2;
    /// Offset from `custom_base` for `StateTransition` (`3`).
    pub const STATE_TRANSITION_OFFSET: u32 = 3;
    /// Offset from `custom_base` for `RunFixedMainLoop` (`4`).
    pub const RUN_FIXED_MAIN_LOOP_OFFSET: u32 = 4;
    /// Offset from `custom_base` for `FixedFirst` (`5`).
    pub const FIXED_FIRST_OFFSET: u32 = 5;
    /// Offset from `custom_base` for `FixedPreUpdate` (`6`).
    pub const FIXED_PRE_UPDATE_OFFSET: u32 = 6;
    /// Offset from `custom_base` for `FixedPostUpdate` (`7`).
    pub const FIXED_POST_UPDATE_OFFSET: u32 = 7;
    /// Offset from `custom_base` for `FixedLast` (`8`).
    pub const FIXED_LAST_OFFSET: u32 = 8;
    /// Offset from `custom_base` for `SpawnScene` (`9`).
    pub const SPAWN_SCENE_OFFSET: u32 = 9;
    /// Total number of custom schedule slots reserved by [`BevySchedules`] (`10`).
    pub const EXTENDED_SLOT_COUNT: u32 = 10;

    /// Creates a [`BevySchedules`] mapper anchored at `custom_base` (`>= ScheduleId::CUSTOM_BASE`),
    /// returning `None` if `custom_base` falls inside the reserved `0..1024` range or overflows.
    #[inline]
    pub const fn with_custom_base(custom_base: ScheduleId) -> Option<Self> {
        if !custom_base.is_custom() {
            return None;
        }
        if custom_base
            .0
            .checked_add(Self::EXTENDED_SLOT_COUNT - 1)
            .is_none()
        {
            return None;
        }
        Some(Self {
            custom_base: custom_base.0,
        })
    }

    /// Returns the base custom schedule ID used for extended Bevy `0.20` schedules.
    #[inline]
    pub const fn custom_base(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base)
    }

    /// Built-in ABI `0.3.0` `Startup` schedule (`ScheduleId::STARTUP`).
    #[inline]
    pub const fn startup(&self) -> ScheduleId {
        ScheduleId::STARTUP
    }

    /// Extended `PreStartup` schedule (`custom_base + 0`).
    #[inline]
    pub const fn pre_startup(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::PRE_STARTUP_OFFSET)
    }

    /// Extended `PostStartup` schedule (`custom_base + 1`).
    #[inline]
    pub const fn post_startup(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::POST_STARTUP_OFFSET)
    }

    /// Extended `First` schedule (`custom_base + 2`).
    #[inline]
    pub const fn first(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::FIRST_OFFSET)
    }

    /// Built-in ABI `0.3.0` `PreUpdate` schedule (`ScheduleId::PRE_UPDATE`).
    #[inline]
    pub const fn pre_update(&self) -> ScheduleId {
        ScheduleId::PRE_UPDATE
    }

    /// Extended `StateTransition` schedule (`custom_base + 3`).
    #[inline]
    pub const fn state_transition(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::STATE_TRANSITION_OFFSET)
    }

    /// Extended `RunFixedMainLoop` schedule (`custom_base + 4`).
    #[inline]
    pub const fn run_fixed_main_loop(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::RUN_FIXED_MAIN_LOOP_OFFSET)
    }

    /// Extended `FixedFirst` schedule (`custom_base + 5`).
    #[inline]
    pub const fn fixed_first(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::FIXED_FIRST_OFFSET)
    }

    /// Extended `FixedPreUpdate` schedule (`custom_base + 6`).
    #[inline]
    pub const fn fixed_pre_update(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::FIXED_PRE_UPDATE_OFFSET)
    }

    /// Built-in ABI `0.3.0` `FixedUpdate` schedule (`ScheduleId::FIXED_UPDATE`).
    #[inline]
    pub const fn fixed_update(&self) -> ScheduleId {
        ScheduleId::FIXED_UPDATE
    }

    /// Extended `FixedPostUpdate` schedule (`custom_base + 7`).
    #[inline]
    pub const fn fixed_post_update(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::FIXED_POST_UPDATE_OFFSET)
    }

    /// Extended `FixedLast` schedule (`custom_base + 8`).
    #[inline]
    pub const fn fixed_last(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::FIXED_LAST_OFFSET)
    }

    /// Built-in ABI `0.3.0` `Update` schedule (`ScheduleId::UPDATE`).
    #[inline]
    pub const fn update(&self) -> ScheduleId {
        ScheduleId::UPDATE
    }

    /// Extended `SpawnScene` schedule (`custom_base + 9`).
    #[inline]
    pub const fn spawn_scene(&self) -> ScheduleId {
        ScheduleId::custom(self.custom_base + Self::SPAWN_SCENE_OFFSET)
    }

    /// Built-in ABI `0.3.0` `PostUpdate` schedule (`ScheduleId::POST_UPDATE`).
    #[inline]
    pub const fn post_update(&self) -> ScheduleId {
        ScheduleId::POST_UPDATE
    }

    /// Built-in ABI `0.3.0` `Last` schedule (`ScheduleId::LAST`).
    #[inline]
    pub const fn last(&self) -> ScheduleId {
        ScheduleId::LAST
    }
}

impl Default for BevySchedules {
    #[inline]
    fn default() -> Self {
        Self::DEFAULT
    }
}
