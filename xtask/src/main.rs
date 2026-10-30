//! Workspace tasks that need a preflight before Cargo starts.
//!
//! Cargo aliases cannot run a check first, so `.cargo/config.toml` routes
//! `cargo build-win` through this binary. It verifies the
//! `x86_64-pc-windows-gnu` standard library and the mingw-w64 tools, reports
//! whatever is missing, and only then starts the real cross-build. Installing
//! is an explicit step: `cargo setup-win` (or `doctor --install`).

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// The target the Windows cross-build produces.
const TARGET: &str = "x86_64-pc-windows-gnu";
/// Tools provided by mingw-w64. `gcc` drives the link and reaches for the
/// others by name, so a partial install fails late and confusingly; check all
/// of them up front.
const TOOLS: &[&str] = &[
    "x86_64-w64-mingw32-gcc",
    "x86_64-w64-mingw32-ld",
    "x86_64-w64-mingw32-ar",
    "x86_64-w64-mingw32-dlltool",
    "x86_64-w64-mingw32-windres",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);
    let rest = args.get(1..).unwrap_or_default();
    match command {
        Some("build-win") => build_win(rest),
        Some("doctor") => doctor(rest),
        Some("-h" | "--help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => usage_error(&format!("unknown command `{other}`")),
        None => {
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// `cargo build-win`: ensure the toolchain, then run the cross-build.
///
/// A build never installs packages on its own; a missing prerequisite fails
/// with the `cargo setup-win` instruction instead.
fn build_win(args: &[String]) -> ExitCode {
    let mut install = false;
    let mut cargo_args = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--install" => install = true,
            "--no-install" => install = false,
            _ => cargo_args.push(arg.clone()),
        }
    }

    if let Err(error) = ensure_toolchain(install) {
        eprintln!("xtask: {error}");
        return ExitCode::FAILURE;
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(["build", "--workspace", "--target", TARGET])
        .args(&cargo_args)
        .current_dir(workspace_root());
    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => exit_code(status.code()),
        Err(error) => {
            eprintln!("xtask: could not start cargo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask doctor`: report the toolchain, optionally install it.
fn doctor(args: &[String]) -> ExitCode {
    let mut install = false;
    for arg in args {
        match arg.as_str() {
            "--install" => install = true,
            "--no-install" | "--check" => install = false,
            other => return usage_error(&format!("unknown doctor argument `{other}`")),
        }
    }

    let missing = missing_toolchain();
    if missing.is_empty() {
        println!("Windows cross-build toolchain is ready ({TARGET}).");
        return ExitCode::SUCCESS;
    }
    if !install {
        missing.report();
        eprintln!("xtask: run `cargo setup-win` to install the missing pieces.");
        return ExitCode::FAILURE;
    }
    if let Err(error) = ensure_toolchain(true) {
        eprintln!("xtask: {error}");
        return ExitCode::FAILURE;
    }
    println!("Windows cross-build toolchain is ready ({TARGET}).");
    ExitCode::SUCCESS
}

fn ensure_toolchain(install: bool) -> Result<(), String> {
    let missing = missing_toolchain();
    if missing.is_empty() {
        return Ok(());
    }
    missing.report();

    if !install {
        return Err(
            "the Windows cross-build needs mingw-w64 and the Rust target; \
             run `cargo setup-win` or rerun with --install"
                .to_owned(),
        );
    }
    if !can_install() {
        return Err("installing needs sudo, but this process has no terminal; \
             run `cargo setup-win` in a terminal"
            .to_owned());
    }

    install_missing(&missing)?;

    let missing = missing_toolchain();
    if missing.is_empty() {
        Ok(())
    } else {
        missing.report();
        Err("the installer finished, but the toolchain is still incomplete".to_owned())
    }
}

/// What the Windows cross-build is missing right now.
struct Missing {
    target: bool,
    tools: Vec<&'static str>,
}

impl Missing {
    fn is_empty(&self) -> bool {
        !self.target && self.tools.is_empty()
    }

    fn report(&self) {
        eprintln!("Windows cross-build prerequisites are missing:");
        if self.target {
            eprintln!("  - Rust target {TARGET} (rustup target add {TARGET})");
        }
        if !self.tools.is_empty() {
            eprintln!("  - mingw-w64: {}", self.tools.join(", "));
        }
    }
}

fn missing_toolchain() -> Missing {
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
fn target_installed() -> bool {
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

fn install_missing(missing: &Missing) -> Result<(), String> {
    if missing.target {
        install_target()?;
    }
    if !missing.tools.is_empty() {
        install_mingw()?;
    }
    Ok(())
}

fn install_target() -> Result<(), String> {
    if !program_on_path("rustup") {
        return Err(format!(
            "the Rust target is missing and rustup is not on PATH; \
             install {TARGET} with your Rust distribution"
        ));
    }
    eprintln!("xtask: rustup target add {TARGET}");
    run(
        with_args("rustup", &["target", "add", TARGET]),
        "rustup target add",
    )
}

fn install_mingw() -> Result<(), String> {
    let (program, args) = mingw_install_command()?;
    eprintln!("xtask: installing mingw-w64 with {program}");
    match run_privileged(program, &args) {
        Ok(()) => Ok(()),
        // Package lists in a fresh container are often empty. Retry apt once
        // after an update before giving up.
        Err(error) if program == "apt-get" => {
            eprintln!("xtask: {error}; refreshing package lists and retrying");
            run_privileged("apt-get", &["update"])?;
            run_privileged(program, &args)
        }
        Err(error) => Err(error),
    }
}

fn mingw_install_command() -> Result<(&'static str, Vec<&'static str>), String> {
    if program_on_path("apt-get") {
        return Ok(("apt-get", vec!["install", "-y", "mingw-w64"]));
    }
    if program_on_path("dnf") {
        return Ok((
            "dnf",
            vec!["install", "-y", "mingw64-gcc", "mingw64-binutils"],
        ));
    }
    if program_on_path("yum") {
        return Ok((
            "yum",
            vec!["install", "-y", "mingw64-gcc", "mingw64-binutils"],
        ));
    }
    if program_on_path("pacman") {
        return Ok((
            "pacman",
            vec!["-S", "--needed", "--noconfirm", "mingw-w64-gcc"],
        ));
    }
    if program_on_path("brew") {
        return Ok(("brew", vec!["install", "mingw-w64"]));
    }
    Err("no supported package manager found; install mingw-w64 \
         (the x86_64-w64-mingw32-* tools) manually"
        .to_owned())
}

/// Runs an installer, adding sudo (or polkit) only when the host needs it.
fn run_privileged(program: &str, args: &[&str]) -> Result<(), String> {
    if program == "brew" || is_root() {
        return run(with_args(program, args), program);
    }

    if program_on_path("sudo") {
        let passwordless = passwordless_sudo();
        // Passwordless sudo works from the launcher's no-terminal job too.
        if passwordless || io::stdin().is_terminal() {
            let mut command = Command::new("sudo");
            if passwordless {
                command.arg("-n");
            }
            command.arg(program).args(args);
            return run(command, program);
        }
        if program_on_path("pkexec") {
            let mut command = Command::new("pkexec");
            command.arg(program).args(args);
            return run(command, program);
        }
        return Err(
            "sudo needs a terminal and passwordless sudo is not configured; \
             run `cargo setup-win` in a terminal"
                .to_owned(),
        );
    }

    if program_on_path("pkexec") {
        let mut command = Command::new("pkexec");
        command.arg(program).args(args);
        return run(command, program);
    }
    Err(format!(
        "cannot install system packages without sudo or pkexec; \
         run the {program} command manually"
    ))
}

fn with_args(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    command
}

/// True when the toolchain can be installed without an interactive terminal.
fn can_install() -> bool {
    is_root()
        || io::stdin().is_terminal()
        || passwordless_sudo()
        || (!program_on_path("sudo") && program_on_path("pkexec"))
}

fn run(mut command: Command, what: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("could not run `{what}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{what}` failed with {status}"))
    }
}

/// True when `name` resolves to an executable on `PATH`.
fn program_on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let path = dir.join(name);
            path.is_file() && is_executable(&path)
        })
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

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

fn passwordless_sudo() -> bool {
    program_on_path("sudo")
        && Command::new("sudo")
            .args(["-n", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

fn exit_code(code: Option<i32>) -> ExitCode {
    let code = code.unwrap_or(1);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("xtask: {message}");
    print_help();
    ExitCode::FAILURE
}

fn print_help() {
    println!(
        "xtask: workspace preflight tasks\n\
         \n\
         usage:\n\
           cargo build-win [--install] [cargo build args...]     cross-build Windows\n\
           cargo setup-win                                       install the cross toolchain\n\
           cargo xtask doctor [--install]                        check the cross toolchain\n\
         \n\
         `cargo build-win` checks mingw-w64 and the {TARGET}\n\
         Rust target and fails with instructions when something is missing.\n\
         Nothing is installed unless you pass --install or run `cargo setup-win`."
    );
}
