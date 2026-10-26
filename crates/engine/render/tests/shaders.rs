//! The WGSL contract must stay parseable, valid and numerically in step with
//! the CPU tables, without a GPU.
//!
//! WGSL has no include mechanism, so `svdag.wgsl` spells out the layout
//! constants the Rust tables own. [`shader_constants_match_the_cpu_tables`]
//! parses those declarations back out and compares them value for value;
//! [`shader_constants_cover_every_declaration`] keeps the mirror total, so a
//! shader constant with no Rust owner, or one that is declared but never used,
//! fails too.

use std::collections::BTreeMap;

use voxel_render::{MATERIAL_WGSL, SVDAG_WGSL};
use voxel_storage::dag::{NONE, ROOT_LEVEL, hdr_mask};
use voxel_storage::gpu::{
    BLOCKS_PER_CHUNK, EMPTY, NO_OVERLAY, NO_TABLE, OVERLAY_VALUES, OVERLAY_WORDS, SLOT_WORDS, TOMB,
};

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

/// One `const NAME: u32 = VALUEu;` declaration in a shader and the Rust item
/// that owns the value.
struct Mirrored {
    name: &'static str,
    value: u64,
    owner: &'static str,
}

/// Every constant `svdag.wgsl` mirrors, paired with the Rust item that owns it.
///
/// [`shader_constants_cover_every_declaration`] requires the two sets to match
/// exactly, so adding a shader constant means adding a row here and thereby
/// naming the Rust value it must follow.
fn mirrored_constants() -> [Mirrored; 11] {
    [
        Mirrored {
            name: "NONE",
            value: u64::from(NONE),
            owner: "voxel_storage::dag::NONE",
        },
        Mirrored {
            name: "EMPTY",
            value: u64::from(EMPTY),
            owner: "voxel_storage::gpu::EMPTY",
        },
        Mirrored {
            name: "TOMB",
            value: u64::from(TOMB),
            owner: "voxel_storage::gpu::TOMB",
        },
        Mirrored {
            name: "NO_OVERLAY",
            value: u64::from(NO_OVERLAY),
            owner: "voxel_storage::gpu::NO_OVERLAY",
        },
        Mirrored {
            name: "NO_TABLE",
            value: u64::from(NO_TABLE),
            owner: "voxel_storage::gpu::NO_TABLE",
        },
        Mirrored {
            name: "ROOT_LEVEL",
            value: u64::from(ROOT_LEVEL),
            owner: "voxel_storage::dag::ROOT_LEVEL",
        },
        Mirrored {
            name: "SLOT_WORDS",
            value: SLOT_WORDS as u64,
            owner: "voxel_storage::gpu::SLOT_WORDS",
        },
        Mirrored {
            name: "BLOCKS_PER_CHUNK",
            value: BLOCKS_PER_CHUNK as u64,
            owner: "voxel_storage::gpu::BLOCKS_PER_CHUNK",
        },
        Mirrored {
            name: "OVERLAY_VALUES",
            value: OVERLAY_VALUES as u64,
            owner: "voxel_storage::gpu::OVERLAY_VALUES",
        },
        Mirrored {
            name: "OVERLAY_WORDS",
            value: OVERLAY_WORDS as u64,
            owner: "voxel_storage::gpu::OVERLAY_WORDS",
        },
        Mirrored {
            name: "MASK_BITS",
            value: u64::from(hdr_mask(u32::MAX)),
            owner: "voxel_storage::dag::hdr_mask(u32::MAX)",
        },
    ]
}

/// Parses the literal `const NAME: u32 = VALUEu;` declarations of `source`.
///
/// The mirrored block declares one literal per line, so a line scan is enough;
/// a declaration that does not fit that shape panics instead of being skipped,
/// because a skipped constant would silently escape the mirror.
fn wgsl_constants(source: &str) -> BTreeMap<&str, u64> {
    let mut constants = BTreeMap::new();

    for (index, line) in source.lines().enumerate() {
        let line_no = index + 1;
        let Some(rest) = line.trim().strip_prefix("const ") else {
            continue;
        };
        let rest = rest
            .strip_suffix(';')
            .unwrap_or_else(|| panic!("line {line_no}: constant declaration needs `;`: {line}"));
        let (declared, value) = rest
            .split_once('=')
            .unwrap_or_else(|| panic!("line {line_no}: malformed constant: {line}"));
        let (name, ty) = declared
            .split_once(':')
            .unwrap_or_else(|| panic!("line {line_no}: malformed constant: {line}"));
        let name = name.trim();
        assert_eq!(
            ty.trim(),
            "u32",
            "line {line_no}: only `u32` constants are mirrored: {line}"
        );
        let literal = value.trim().strip_suffix('u').unwrap_or_else(|| {
            panic!("line {line_no}: constant `{name}` needs an unsigned literal: {line}")
        });
        let value = match literal.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16)
                .unwrap_or_else(|e| panic!("line {line_no}: bad hex for `{name}`: {e}")),
            None => literal
                .parse()
                .unwrap_or_else(|e| panic!("line {line_no}: bad value for `{name}`: {e}")),
        };
        assert!(
            constants.insert(name, value).is_none(),
            "line {line_no}: `{name}` is declared twice"
        );
    }

    constants
}

/// True when the executable body of `source` reads `name` as a whole
/// identifier.
///
/// Comments and `const` declarations do not count: a constant that only shows
/// up in prose, or only in its own declaration, is not actually driving the
/// shader.
fn body_uses(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        if code.trim_start().starts_with("const ") {
            return false;
        }
        code.match_indices(name).any(|(start, _)| {
            let before = code[..start].chars().next_back();
            let after = code[start + name.len()..].chars().next();
            let is_ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !is_ident(before) && !is_ident(after)
        })
    })
}

/// Asserts that every mirrored constant is declared in `source` with the value
/// its Rust owner has today.
#[test]
fn shader_constants_match_the_cpu_tables() {
    let declared = wgsl_constants(SVDAG_WGSL);

    for mirror in mirrored_constants() {
        let actual = declared.get(mirror.name).unwrap_or_else(|| {
            panic!(
                "svdag.wgsl is missing `const {}: u32 = {}u;` ({})",
                mirror.name, mirror.value, mirror.owner
            )
        });
        assert_eq!(
            *actual, mirror.value,
            "svdag.wgsl declares {} = {actual}u, but {} = {}; update the shader",
            mirror.name, mirror.owner, mirror.value
        );
    }
}

/// Every declaration in `svdag.wgsl` has a Rust owner, and every mirrored
/// constant is actually used by the shader body.
///
/// Without the first half a new shader constant could drift unnoticed; without
/// the second a stale declaration would satisfy the value check while the body
/// kept a hard-coded literal.
#[test]
fn shader_constants_cover_every_declaration() {
    let declared = wgsl_constants(SVDAG_WGSL);
    let mirrored = mirrored_constants();

    for mirror in &mirrored {
        assert!(
            declared.contains_key(mirror.name),
            "`{}` is in the mirror table but not declared in svdag.wgsl",
            mirror.name
        );
        // The declaration alone does not count: the body must read the
        // constant, not restate its value.
        assert!(
            body_uses(SVDAG_WGSL, mirror.name),
            "svdag.wgsl declares `{}` but never uses it; the body should read the constant",
            mirror.name
        );
    }

    for name in declared.keys() {
        assert!(
            mirrored.iter().any(|mirror| mirror.name == *name),
            "svdag.wgsl declares `{name}` with no Rust owner; add it to `mirrored_constants` or drop it"
        );
    }
}
