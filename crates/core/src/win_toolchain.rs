//! Windows cross-build preflight, shared by `xtask` and the launcher.
//!
//! Both entry points used to carry their own copy of the target, the tool list
//! and the `PATH` probe, so adding or renaming a mingw tool silently made one
//! of them wrong. They now ask this module.

use std::path::Path;
use std::process::{Command, Stdio};

/// The target the Windows cross-build produces.
pub const TARGET: &str = "x86_64-pc-windows-gnu";

/// Tools provided by mingw-w64. `gcc` drives the link and reaches for the
/// others by name, so a partial install fails late and confusingly; check all
/// of them up front.
pub const TOOLS: &[&str] = &[
    "x86_64-w64-mingw32-gcc",
    "x86_64-w64-mingw32-ld",
    "x86_64-w64-mingw32-ar",
    "x86_64-w64-mingw32-dlltool",
    "x86_64-w64-mingw32-windres",
];

/// What the Windows cross-build is missing right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Missing {
    /// The Rust standard library for [`TARGET`].
    pub target: bool,
    /// mingw-w64 tools that are not on `PATH`.
    pub tools: Vec<&'static str>,
}

impl Missing {
    /// True when the target and every tool are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.target && self.tools.is_empty()
    }

    /// The human-readable report for a terminal, or `None` when ready.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut text = String::from("Windows cross-build prerequisites are missing:");
        if self.target {
            text.push_str(&format!(
                "\n  - Rust target {TARGET} (rustup target add {TARGET})"
            ));
        }
        if !self.tools.is_empty() {
            text.push_str(&format!("\n  - mingw-w64: {}", self.tools.join(", ")));
        }
        Some(text)
    }
}

/// Probe the target and every mingw tool.
#[must_use]
pub fn missing() -> Missing {
    Missing {
        target: !target_installed(),
        tools: TOOLS
            .iter()
            .copied()
            .filter(|tool| !program_on_path(tool))
            .collect(),
    }
}

/// True when `rustc` has the standard library for the cross target.
#[must_use]
pub fn target_installed() -> bool {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let Ok(output) = Command::new(rustc)
        .args(["--print", "sysroot"])
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let sysroot = String::from_utf8_lossy(&output.stdout);
    Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(TARGET)
        .join("lib")
        .is_dir()
}

/// True when `name` resolves to an executable on `PATH`.
#[must_use]
pub fn program_on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(name)))
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
