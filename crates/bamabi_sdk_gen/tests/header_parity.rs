//! Integration parity tests verifying checked-in C99 headers against `bamabi_sdk_gen`.

use std::fs;
use std::path::Path;

/// Test scenario: verifies strict byte-for-byte parity between `bamabi_sdk_gen::render_c99_header()`
/// and the checked-in repository artifact `include/bamabi.h`, as well as `write_or_check_header`
/// in `check_only = true` mode, catching any ABI drift between Rust (`bamabi_sys`) and C99.
#[test]
fn checked_in_c99_header_matches_render_c99_header_byte_for_byte() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate sits under <repo>/crates/bamabi_sdk_gen");
    let header_path = repo_root.join("include").join("bamabi.h");

    // Packaged standalone builds outside the repository checkout may not include ../../include/bamabi.h.
    if !header_path.is_file() {
        return;
    }

    let rendered = bamabi_sdk_gen::render_c99_header().expect("render_c99_header() must succeed");
    let on_disk = fs::read_to_string(&header_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", header_path.display()));

    if rendered != on_disk {
        let first_diff = rendered
            .lines()
            .zip(on_disk.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(idx, (a, b))| format!("line {}:\n  rendered: {a:?}\n  on_disk:  {b:?}", idx + 1))
            .unwrap_or_else(|| {
                format!(
                    "byte length mismatch (rendered={}, on_disk={})",
                    rendered.len(),
                    on_disk.len()
                )
            });
        panic!(
            "{} is out of date with bamabi_sdk_gen::render_c99_header():\n{first_diff}",
            header_path.display()
        );
    }

    assert_eq!(rendered.as_bytes(), on_disk.as_bytes());
    bamabi_sdk_gen::write_or_check_header(repo_root, true)
        .expect("write_or_check_header(..., true) must succeed");
}

/// Test scenario: when `include/generated/testbed_components.h` is present in the checkout,
/// verifies that it includes `"../bamabi.h"` so generated component headers resolve the root
/// ABI header cleanly without extra include-path flags.
#[test]
fn generated_testbed_components_header_includes_parent_bamabi_h() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate sits under <repo>/crates/bamabi_sdk_gen");
    let testbed_header = repo_root
        .join("include")
        .join("generated")
        .join("testbed_components.h");

    if !testbed_header.is_file() {
        return;
    }

    let content = fs::read_to_string(&testbed_header)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", testbed_header.display()));
    assert!(
        content
            .lines()
            .any(|line| line.trim() == r#"#include "../bamabi.h""#),
        "{} must include \"../bamabi.h\"",
        testbed_header.display()
    );
}
