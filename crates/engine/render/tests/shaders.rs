//! The WGSL contract must stay parseable and valid without a GPU.

use voxel_render::{MATERIAL_WGSL, SVDAG_WGSL};

fn validate(name: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{name} does not parse:\n{e}"));
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} does not validate: {e:?}"));
}

#[test]
fn svdag_shader_is_valid_wgsl() {
    validate("svdag.wgsl", SVDAG_WGSL);
}

#[test]
fn material_shader_is_valid_wgsl() {
    validate("material.wgsl", MATERIAL_WGSL);
}

#[test]
fn shader_bindings_use_separate_groups() {
    // The two units are concatenated into one pipeline module, so a shared
    // group must be impossible by construction.
    assert!(SVDAG_WGSL.contains("@group(0)"));
    assert!(MATERIAL_WGSL.contains("@group(1)"));
    assert!(!SVDAG_WGSL.contains("@group(1)"));
    assert!(!MATERIAL_WGSL.contains("@group(0)"));
}
