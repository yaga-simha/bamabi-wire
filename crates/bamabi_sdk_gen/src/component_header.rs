//! Shared C99 + C++17 + Zig generated component & VTable header renderer.

use core::fmt::Write as _;

use bamabi_sys::{BamFieldKind, ModComponent};

/// Safe RTTI field metadata descriptor used when emitting C99/C++17 or Zig component headers
/// under `#![forbid(unsafe_code)]` (re-export of [`bamabi_sys::BamFieldMeta`]).
pub type ComponentFieldMeta<'a> = bamabi_sys::BamFieldMeta<'a>;

/// Full metadata descriptor for one `#[derive(ModComponent)]` type used when rendering
/// metadata-rich C99/C++17 or Zig component headers.
#[derive(Debug, Copy, Clone)]
pub struct ComponentSection<'a> {
    /// Canonical namespaced component name (`ModComponent::CANONICAL_NAME`).
    pub canonical_name: &'a str,
    /// `true` iff the type is cross-language `#[repr(C)]` `AbiPod`.
    pub is_abi_pod: bool,
    /// Deterministic 128-bit UUID bytes (`ModComponent::UUID.bytes`).
    pub uuid_bytes: [u8; 16],
    /// Deterministic 64-bit structural schema fingerprint (`ModComponent::SCHEMA_FINGERPRINT`).
    pub schema_fingerprint: u64,
    /// Component registration flags (`ModComponent::COMPONENT_FLAGS`).
    pub component_flags: u32,
    /// Rust host byte size (`core::mem::size_of::<T>()`).
    pub size: usize,
    /// Rust host byte alignment (`core::mem::align_of::<T>()`).
    pub align: usize,
    /// Total reflected field count (`ModComponent::BAM_FIELDS.len()`).
    pub field_count: usize,
    /// Optional safe RTTI field descriptors for emitting `BamFieldDesc` tables.
    pub fields: &'a [ComponentFieldMeta<'a>],
    /// Auto-generated C99 struct, VTable, and static assert declarations (`ModComponent::C_DECL`).
    pub c_decl: &'a str,
    /// Auto-generated Zig `extern struct` and VTable declarations (`ModComponent::ZIG_DECL`).
    pub zig_decl: &'a str,
}

fn sanitize_c_upper(canonical_name: &str) -> String {
    let mut out = String::with_capacity(canonical_name.len());
    let mut last_underscore = false;
    for ch in canonical_name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "COMP_");
    }
    out
}

fn sanitize_c_lower(canonical_name: &str) -> String {
    let mut out = String::with_capacity(canonical_name.len());
    let mut last_underscore = false;
    for ch in canonical_name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "comp_");
    }
    out
}

fn sanitize_zig_ident(canonical_name: &str) -> String {
    let mut out = String::with_capacity(canonical_name.len());
    let mut capitalize_next = true;
    for ch in canonical_name.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize_next {
                out.push(ch.to_ascii_uppercase());
                capitalize_next = false;
            } else {
                out.push(ch);
            }
        } else {
            capitalize_next = true;
        }
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "Bam");
    }
    out
}

fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn c_field_kind_macro(kind: BamFieldKind) -> &'static str {
    match kind {
        BamFieldKind::BOOL => "BAM_FIELD_KIND_BOOL",
        BamFieldKind::U8 => "BAM_FIELD_KIND_U8",
        BamFieldKind::I8 => "BAM_FIELD_KIND_I8",
        BamFieldKind::U16 => "BAM_FIELD_KIND_U16",
        BamFieldKind::I16 => "BAM_FIELD_KIND_I16",
        BamFieldKind::U32 => "BAM_FIELD_KIND_U32",
        BamFieldKind::I32 => "BAM_FIELD_KIND_I32",
        BamFieldKind::U64 => "BAM_FIELD_KIND_U64",
        BamFieldKind::I64 => "BAM_FIELD_KIND_I64",
        BamFieldKind::F32 => "BAM_FIELD_KIND_F32",
        BamFieldKind::F64 => "BAM_FIELD_KIND_F64",
        BamFieldKind::USIZE => "BAM_FIELD_KIND_USIZE",
        BamFieldKind::ISIZE => "BAM_FIELD_KIND_ISIZE",
        BamFieldKind::STRING => "BAM_FIELD_KIND_STRING",
        BamFieldKind::VEC => "BAM_FIELD_KIND_VEC",
        BamFieldKind::FFI_OPTION => "BAM_FIELD_KIND_FFI_OPTION",
        BamFieldKind::NESTED => "BAM_FIELD_KIND_NESTED",
        BamFieldKind::OPAQUE => "BAM_FIELD_KIND_OPAQUE",
        _ => "BAM_FIELD_KIND_UNKNOWN",
    }
}

/// Renders a standalone C99 + C++17 (`#ifdef __cplusplus` RAII / typed views) component header
/// from a slice of `(canonical_name, is_abi_pod, c_decl)` sections.
///
/// Intended purpose: allows host crates to publish generated C/C++ component structs and VTable
/// declarations derived from `#[derive(ModComponent)]` so polyglot guests can access host
/// components with zero hand-written header maintenance.
///
/// What it does: wraps the provided `c_decl` snippets in `#ifndef <guard_macro>` include guards,
/// `#include "<abi_header_include>"`, and `extern "C"` blocks, annotating each section with its
/// canonical name and `IS_ABI_POD` flag.
#[must_use]
pub fn render_component_header(
    guard_macro: &str,
    abi_header_include: &str,
    sections: &[(&str, bool, &str)],
) -> String {
    let mut out = String::with_capacity(12 * 1024);
    out.push_str("/*\n");
    out.push_str(
        " * ============================================================================\n",
    );
    out.push_str(
        " * AUTO-GENERATED BY `cargo xtask gen-component-headers` — DO NOT EDIT BY HAND.\n",
    );
    out.push_str(" *\n");
    out.push_str(
        " * Source of truth: `ModComponent::C_DECL` emitted by `#[derive(ModComponent)]`\n",
    );
    out.push_str(" * in `crates/bamabi_macros`.\n");
    out.push_str(
        " * ============================================================================\n",
    );
    out.push_str(" */\n\n");
    out.push_str(&format!("#ifndef {guard_macro}\n"));
    out.push_str(&format!("#define {guard_macro}\n\n"));
    out.push_str(&format!("#include \"{abi_header_include}\"\n\n"));
    out.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");

    for &(canonical_name, is_abi_pod, c_decl) in sections {
        out.push_str(&format!(
            "/* Component: `{canonical_name}` (IS_ABI_POD = {is_abi_pod}) */\n"
        ));
        out.push_str(c_decl);
        out.push('\n');
    }

    out.push_str("#ifdef __cplusplus\n}\n#endif\n\n");
    out.push_str(&format!("#endif /* {guard_macro} */\n"));
    out
}

/// Renders a metadata-enriched C99 + C++17 component header emitting `ModComponent::C_DECL`
/// alongside deterministic UUID, schema fingerprint, size/align/flags macros, and
/// [`bamabi_sys::BamFieldDesc`] RTTI descriptor tables.
#[must_use]
pub fn render_component_header_with_metadata(
    guard_macro: &str,
    abi_header_include: &str,
    sections: &[ComponentSection<'_>],
) -> String {
    let mut out = String::with_capacity(16 * 1024);
    let _ = writeln!(
        out,
        "/*\n\
         * ============================================================================\n\
         * AUTO-GENERATED COMPONENT METADATA HEADER BY `bamabi_sdk_gen` — DO NOT EDIT.\n\
         *\n\
         * Source of truth: `ModComponent` constants emitted by `#[derive(ModComponent)]`.\n\
         * ============================================================================\n\
         */\n\n\
         #ifndef {guard_macro}\n\
         #define {guard_macro}\n\n\
         #include \"{abi_header_include}\"\n\n\
         #ifdef __cplusplus\n\
         extern \"C\" {{\n\
         #endif\n"
    );

    for sec in sections {
        let upper = sanitize_c_upper(sec.canonical_name);
        let lower = sanitize_c_lower(sec.canonical_name);
        let escaped_name = escape_c_string(sec.canonical_name);
        let _ = writeln!(
            out,
            "/* Component: `{}` (IS_ABI_POD = {}) */",
            sec.canonical_name, sec.is_abi_pod
        );
        if !sec.c_decl.is_empty() {
            out.push_str(sec.c_decl);
            if !sec.c_decl.ends_with('\n') {
                out.push('\n');
            }
        }
        let b = sec.uuid_bytes;
        let _ = writeln!(
            out,
            "#define BAM_COMP_{upper}_NAME \"{escaped_name}\"\n\
             #define BAM_COMP_{upper}_FLAGS 0x{:08X}u\n\
             #define BAM_COMP_{upper}_SCHEMA_FINGERPRINT 0x{:016X}ull\n\
             #define BAM_COMP_{upper}_SIZE {}u\n\
             #define BAM_COMP_{upper}_ALIGN {}u\n\
             #define BAM_COMP_{upper}_FIELD_COUNT {}u\n\
             static const BamComponentUuid BAM_UNUSED bam_comp_{lower}_uuid = {{\n\
             \x20   {{ 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u,\n\
             \x20     0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u, 0x{:02X}u }}\n\
             }};",
            sec.component_flags,
            sec.schema_fingerprint,
            sec.size,
            sec.align,
            sec.field_count.max(sec.fields.len()),
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15],
        );
        if !sec.fields.is_empty() {
            let _ = writeln!(
                out,
                "static const BamFieldDesc BAM_UNUSED bam_comp_{lower}_fields[{}] = {{",
                sec.fields.len()
            );
            for field in sec.fields {
                let fname = field.name;
                let ftag = field.type_tag;
                let _ = writeln!(
                    out,
                    "    {{ (uint32_t)sizeof(BamFieldDesc), {}, {{ (const uint8_t*)\"{}\", (size_t){}u }}, {{ (const uint8_t*)\"{}\", (size_t){}u }}, (size_t){}u, (size_t){}u, (size_t){}u }},",
                    c_field_kind_macro(field.kind),
                    escape_c_string(fname),
                    fname.len(),
                    escape_c_string(ftag),
                    ftag.len(),
                    field.offset,
                    field.size,
                    field.align,
                );
            }
            let _ = writeln!(out, "}};");
        }
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "#ifdef __cplusplus\n\
         }}\n\
         #endif\n\n\
         #endif /* {guard_macro} */"
    );
    out
}

/// Renders a standalone Zig component declaration file emitting `ModComponent::ZIG_DECL`,
/// UUIDs, structural schema fingerprints, flags, and RTTI [`bamabi_sys::BamFieldDesc`] metadata.
#[must_use]
pub fn render_zig_component_header(
    abi_zig_import: &str,
    sections: &[ComponentSection<'_>],
) -> String {
    let mut out = String::with_capacity(8 * 1024);
    let _ = writeln!(
        out,
        "// AUTO-GENERATED BY `bamabi_sdk_gen` — DO NOT EDIT BY HAND.\n\
         const std = @import(\"std\");\n\
         const bam = @import(\"{abi_zig_import}\");\n\
         const c = bam.c;\n"
    );

    for sec in sections {
        let zig_ident = sanitize_zig_ident(sec.canonical_name);
        let b = sec.uuid_bytes;
        let _ = writeln!(
            out,
            "// Component: `{}` (IS_ABI_POD = {})",
            sec.canonical_name, sec.is_abi_pod
        );
        if !sec.zig_decl.is_empty() {
            out.push_str(sec.zig_decl);
            if !sec.zig_decl.ends_with('\n') {
                out.push('\n');
            }
        }
        let _ = writeln!(
            out,
            "pub const {zig_ident}Meta = struct {{\n\
             \x20   pub const canonical_name: []const u8 = \"{}\";\n\
             \x20   pub const is_abi_pod: bool = {};\n\
             \x20   pub const component_flags: u32 = 0x{:08X};\n\
             \x20   pub const schema_fingerprint: u64 = 0x{:016X};\n\
             \x20   pub const size: usize = {};\n\
             \x20   pub const align_bytes: usize = {};\n\
             \x20   pub const field_count: usize = {};\n\
             \x20   pub const uuid: c.BamComponentUuid = .{{\n\
             \x20       .bytes = .{{ 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X} }},\n\
             \x20   }};",
            escape_c_string(sec.canonical_name),
            sec.is_abi_pod,
            sec.component_flags,
            sec.schema_fingerprint,
            sec.size,
            sec.align,
            sec.field_count.max(sec.fields.len()),
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15],
        );
        if sec.fields.is_empty() {
            let _ = writeln!(
                out,
                "    pub const fields: []const c.BamFieldDesc = &.{{}};"
            );
        } else {
            let _ = writeln!(out, "    pub const fields = [_]c.BamFieldDesc{{");
            for field in sec.fields {
                let fname = field.name;
                let ftag = field.type_tag;
                let _ = writeln!(
                    out,
                    "        .{{\n\
                     \x20           .struct_size = @sizeOf(c.BamFieldDesc),\n\
                     \x20           .kind = c.{},\n\
                     \x20           .name = .{{ .ptr = \"{}\".ptr, .len = {} }},\n\
                     \x20           .type_tag = .{{ .ptr = \"{}\".ptr, .len = {} }},\n\
                     \x20           .offset = {},\n\
                     \x20           .size = {},\n\
                     \x20           .@\"align\" = {},\n\
                     \x20       }},",
                    c_field_kind_macro(field.kind),
                    escape_c_string(fname),
                    fname.len(),
                    escape_c_string(ftag),
                    ftag.len(),
                    field.offset,
                    field.size,
                    field.align,
                );
            }
            let _ = writeln!(out, "    }};");
        }
        let _ = writeln!(out, "}};\n");
    }

    out
}

/// Extracts `(CANONICAL_NAME, IS_ABI_POD, C_DECL)` metadata for any [`ModComponent`].
///
/// Intended purpose: provides a concise compile-time adapter for building the `sections` slice
/// passed to [`render_component_header`].
///
/// What it does: returns the three associated constants from `T`'s [`ModComponent`] impl.
#[must_use]
pub const fn section_for<T: ModComponent>() -> (&'static str, bool, &'static str) {
    (T::CANONICAL_NAME, T::IS_ABI_POD, T::C_DECL)
}

/// Extracts complete metadata (UUID, schema fingerprint, flags, layout, field count, `C_DECL`,
/// and `ZIG_DECL`) for any [`ModComponent`].
#[must_use]
pub const fn component_section_for<T: ModComponent>() -> ComponentSection<'static> {
    ComponentSection {
        canonical_name: T::CANONICAL_NAME,
        is_abi_pod: T::IS_ABI_POD,
        uuid_bytes: T::UUID.bytes,
        schema_fingerprint: T::SCHEMA_FINGERPRINT,
        component_flags: T::COMPONENT_FLAGS,
        size: core::mem::size_of::<T>(),
        align: core::mem::align_of::<T>(),
        field_count: T::BAM_FIELDS.len(),
        fields: T::BAM_FIELD_META,
        c_decl: T::C_DECL,
        zig_decl: T::ZIG_DECL,
    }
}

#[cfg(test)]
mod tests {
    use bamabi_sys::ComponentUuid;

    use super::*;

    #[test]
    fn renders_c99_metadata_and_zig_component_headers_deterministically() {
        let fields = [
            ComponentFieldMeta {
                kind: BamFieldKind::F32,
                name: "x",
                type_tag: "f32",
                offset: 0,
                size: 4,
                align: 4,
            },
            ComponentFieldMeta {
                kind: BamFieldKind::F32,
                name: "y",
                type_tag: "f32",
                offset: 4,
                size: 4,
                align: 4,
            },
        ];
        let uuid = ComponentUuid::from_parts("game::Pos2", 8, 4, 0);
        let sections = [ComponentSection {
            canonical_name: "game::Pos2",
            is_abi_pod: true,
            uuid_bytes: uuid.bytes,
            schema_fingerprint: 0x0123_4567_89AB_CDEF,
            component_flags: 0x0000_0004,
            size: 8,
            align: 4,
            field_count: 2,
            fields: &fields,
            c_decl: "typedef struct GamePos2 { float x; float y; } GamePos2;\n",
            zig_decl: "pub const GamePos2 = extern struct { x: f32, y: f32 };\n",
        }];

        let c_meta = render_component_header_with_metadata("GAME_POS_H", "bamabi.h", &sections);
        assert!(c_meta.contains("#ifndef GAME_POS_H"));
        assert!(c_meta.contains("#define BAM_COMP_GAME_POS2_NAME \"game::Pos2\""));
        assert!(
            c_meta.contains("#define BAM_COMP_GAME_POS2_SCHEMA_FINGERPRINT 0x0123456789ABCDEFull")
        );
        assert!(c_meta.contains("#define BAM_COMP_GAME_POS2_FIELD_COUNT 2u"));
        assert!(c_meta.contains("bam_comp_game_pos2_uuid"));
        assert!(c_meta.contains("bam_comp_game_pos2_fields[2]"));

        let zig = render_zig_component_header("bamabi", &sections);
        assert!(zig.contains("const bam = @import(\"bamabi\");"));
        assert!(zig.contains("pub const GamePos2 = extern struct { x: f32, y: f32 };"));
        assert!(zig.contains("pub const GamePos2Meta = struct"));
        assert!(zig.contains("pub const schema_fingerprint: u64 = 0x0123456789ABCDEF;"));
        assert!(zig.contains("pub const fields = [_]c.BamFieldDesc"));
    }
}
