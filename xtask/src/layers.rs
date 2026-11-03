//! Workspace layering: one table, one check.
//!
//! Directory layout alone does not stop `storage` from importing `viewer`, or a
//! new crate from appearing in the wrong place. This module reads
//! `cargo metadata` and enforces two rules that the reorganisation relies on:
//!
//! 1. **Layering.** Every workspace-internal dependency must appear in
//!    [`ALLOWED`] for its crate. Dev-dependencies are exempt: a test may build
//!    a fixture from any crate without that becoming a production edge.
//! 2. **Naming.** A crate lives in a directory named after it with the
//!    `voxel-` prefix removed: `voxel-storage` in `crates/engine/storage`,
//!    `xtask` in `xtask`.
//!
//! The table is the source of truth. Changing an edge means changing the table
//! in the same commit, which is the review this check exists to force.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde::Deserialize;

/// Allowed production dependencies of each workspace crate.
///
/// Order is layer order: a crate may only depend on one above it in this list.
/// `voxel-ecs` depends on nothing in the engine; it is the CPU ECS the viewer
/// will adopt in M4.
const ALLOWED: &[(&str, &[&str])] = &[
    ("voxel-core", &[]),
    ("voxel-app-support", &[]),
    ("voxel-build-support", &[]),
    ("voxel-profiling", &[]),
    ("voxel-ecs", &[]),
    ("voxel-block", &["voxel-core"]),
    ("voxel-storage", &["voxel-core", "voxel-block"]),
    ("voxel-mesh", &["voxel-core", "voxel-storage"]),
    ("voxel-worldgen", &["voxel-core"]),
    ("voxel-texgraph", &["voxel-core", "voxel-block"]),
    ("voxel-render", &["voxel-block", "voxel-storage"]),
    (
        "voxel-viewer",
        &[
            "voxel-core",
            "voxel-app-support",
            "voxel-mesh",
            "voxel-storage",
            "voxel-texgraph",
            "voxel-profiling",
        ],
    ),
    (
        "voxel-launcher",
        &["voxel-app-support", "voxel-build-support"],
    ),
    (
        "voxel-texgen",
        &["voxel-app-support", "voxel-block", "voxel-texgraph"],
    ),
    (
        "voxel-bench",
        &[
            "voxel-core",
            "voxel-block",
            "voxel-mesh",
            "voxel-storage",
            "voxel-profiling",
            "voxel-viewer",
        ],
    ),
    ("xtask", &["voxel-build-support"]),
];

#[derive(Deserialize)]
struct Metadata {
    workspace_root: String,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: String,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    path: Option<String>,
    kind: Option<String>,
}

/// `cargo xtask check-layers`: fail when the workspace drifts from the table.
pub fn check() -> Result<(), Vec<String>> {
    let metadata = load()?;
    let mut problems = Vec::new();

    let known: BTreeMap<&str, &[&str]> = ALLOWED.iter().copied().collect();
    let mut present = BTreeSet::new();

    for package in &metadata.packages {
        present.insert(package.name.as_str());
        let Some(allowed) = known.get(package.name.as_str()) else {
            problems.push(format!(
                "`{}` is not in the layering table; add it to xtask/src/layers.rs",
                package.name
            ));
            continue;
        };

        check_directory(&metadata.workspace_root, package, &mut problems);

        for dependency in &package.dependencies {
            if dependency.path.is_none() || dependency.kind.is_some() {
                continue; // external, or a dev/build dependency
            }
            if !allowed.contains(&dependency.name.as_str()) {
                problems.push(format!(
                    "`{}` may not depend on `{}` (allowed: {})",
                    package.name,
                    dependency.name,
                    if allowed.is_empty() {
                        "none".to_owned()
                    } else {
                        allowed.join(", ")
                    }
                ));
            }
        }
    }

    for (name, _) in ALLOWED {
        if !present.contains(name) {
            problems.push(format!(
                "`{name}` is in the layering table but is not a workspace member"
            ));
        }
    }

    if problems.is_empty() {
        println!(
            "workspace layering ok: {} crates, {} internal edges",
            metadata.packages.len(),
            metadata
                .packages
                .iter()
                .map(|package| package
                    .dependencies
                    .iter()
                    .filter(|dependency| dependency.path.is_some() && dependency.kind.is_none())
                    .count())
                .sum::<usize>()
        );
        Ok(())
    } else {
        Err(problems)
    }
}

/// `voxel-storage` must live in a directory called `storage`.
fn check_directory(root: &str, package: &Package, problems: &mut Vec<String>) {
    let expected = package.name.strip_prefix("voxel-").unwrap_or(&package.name);
    let directory = std::path::Path::new(&package.manifest_path)
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str());
    if directory != Some(expected) {
        let relative = std::path::Path::new(&package.manifest_path)
            .strip_prefix(root)
            .unwrap_or_else(|_| std::path::Path::new(&package.manifest_path))
            .display();
        problems.push(format!(
            "`{}` must live in a directory named `{expected}` (found {relative})",
            package.name
        ));
    }
}

fn load() -> Result<Metadata, Vec<String>> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|error| vec![format!("could not run cargo metadata: {error}")])?;
    if !output.status.success() {
        return Err(vec![format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )]);
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| vec![format!("could not read cargo metadata: {error}")])
}
