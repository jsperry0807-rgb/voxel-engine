//! Workspace tasks that need a preflight before Cargo starts.
//!
//! Cargo aliases cannot run a check first, so `.cargo/config.toml` routes
//! `cargo build-win` through this binary. It verifies the
//! `x86_64-pc-windows-gnu` standard library and the mingw-w64 tools, reports
//! whatever is missing, and only then starts the real cross-build. Installing
//! is an explicit step: `cargo setup-win` (or `doctor --install`).

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use voxel_core::win_toolchain::{self, Missing, TARGET};

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
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--install" => install = true,
            "--no-install" => install = false,
            other if other == "--target" || other.starts_with("--target=") => {
                return usage_error(
                    "build-win fixes --target; run `cargo build --target …` directly",
                );
            }
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
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--install" => install = true,
            "--no-install" | "--check" => install = false,
            other => return usage_error(&format!("unknown doctor argument `{other}`")),
        }
    }

    let missing = win_toolchain::missing();
    if missing.is_empty() {
        println!("Windows cross-build toolchain is ready ({TARGET}).");
        return ExitCode::SUCCESS;
    }
    if !install {
        if let Some(summary) = missing.summary() {
            eprintln!("{summary}");
        }
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
    let missing = win_toolchain::missing();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(summary) = missing.summary() {
        eprintln!("{summary}");
    }

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

    let missing = win_toolchain::missing();
    if missing.is_empty() {
        Ok(())
    } else {
        if let Some(summary) = missing.summary() {
            eprintln!("{summary}");
        }
        Err("the installer finished, but the toolchain is still incomplete".to_owned())
    }
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
    if !win_toolchain::program_on_path("rustup") {
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
    if win_toolchain::program_on_path("apt-get") {
        return Ok(("apt-get", vec!["install", "-y", "mingw-w64"]));
    }
    if win_toolchain::program_on_path("dnf") {
        return Ok((
            "dnf",
            vec!["install", "-y", "mingw64-gcc", "mingw64-binutils"],
        ));
    }
    if win_toolchain::program_on_path("yum") {
        return Ok((
            "yum",
            vec!["install", "-y", "mingw64-gcc", "mingw64-binutils"],
        ));
    }
    if win_toolchain::program_on_path("pacman") {
        return Ok((
            "pacman",
            vec!["-S", "--needed", "--noconfirm", "mingw-w64-gcc"],
        ));
    }
    if win_toolchain::program_on_path("brew") {
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

    if win_toolchain::program_on_path("sudo") {
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
        if win_toolchain::program_on_path("pkexec") {
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

    if win_toolchain::program_on_path("pkexec") {
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
///
/// This mirrors [`run_privileged`]: if `pkexec` exists it is a usable path even
/// when `sudo` also exists but needs a password, which is exactly the launcher's
/// no-terminal case.
fn can_install() -> bool {
    is_root()
        || io::stdin().is_terminal()
        || passwordless_sudo()
        || win_toolchain::program_on_path("pkexec")
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
    win_toolchain::program_on_path("sudo")
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
