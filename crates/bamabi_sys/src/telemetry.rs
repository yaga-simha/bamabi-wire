//! Canonical `"BAMT"` 64-byte telemetry digest frame and `bam/*` BRP method wire constants.
//!
//! Shared across Layer 0 (`bamabi_sys` / `bamabi_guest`), Layer 2/3 host telemetry
//! (`bamabi_reflect`), and developer tooling (`strata_lsp`) in `#![no_std]` environments with zero
//! external dependencies.

use core::error::Error;
use core::fmt;

use crate::primitives::{ComponentUuid, fnv1a_64_init, fnv1a_64_update_bytes};
use crate::traits::{AbiPod, RawInsertable, assert_abi_pod};

/// BRP method path listing active mods, versions, system counts, fault counts, and quarantine state.
pub const BRP_METHOD_BAM_LIST_MODS: &str = "bam/list_mods";
/// BRP method path reporting per-system execution, `summary_tick`, and `p50_us` / `p99_us` metrics.
pub const BRP_METHOD_BAM_SYSTEM_METRICS: &str = "bam/system_metrics";
/// BRP method path inspecting registered component schemas, fingerprints, and `BamFieldDesc` offsets.
pub const BRP_METHOD_BAM_COMPONENT_SCHEMAS: &str = "bam/component_schemas";
/// BRP method path triggering or scheduling a mod hot-reload by `mod_id` or `mod_name`.
pub const BRP_METHOD_BAM_HOT_RELOAD: &str = "bam/hot_reload";

/// 4-byte wire magic prefix for [`SystemTelemetryDigest64`] frames (`"BAMT"`).
pub const SYSTEM_TELEMETRY_MAGIC: [u8; 4] = *b"BAMT";

/// Wire schema version for [`SystemTelemetryDigest64`].
pub const SYSTEM_TELEMETRY_DIGEST_VERSION: u16 = 1;

/// Exact byte length of one serialized [`SystemTelemetryDigest64`] frame (`64` bytes).
pub const SYSTEM_TELEMETRY_DIGEST_SIZE: usize = 64;

/// Flag bit indicating the guest system is currently quarantined (`1 << 0`).
pub const TELEMETRY_FLAG_QUARANTINED: u16 = 1 << 0;
/// Flag bit indicating the guest system faulted on its latest sample (`1 << 1`).
pub const TELEMETRY_FLAG_FAULTED: u16 = 1 << 1;
/// Flag bit indicating Bevy `0.20` `summary_tick` table skipping occurred (`skipped > 0`, `1 << 2`).
pub const TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE: u16 = 1 << 2;
/// Flag bit indicating a per-mod budget watchdog breach was reported for this system (`1 << 3`).
pub const TELEMETRY_FLAG_BUDGET_BREACHED: u16 = 1 << 3;

/// Computes the deterministic 64-bit FNV-1a system identifier hash streamed in
/// [`SystemTelemetryDigest64::system_hash`] (`"{mod_name}::{system_name}"`).
#[must_use]
#[inline]
pub const fn fnv1a_64_hash_system(mod_name: &str, system_name: &str) -> u64 {
    let h = fnv1a_64_init();
    let h = fnv1a_64_update_bytes(h, mod_name.as_bytes());
    let h = fnv1a_64_update_bytes(h, b"::");
    fnv1a_64_update_bytes(h, system_name.as_bytes())
}

/// Errors returned when decoding a 64-byte [`SystemTelemetryDigest64`] wire frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelemetryFrameError {
    /// Leading 4-byte magic did not match [`SYSTEM_TELEMETRY_MAGIC`] (`"BAMT"`).
    InvalidMagic([u8; 4]),
    /// Frame wire version is unsupported.
    UnsupportedVersion(u16),
}

impl fmt::Display for TelemetryFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic(m) => {
                write!(
                    f,
                    "invalid telemetry frame magic {m:?} (expected b\"BAMT\")"
                )
            }
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported telemetry frame version {v} (expected {SYSTEM_TELEMETRY_DIGEST_VERSION})"
            ),
        }
    }
}

impl Error for TelemetryFrameError {}

/// Errors returned when decoding a [`SystemTelemetryDigest64`] from a variable-length byte slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelemetrySliceError {
    /// Byte slice length did not equal [`SYSTEM_TELEMETRY_DIGEST_SIZE`] (`64`).
    InvalidLength(usize),
    /// Frame header validation failed (`InvalidMagic` or `UnsupportedVersion`).
    Frame(TelemetryFrameError),
}

impl fmt::Display for TelemetrySliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(len) => write!(
                f,
                "invalid telemetry frame length {len} (expected {SYSTEM_TELEMETRY_DIGEST_SIZE})"
            ),
            Self::Frame(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl Error for TelemetrySliceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidLength(_) => None,
            Self::Frame(err) => Some(err),
        }
    }
}

impl From<TelemetryFrameError> for TelemetrySliceError {
    #[inline]
    fn from(err: TelemetryFrameError) -> Self {
        Self::Frame(err)
    }
}

/// Fixed **64-byte** little-endian binary telemetry digest frame emitted by host telemetry and
/// consumed by guest mods and LSP diagnostics.
///
/// Layout (`64` bytes, `align_of::<u64>()` — `8` on 64-bit / `wasm32`, `4` on `i686`, zero implicit padding):
/// - `0..4`: `magic` (`b"BAMT"`)
/// - `4..6`: `version` (`u16` LE, currently `1`)
/// - `6..8`: `flags` (`u16` LE bitmask of `TELEMETRY_FLAG_*`)
/// - `8..16`: `mod_id` (`u64` LE)
/// - `16..24`: `system_hash` (`u64` LE FNV-1a of `"{mod_name}::{system_name}"`)
/// - `24..32`: `frame_seq` (`u64` LE monotonic frame counter)
/// - `32..36`: `matched_entities` (`u32` LE)
/// - `36..40`: `matched_tables` (`u32` LE)
/// - `40..44`: `skipped_summary_tick_tables` (`u32` LE)
/// - `44..48`: `chunks` (`u32` LE)
/// - `48..52`: `p50_us` (`u32` LE rolling median duration in microseconds)
/// - `52..56`: `p99_us` (`u32` LE rolling 99th-percentile duration in microseconds)
/// - `56..60`: `commands_queued` (`u32` LE)
/// - `60..64`: `reserved` (`u32` LE, always `0`)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemTelemetryDigest64 {
    /// Wire magic prefix (`b"BAMT"`).
    pub magic: [u8; 4],
    /// Wire schema version (`SYSTEM_TELEMETRY_DIGEST_VERSION` = `1`).
    pub version: u16,
    /// Status bitflags (`TELEMETRY_FLAG_*`).
    pub flags: u16,
    /// Owning `ModId.0` (`0` for unscoped / host-owned systems).
    pub mod_id: u64,
    /// Deterministic 64-bit FNV-1a hash of `"{mod_name}::{system_name}"`.
    pub system_hash: u64,
    /// Monotonic frame sequence number.
    pub frame_seq: u64,
    /// Number of entities matched/visited by the system on the latest sample.
    pub matched_entities: u32,
    /// Number of ECS tables matched by the system's query.
    pub matched_tables: u32,
    /// Number of matched ECS tables skipped via Bevy `0.20` `summary_tick` acceleration.
    pub skipped_summary_tick_tables: u32,
    /// Number of contiguous chunk iterations executed by the system.
    pub chunks: u32,
    /// Rolling median (`p50`) wall-clock execution duration in microseconds.
    pub p50_us: u32,
    /// Rolling 99th-percentile (`p99`) wall-clock execution duration in microseconds.
    pub p99_us: u32,
    /// Number of deferred host commands queued on the latest sample.
    pub commands_queued: u32,
    /// Reserved zero word for 64-byte wire alignment.
    pub reserved: u32,
}

const _: () = {
    assert!(!core::mem::needs_drop::<SystemTelemetryDigest64>());
    assert_abi_pod::<[u8; 4]>();
    assert_abi_pod::<u16>();
    assert_abi_pod::<u64>();
    assert_abi_pod::<u32>();
    let field_sum = core::mem::size_of::<[u8; 4]>()
        + core::mem::size_of::<u16>() * 2
        + core::mem::size_of::<u64>() * 3
        + core::mem::size_of::<u32>() * 8;
    assert!(core::mem::size_of::<SystemTelemetryDigest64>() == field_sum);
    assert!(core::mem::size_of::<SystemTelemetryDigest64>() == SYSTEM_TELEMETRY_DIGEST_SIZE);
    assert!(core::mem::align_of::<SystemTelemetryDigest64>() == core::mem::align_of::<u64>());
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, magic) == 0);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, version) == 4);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, flags) == 6);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, mod_id) == 8);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, system_hash) == 16);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, frame_seq) == 24);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, matched_entities) == 32);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, matched_tables) == 36);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, skipped_summary_tick_tables) == 40);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, chunks) == 44);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, p50_us) == 48);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, p99_us) == 52);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, commands_queued) == 56);
    assert!(core::mem::offset_of!(SystemTelemetryDigest64, reserved) == 60);
};

// SAFETY: `SystemTelemetryDigest64` is a zero-padding `#[repr(C)]` struct of `Copy` integer and
// byte-array fields with no heap pointers, no drop glue, and universal bit validity.
unsafe impl RawInsertable for SystemTelemetryDigest64 {}

// SAFETY: All 14 fields satisfy `AbiPod`, layout is deterministic C-ABI with zero implicit
// padding (`size = 64, align = align_of::<u64>()`), verified by compile-time assertions above.
unsafe impl AbiPod for SystemTelemetryDigest64 {}

impl Default for SystemTelemetryDigest64 {
    #[inline]
    fn default() -> Self {
        Self {
            magic: SYSTEM_TELEMETRY_MAGIC,
            version: SYSTEM_TELEMETRY_DIGEST_VERSION,
            flags: 0,
            mod_id: 0,
            system_hash: 0,
            frame_seq: 0,
            matched_entities: 0,
            matched_tables: 0,
            skipped_summary_tick_tables: 0,
            chunks: 0,
            p50_us: 0,
            p99_us: 0,
            commands_queued: 0,
            reserved: 0,
        }
    }
}

/// Writes `src` into `dst` starting at `offset` in `const` contexts.
#[inline]
const fn write_bytes_at<const N: usize>(
    buf: &mut [u8; SYSTEM_TELEMETRY_DIGEST_SIZE],
    offset: usize,
    src: [u8; N],
) {
    let mut i = 0;
    while i < N {
        buf[offset + i] = src[i];
        i += 1;
    }
}

impl SystemTelemetryDigest64 {
    /// Deterministic SipHash-2-4 field-schema hash matching `#[derive(ModComponent)]` and
    /// `#[derive(GuestComponent)]`.
    pub const FIELD_SCHEMA_HASH: u64 = {
        let mut acc: u64 = 0;
        acc = ComponentUuid::hash_field_schema(
            acc,
            "magic",
            "[u8;4]",
            core::mem::offset_of!(Self, magic),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "version",
            "u16",
            core::mem::offset_of!(Self, version),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "flags",
            "u16",
            core::mem::offset_of!(Self, flags),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "mod_id",
            "u64",
            core::mem::offset_of!(Self, mod_id),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "system_hash",
            "u64",
            core::mem::offset_of!(Self, system_hash),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "frame_seq",
            "u64",
            core::mem::offset_of!(Self, frame_seq),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "matched_entities",
            "u32",
            core::mem::offset_of!(Self, matched_entities),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "matched_tables",
            "u32",
            core::mem::offset_of!(Self, matched_tables),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "skipped_summary_tick_tables",
            "u32",
            core::mem::offset_of!(Self, skipped_summary_tick_tables),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "chunks",
            "u32",
            core::mem::offset_of!(Self, chunks),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "p50_us",
            "u32",
            core::mem::offset_of!(Self, p50_us),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "p99_us",
            "u32",
            core::mem::offset_of!(Self, p99_us),
        );
        acc = ComponentUuid::hash_field_schema(
            acc,
            "commands_queued",
            "u32",
            core::mem::offset_of!(Self, commands_queued),
        );
        ComponentUuid::hash_field_schema(
            acc,
            "reserved",
            "u32",
            core::mem::offset_of!(Self, reserved),
        )
    };

    /// Encodes this digest into a canonical 64-byte little-endian wire buffer.
    #[must_use]
    pub const fn to_le_bytes(&self) -> [u8; SYSTEM_TELEMETRY_DIGEST_SIZE] {
        let mut buf = [0u8; SYSTEM_TELEMETRY_DIGEST_SIZE];
        write_bytes_at(&mut buf, 0, self.magic);
        write_bytes_at(&mut buf, 4, self.version.to_le_bytes());
        write_bytes_at(&mut buf, 6, self.flags.to_le_bytes());
        write_bytes_at(&mut buf, 8, self.mod_id.to_le_bytes());
        write_bytes_at(&mut buf, 16, self.system_hash.to_le_bytes());
        write_bytes_at(&mut buf, 24, self.frame_seq.to_le_bytes());
        write_bytes_at(&mut buf, 32, self.matched_entities.to_le_bytes());
        write_bytes_at(&mut buf, 36, self.matched_tables.to_le_bytes());
        write_bytes_at(&mut buf, 40, self.skipped_summary_tick_tables.to_le_bytes());
        write_bytes_at(&mut buf, 44, self.chunks.to_le_bytes());
        write_bytes_at(&mut buf, 48, self.p50_us.to_le_bytes());
        write_bytes_at(&mut buf, 52, self.p99_us.to_le_bytes());
        write_bytes_at(&mut buf, 56, self.commands_queued.to_le_bytes());
        write_bytes_at(&mut buf, 60, self.reserved.to_le_bytes());
        buf
    }

    /// Decodes and validates a 64-byte little-endian wire buffer into [`SystemTelemetryDigest64`].
    pub const fn from_le_bytes(
        buf: &[u8; SYSTEM_TELEMETRY_DIGEST_SIZE],
    ) -> Result<Self, TelemetryFrameError> {
        let magic = [buf[0], buf[1], buf[2], buf[3]];
        if magic[0] != SYSTEM_TELEMETRY_MAGIC[0]
            || magic[1] != SYSTEM_TELEMETRY_MAGIC[1]
            || magic[2] != SYSTEM_TELEMETRY_MAGIC[2]
            || magic[3] != SYSTEM_TELEMETRY_MAGIC[3]
        {
            return Err(TelemetryFrameError::InvalidMagic(magic));
        }
        let version = u16::from_le_bytes([buf[4], buf[5]]);
        if version != SYSTEM_TELEMETRY_DIGEST_VERSION {
            return Err(TelemetryFrameError::UnsupportedVersion(version));
        }
        Ok(Self {
            magic,
            version,
            flags: u16::from_le_bytes([buf[6], buf[7]]),
            mod_id: u64::from_le_bytes([
                buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
            ]),
            system_hash: u64::from_le_bytes([
                buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
            ]),
            frame_seq: u64::from_le_bytes([
                buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
            ]),
            matched_entities: u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]),
            matched_tables: u32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]),
            skipped_summary_tick_tables: u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            chunks: u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]),
            p50_us: u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]),
            p99_us: u32::from_le_bytes([buf[52], buf[53], buf[54], buf[55]]),
            commands_queued: u32::from_le_bytes([buf[56], buf[57], buf[58], buf[59]]),
            reserved: u32::from_le_bytes([buf[60], buf[61], buf[62], buf[63]]),
        })
    }

    /// Decodes and validates a variable-length byte slice into [`SystemTelemetryDigest64`],
    /// rejecting slices whose length is not [`SYSTEM_TELEMETRY_DIGEST_SIZE`] (`64`).
    pub const fn from_le_slice(buf: &[u8]) -> Result<Self, TelemetrySliceError> {
        if buf.len() != SYSTEM_TELEMETRY_DIGEST_SIZE {
            return Err(TelemetrySliceError::InvalidLength(buf.len()));
        }
        let mut arr = [0u8; SYSTEM_TELEMETRY_DIGEST_SIZE];
        let mut i = 0;
        while i < SYSTEM_TELEMETRY_DIGEST_SIZE {
            arr[i] = buf[i];
            i += 1;
        }
        match Self::from_le_bytes(&arr) {
            Ok(digest) => Ok(digest),
            Err(err) => Err(TelemetrySliceError::Frame(err)),
        }
    }
}
