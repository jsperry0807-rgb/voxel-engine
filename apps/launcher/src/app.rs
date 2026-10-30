//! The launcher window.
//!
//! The left column is the action list, the centre is the output of whichever
//! job is selected, and the header always shows the state of the Windows
//! cross-build. Starting an action never blocks: the [`Runner`] owns the child
//! process and the UI polls it once per frame.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use crate::jobs::{CommandSpec, Job, Runner, Status};

/// The Windows target, shared with `xtask` so the two entry points cannot drift.
const WINDOWS_TARGET: &str = voxel_core::win_toolchain::TARGET;
/// True when the Windows build is a cross-build (Unix host). On Windows the
/// "Windows build" is just the host build, with no mingw-w64 toolchain.
const CROSS_BUILD: bool = cfg!(unix);
const WINDOWS_SETUP: &str = "Windows toolchain";
const LINUX_BUILD: &str = "Linux build";
const WINDOWS_BUILD: &str = "Windows build";

/// Repaint cadence while a job is running and while the window is idle.
const RUNNING_REPAINT: Duration = Duration::from_millis(80);
const IDLE_REPAINT: Duration = Duration::from_millis(500);
/// Output lines rendered at once. The buffer keeps more, but a chatty viewer
/// log must not turn every frame into thousands of labels.
const VISIBLE_LINES: usize = 800;
/// Output text at the reference launcher size (1180×800). The log grows with
/// the window; the cap keeps long lines usable on a 4K display.
const LOG_TEXT_BASE: f32 = 15.0;
const LOG_TEXT_MAX_SCALE: f32 = 1.35;

const OK: Color32 = Color32::from_rgb(112, 190, 124);
const WARN: Color32 = Color32::from_rgb(226, 176, 92);
const FAIL: Color32 = Color32::from_rgb(235, 104, 104);
const MUTED: Color32 = Color32::from_rgb(140, 145, 152);

/// The launcher application state.
pub struct LauncherApp {
    root: PathBuf,
    runner: Runner,
    /// Job shown in the output panel. Newly started jobs take over.
    selected: Option<u64>,
    viewer: ViewerOptions,
    /// Whether the Linux and Windows builds start by themselves on the next
    /// frame.
    auto_build: bool,
    autostart_done: bool,
    windows_job: Option<u64>,
    linux_job: Option<u64>,
    /// The on-demand install job started by Build Windows or the setup button.
    windows_setup_job: Option<u64>,
    /// Cached preflight result; refreshed when a setup job finishes.
    windows_ready: bool,
    /// Start the cross-build as soon as an install job succeeds.
    build_after_setup: bool,
    /// A launcher-level message, such as a spawn failure.
    notice: Option<String>,
}

/// Options passed to the viewer as environment variables.
struct ViewerOptions {
    atlas: String,
    log: String,
    wayland: bool,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        Self {
            atlas: String::new(),
            log: "warn,voxel_viewer=info".to_owned(),
            wayland: false,
        }
    }
}

impl ViewerOptions {
    /// The environment for a viewer launch. The atlas path is translated for
    /// the Windows exe when WSL can do it.
    fn environment(&self, windows: bool) -> Vec<(String, String)> {
        let mut env = vec![("RUST_BACKTRACE".to_owned(), "1".to_owned())];
        let log = self.log.trim();
        if !log.is_empty() {
            env.push(("RUST_LOG".to_owned(), log.to_owned()));
        }
        let atlas = self.atlas.trim();
        if !atlas.is_empty() {
            let atlas = if windows {
                windows_path(atlas)
            } else {
                atlas.to_owned()
            };
            env.push(("VOXEL_VIEWER_ATLAS".to_owned(), atlas));
        }
        if self.wayland {
            env.push(("VOXEL_VIEWER_WAYLAND".to_owned(), "1".to_owned()));
        }
        env
    }
}

/// Everything the header needs about one job, copied out of the runner so the
/// UI can mutate it.
struct JobInfo {
    id: u64,
    label: String,
    status: Status,
    exit: Option<i32>,
    command: String,
    note: Option<String>,
    elapsed: Duration,
}

impl LauncherApp {
    /// Builds the launcher for a workspace root.
    pub fn new(cc: &eframe::CreationContext<'_>, root: PathBuf) -> Self {
        cc.egui_ctx.set_visuals(dark_visuals());
        Self {
            runner: Runner::new(root.clone()),
            root,
            selected: None,
            viewer: ViewerOptions::default(),
            auto_build: std::env::var_os("VOXEL_LAUNCHER_NO_AUTOBUILD").is_none(),
            autostart_done: false,
            windows_job: None,
            linux_job: None,
            windows_setup_job: None,
            windows_ready: !CROSS_BUILD || windows_toolchain_ready(),
            build_after_setup: false,
            notice: None,
        }
    }

    /// Starts the Linux and Windows builds once, on the first frame.
    ///
    /// They are separate jobs. The Windows build starts only when its
    /// toolchain is already present: autostart never installs packages, it
    /// points at the *Build Windows* button instead. Set
    /// `VOXEL_LAUNCHER_NO_AUTOBUILD` to keep both manual.
    fn autostart(&mut self) {
        if self.autostart_done {
            return;
        }
        self.autostart_done = true;
        if !self.auto_build {
            return;
        }
        // A launcher running natively on Windows has no cross toolchain to
        // install, so it just builds its own target.
        if cfg!(unix) {
            self.start_both_builds();
        } else {
            self.start_build(false);
        }
    }

    fn start(&mut self, spec: CommandSpec) {
        match self.runner.start(spec) {
            Ok(id) => {
                self.selected = Some(id);
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
    }

    fn start_build(&mut self, windows: bool) {
        if windows && CROSS_BUILD && !self.job_running(self.windows_setup_job) {
            // The toolchain can appear after startup (`cargo setup-win` in a
            // terminal), so re-check instead of trusting the cached preflight.
            self.windows_ready = windows_toolchain_ready();
        }
        if windows && !self.windows_ready {
            // Make the build button do the setup too, then continue the build
            // automatically when the installer job passes.
            self.build_after_setup = true;
            self.start_windows_setup();
            return;
        }
        let (label, args): (&str, Vec<&str>) = if windows {
            if CROSS_BUILD {
                // `build-win` runs the same preflight as the CLI, so the
                // launcher and `cargo build-win` cannot drift.
                (WINDOWS_BUILD, vec!["build-win"])
            } else {
                (WINDOWS_BUILD, vec!["build", "--workspace"])
            }
        } else {
            (LINUX_BUILD, vec!["build", "--workspace"])
        };
        match self.runner.start(CommandSpec::new(label, "cargo", &args)) {
            Ok(id) => {
                self.selected = Some(id);
                if windows {
                    self.windows_job = Some(id);
                } else {
                    self.linux_job = Some(id);
                }
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
    }

    /// Starts the Linux and Windows builds as independent jobs.
    ///
    /// A Windows build is only started when the toolchain is already in place.
    /// Installing is a deliberate action, so autostart leaves that to the
    /// *Build Windows* button rather than running a package manager.
    fn start_both_builds(&mut self) {
        self.start_build(false);
        if self.windows_ready {
            self.start_build(true);
        } else {
            self.notice =
                Some("Windows toolchain missing; press Build Windows to install it".to_owned());
        }
    }

    /// Installs mingw-w64 and the Rust target through `xtask`.
    fn start_windows_setup(&mut self) {
        if self.job_running(self.windows_setup_job) {
            return;
        }
        let spec = CommandSpec::new(WINDOWS_SETUP, "cargo", &["setup-win"])
            .note("installs mingw-w64 and the x86_64-pc-windows-gnu target when missing");
        match self.runner.start(spec) {
            Ok(id) => {
                self.selected = Some(id);
                self.windows_setup_job = Some(id);
                self.notice = None;
            }
            Err(error) => {
                self.build_after_setup = false;
                self.notice = Some(error);
            }
        }
    }

    /// Refreshes the cached preflight once an install job stops.
    fn finish_windows_setup(&mut self) {
        let Some(id) = self.windows_setup_job else {
            return;
        };
        let Some(job) = self.runner.job(id) else {
            self.windows_setup_job = None;
            return;
        };
        let status = job.status();
        if status.is_running() {
            return;
        }
        self.windows_setup_job = None;
        self.windows_ready = windows_toolchain_ready();
        if status == Status::Passed && self.windows_ready {
            if self.build_after_setup {
                self.build_after_setup = false;
                self.start_build(true);
            } else {
                self.notice = Some("Windows toolchain ready".to_owned());
            }
        } else {
            self.build_after_setup = false;
            self.notice = Some(
                "Windows toolchain setup failed; install mingw-w64 and the Rust target in a terminal"
                    .to_owned(),
            );
        }
    }

    fn start_viewer(&mut self, windows: bool) {
        let spec = if windows {
            CommandSpec::new(
                "Viewer (Windows)",
                self.windows_exe().display().to_string(),
                &[],
            )
            .note("native Win32 window from the cross-built exe; atlas paths are translated")
        } else {
            CommandSpec::new("Viewer (Linux)", "cargo", &["run", "-p", "voxel-viewer"])
                .note("pointer capture prefers X11/XWayland; enable Force Wayland to override")
        };
        let spec = spec.envs(self.viewer.environment(windows));
        self.start(spec);
    }

    fn start_texgen(&mut self) {
        self.start(
            CommandSpec::new(
                "Texture editor",
                "cargo",
                &["run", "--quiet", "-p", "voxel-texgen"],
            )
            .note("the editor writes only through the shared bake pipeline"),
        );
    }

    fn start_texgen_tool(&mut self, label: &str, command: &str) {
        self.start(CommandSpec::new(
            label,
            "cargo",
            &["run", "--quiet", "-p", "voxel-texgen", "--", command],
        ));
    }

    fn start_cargo(&mut self, label: &str, args: &[&str]) {
        self.start(CommandSpec::new(label, "cargo", args));
    }

    fn start_script(&mut self, label: &str, script: &str) {
        self.start(CommandSpec::new(label, script, &[]));
    }

    /// The Windows viewer produced by the background build.
    fn windows_exe(&self) -> PathBuf {
        let directory = if CROSS_BUILD {
            self.target_dir().join(WINDOWS_TARGET)
        } else {
            self.target_dir()
        };
        directory.join("debug").join("voxel-viewer.exe")
    }

    /// Command line shown in tooltips for the Windows build.
    fn windows_build_command(&self) -> String {
        if CROSS_BUILD {
            "cargo build-win".to_owned()
        } else {
            "cargo build --workspace".to_owned()
        }
    }

    fn target_dir(&self) -> PathBuf {
        match std::env::var_os("CARGO_TARGET_DIR") {
            Some(directory) => {
                let directory = PathBuf::from(directory);
                if directory.is_absolute() {
                    directory
                } else {
                    self.root.join(directory)
                }
            }
            None => self.root.join("target"),
        }
    }

    fn job_info(&self, id: Option<u64>) -> Option<JobInfo> {
        let job = self.runner.job(id?)?;
        Some(job_info(job))
    }

    /// True when the job behind `id` is still running.
    fn job_running(&self, id: Option<u64>) -> bool {
        id.and_then(|id| self.runner.job(id))
            .is_some_and(|job| job.status().is_running())
    }

    /// The Windows build status shown in the header.
    fn windows_state(&self) -> (&'static str, Color32) {
        if self.job_running(self.windows_setup_job) {
            return ("installing…", WARN);
        }
        if !self.windows_ready {
            return ("toolchain missing", FAIL);
        }
        match self.windows_job.and_then(|id| self.runner.job(id)) {
            Some(job) => state_of(job.status()),
            None if is_executable(&self.windows_exe()) => ("exe present", OK),
            None => ("not built", MUTED),
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Voxel Launcher");
                ui.label(
                    RichText::new(self.root.display().to_string())
                        .weak()
                        .monospace()
                        .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, color) = self.windows_state();
                    ui.colored_label(color, RichText::new(format!("Windows: {text}")).strong());
                    let building = self.job_running(self.windows_job)
                        || self.job_running(self.windows_setup_job);
                    if ui
                        .add_enabled(!building, egui::Button::new("Build Windows"))
                        .on_hover_text(if self.windows_ready {
                            self.windows_build_command()
                        } else {
                            "Installs mingw-w64 and the Rust target first, then builds".to_owned()
                        })
                        .clicked()
                    {
                        self.start_build(true);
                    }
                    let building = self.job_running(self.linux_job);
                    if ui
                        .add_enabled(!building, egui::Button::new("Build Linux"))
                        .on_hover_text("cargo build --workspace")
                        .clicked()
                    {
                        self.start_build(false);
                    }
                });
            });
            ui.add_space(3.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let running = self
                    .runner
                    .jobs()
                    .iter()
                    .filter(|job| job.status().is_running())
                    .count();
                if running > 0 {
                    ui.spinner();
                    ui.label(RichText::new(format!("{running} running")).weak());
                } else {
                    ui.label(RichText::new("idle").weak());
                }
                if let Some(notice) = self.notice.clone() {
                    ui.colored_label(FAIL, notice);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(if CROSS_BUILD {
                            WINDOWS_TARGET
                        } else {
                            "native"
                        })
                        .weak()
                        .monospace()
                        .small(),
                    );
                    let linux = self
                        .linux_job
                        .and_then(|id| self.runner.job(id))
                        .map(|job| job.status().label())
                        .unwrap_or("not run");
                    ui.label(RichText::new(format!("Linux: {linux}")).weak().small());
                });
            });
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        // Give the action list a little more room as the window grows, but
        // keep it from swallowing the output pane on a wide display.
        let width = (ctx.screen_rect().width() * 0.27).clamp(340.0, 460.0);
        egui::SidePanel::left("actions")
            .resizable(false)
            .exact_width(width)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.run_section(ui);
                        self.texture_section(ui);
                        self.verify_section(ui);
                        self.bench_section(ui);
                        self.build_section(ui);
                    });
            });
    }

    fn run_section(&mut self, ui: &mut egui::Ui) {
        section_heading(
            ui,
            "Run",
            "Linux is the main test surface. The Windows exe exists so the same build can be tried without WSL.",
        );
        if primary_button(ui, "Open viewer  ·  Linux", "cargo run -p voxel-viewer") {
            self.start_viewer(false);
        }
        let exe = self.windows_exe();
        let ready = is_executable(&exe);
        if action_button(
            ui,
            "Open viewer  ·  Windows",
            &format!(
                "{} (cross-built, runs natively through WSL interop)",
                exe.display()
            ),
            ready,
        ) {
            self.start_viewer(true);
        }
        if !ready {
            ui.label(
                RichText::new("Build the Windows target first: the button is in the header.")
                    .weak()
                    .small(),
            );
        }
        if action_button(ui, "Open texture editor", "cargo run -p voxel-texgen", true) {
            self.start_texgen();
        }
        egui::CollapsingHeader::new("Viewer options")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new("Atlas override").small().weak());
                ui.add(
                    egui::TextEdit::singleline(&mut self.viewer.atlas)
                        .hint_text("committed atlas")
                        .desired_width(f32::INFINITY),
                );
                ui.label(RichText::new("RUST_LOG").small().weak());
                ui.add(
                    egui::TextEdit::singleline(&mut self.viewer.log)
                        .desired_width(f32::INFINITY),
                );
                ui.checkbox(&mut self.viewer.wayland, "Force Wayland")
                    .on_hover_text("X11/XWayland is the default because pointer capture is more reliable there.");
                ui.label(
                    RichText::new(
                        "The atlas path is converted with wslpath when the Windows exe is launched.",
                    )
                    .weak()
                    .small(),
                );
            });
        ui.separator();
    }

    fn texture_section(&mut self, ui: &mut egui::Ui) {
        section_heading(
            ui,
            "Textures",
            "Every write goes through the same deterministic pipeline that `texgen check` verifies.",
        );
        if action_button(
            ui,
            "Check committed textures",
            "Fails on a one-byte difference",
            true,
        ) {
            self.start_texgen_tool("Texture check", "check");
        }
        if action_button(
            ui,
            "Generate sources + manifest",
            "Renders every graph, then writes textures.ron and materials.gen.ron",
            true,
        ) {
            self.start_texgen_tool("Texture generate", "generate");
        }
        if action_button(ui, "Bake atlas", "Writes the 256² atlas and its hash", true) {
            self.start_texgen_tool("Atlas bake", "bake-atlas");
        }
        if action_button(
            ui,
            "Update everything",
            "Generate then bake the atlas",
            true,
        ) {
            self.start_texgen_tool("Texture update", "update-all");
        }
        ui.separator();
    }

    fn verify_section(&mut self, ui: &mut egui::Ui) {
        section_heading(ui, "Verify", "The same checks CI runs on every push.");
        if action_button(ui, "Format check", "cargo fmt --all --check", true) {
            self.start_cargo("Format check", &["fmt", "--all", "--check"]);
        }
        if action_button(
            ui,
            "Clippy",
            "cargo clippy --workspace --all-targets -- -D warnings",
            true,
        ) {
            self.start_cargo(
                "Clippy",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            );
        }
        if action_button(ui, "Workspace tests", "cargo test --workspace", true) {
            self.start_cargo("Workspace tests", &["test", "--workspace"]);
        }
        if action_button(
            ui,
            "Texture manifest test",
            "cargo test -p voxel-block texture_manifest",
            true,
        ) {
            self.start_cargo(
                "Texture manifest test",
                &["test", "-p", "voxel-block", "texture_manifest"],
            );
        }
        if action_button(
            ui,
            "Shader tests",
            "cargo test -p voxel-render --test shaders",
            true,
        ) {
            self.start_cargo(
                "Shader tests",
                &["test", "-p", "voxel-render", "--test", "shaders"],
            );
        }
        if action_button(ui, "Chunk constants", "./scripts/check-constants.sh", true) {
            self.start_script("Chunk constants", "./scripts/check-constants.sh");
        }
        ui.separator();
    }

    fn bench_section(&mut self, ui: &mut egui::Ui) {
        section_heading(
            ui,
            "Bench",
            "Measurements are archived under benchmarks/ by the suite.",
        );
        if action_button(
            ui,
            "M1 gate report",
            "cargo run --release -p voxel-bench -- gate report",
            true,
        ) {
            self.start_cargo(
                "M1 gate report",
                &[
                    "run",
                    "--release",
                    "-p",
                    "voxel-bench",
                    "--",
                    "gate",
                    "report",
                ],
            );
        }
        if action_button(
            ui,
            "Full benchmark suite",
            "scripts/bench.sh: software lenses, Criterion, then optional Valgrind and perf. Long.",
            true,
        ) {
            self.start_script("Benchmark suite", "./scripts/bench.sh");
        }
        ui.separator();
    }

    fn build_section(&mut self, ui: &mut egui::Ui) {
        section_heading(
            ui,
            "Build",
            "Plain `cargo build` stays Linux. The Windows build is a separate target directory.",
        );
        if action_button(
            ui,
            "Build Linux (workspace)",
            "cargo build --workspace",
            !self.job_running(self.linux_job),
        ) {
            self.start_build(false);
        }
        if action_button(
            ui,
            "Build Windows (background)",
            if self.windows_ready {
                self.windows_build_command()
            } else {
                "Installs mingw-w64 and the Rust target first, then builds".to_owned()
            }
            .as_str(),
            !self.job_running(self.windows_job) && !self.job_running(self.windows_setup_job),
        ) {
            self.start_build(true);
        }
        if !self.windows_ready {
            if action_button(
                ui,
                "Install Windows toolchain",
                "cargo setup-win: mingw-w64 plus the Rust target; may ask for sudo",
                !self.job_running(self.windows_setup_job),
            ) {
                self.start_windows_setup();
            }
            ui.label(
                RichText::new(
                    "Build Windows installs these first; autostart never installs packages. CI checks the same preflight.",
                )
                .weak()
                .small(),
            );
        }
        let busy = self.job_running(self.linux_job)
            || self.job_running(self.windows_job)
            || self.job_running(self.windows_setup_job);
        if action_button(
            ui,
            "Build both (Linux + Windows)",
            "Starts both targets as separate jobs. They share host build scripts, so Cargo may briefly wait on its build lock.",
            !busy,
        ) {
            self.start_both_builds();
        }
        ui.checkbox(&mut self.auto_build, "Build Linux + Windows on launch")
            .on_hover_text(
                "Set VOXEL_LAUNCHER_NO_AUTOBUILD to keep this off without changing the box.",
            );
        let exe = self.windows_exe();
        if exe.is_file() {
            ui.add_space(4.0);
            ui.label(RichText::new("Windows exe").small().weak());
            ui.label(RichText::new(exe.display().to_string()).monospace().small());
            if ui
                .small_button("Copy exe path")
                .on_hover_text("Paste this path in a Windows terminal to test without WSL interop.")
                .clicked()
            {
                ui.ctx().copy_text(exe.display().to_string());
            }
        }
        ui.separator();
    }

    fn output_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.output_header(ui);
            ui.separator();
            self.output_log(ui);
        });
    }

    fn output_header(&mut self, ui: &mut egui::Ui) {
        let mut choice = self.selected;
        let info = self.job_info(self.selected);
        let selected_label = info
            .as_ref()
            .map(|info| format!("#{}  {}", info.id, info.label))
            .unwrap_or_else(|| "no output yet".to_owned());
        let mut stop = None;
        let mut clear = None;
        let mut copy = None;
        let mut copy_log = None;

        ui.horizontal_wrapped(|ui| {
            ui.heading("Output");
            egui::ComboBox::from_id_salt("jobs")
                .width(280.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    for job in self.runner.jobs().iter().rev() {
                        let text = format!(
                            "#{}  {}  ·  {}",
                            job.id(),
                            job.label(),
                            job.status().label()
                        );
                        ui.selectable_value(&mut choice, Some(job.id()), text);
                    }
                });
            if let Some(info) = &info {
                ui.colored_label(status_color(info.status), info.status.label());
                if info.status.is_running() {
                    ui.label(RichText::new(format!("{:.1}s", info.elapsed.as_secs_f32())).weak());
                } else if let Some(code) = info.exit {
                    ui.label(RichText::new(format!("exit {code}")).weak());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(info.status.is_running(), egui::Button::new("Stop"))
                        .clicked()
                    {
                        stop = Some(info.id);
                    }
                    if ui.button("Clear").clicked() {
                        clear = Some(info.id);
                    }
                    if ui.button("Copy command").clicked() {
                        copy = Some(info.command.clone());
                    }
                    if ui
                        .button("Copy log")
                        .on_hover_text("Copy the buffered output for a bug report")
                        .clicked()
                    {
                        copy_log = Some(info.id);
                    }
                });
            }
        });
        self.selected = choice;
        if let Some(id) = stop {
            self.runner.stop(id);
        }
        if let Some(id) = clear {
            self.runner.clear_output(id);
        }
        if let Some(command) = copy {
            ui.ctx().copy_text(command);
        }
        if let Some(id) = copy_log
            && let Some(job) = self.runner.job(id)
        {
            ui.ctx()
                .copy_text(job.lines().collect::<Vec<_>>().join("\n"));
        }

        if let Some(info) = &info {
            ui.label(RichText::new(&info.command).monospace().small().weak());
            if let Some(note) = &info.note {
                ui.label(RichText::new(note).small().weak());
            }
        }
    }

    fn output_log(&mut self, ui: &mut egui::Ui) {
        let size = log_text_size(ui);
        let Some(id) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Pick an action on the left. Its output appears here.")
                        .size(size)
                        .weak(),
                );
            });
            return;
        };
        let Some(job) = self.runner.job(id) else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("This job is no longer in the list.")
                        .size(size)
                        .weak(),
                );
            });
            return;
        };

        let total = job.lines().count();
        let skip = total.saturating_sub(VISIBLE_LINES);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = (size * 0.18).max(1.0);
                if skip > 0 {
                    ui.label(
                        RichText::new(format!("… {skip} earlier lines hidden"))
                            .size(size * 0.85)
                            .weak(),
                    );
                }
                let plain = ui.visuals().text_color();
                for line in job.lines().skip(skip) {
                    ui.label(
                        RichText::new(line)
                            .monospace()
                            .size(size)
                            .color(line_color(line, plain)),
                    );
                }
                if job.status().is_running() {
                    ui.label(RichText::new("…").size(size).weak());
                }
            });
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.autostart();
        self.runner.poll();
        self.finish_windows_setup();
        self.top_bar(ctx);
        self.status_bar(ctx);
        self.sidebar(ctx);
        self.output_panel(ctx);
        let running = self
            .runner
            .jobs()
            .iter()
            .any(|job| job.status().is_running());
        ctx.request_repaint_after(if running {
            RUNNING_REPAINT
        } else {
            IDLE_REPAINT
        });
    }
}

fn job_info(job: &Job) -> JobInfo {
    JobInfo {
        id: job.id(),
        label: job.label().to_owned(),
        status: job.status(),
        exit: job.exit(),
        command: job.command().to_owned(),
        note: job.note().map(str::to_owned),
        elapsed: job.elapsed(),
    }
}

fn state_of(status: Status) -> (&'static str, Color32) {
    match status {
        Status::Running => ("building…", WARN),
        Status::Passed => ("ready", OK),
        Status::Failed => ("failed", FAIL),
        Status::Stopped => ("stopped", MUTED),
    }
}

fn status_color(status: Status) -> Color32 {
    match status {
        Status::Running => WARN,
        Status::Passed => OK,
        Status::Failed => FAIL,
        Status::Stopped => MUTED,
    }
}

fn line_color(line: &str, plain: Color32) -> Color32 {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("panicked") {
        FAIL
    } else if lower.contains("warning") {
        WARN
    } else {
        plain
    }
}

fn section_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.label(RichText::new(subtitle).weak().small());
    ui.add_space(2.0);
}

fn primary_button(ui: &mut egui::Ui, text: &str, hint: &str) -> bool {
    let fill = ui.style().visuals.selection.bg_fill;
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(RichText::new(text).strong()).fill(fill),
    )
    .on_hover_text(hint)
    .clicked()
}

fn action_button(ui: &mut egui::Ui, text: &str, hint: &str, enabled: bool) -> bool {
    let width = ui.available_width();
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(egui::vec2(width, 26.0)),
    );
    let response = if enabled {
        response.on_hover_text(hint)
    } else {
        response.on_disabled_hover_text(hint)
    };
    response.clicked()
}

/// Font size for the output log. It follows the window size, with a floor at
/// the reference size and a ceiling so long lines still fit on a wide display.
fn log_text_size(ui: &egui::Ui) -> f32 {
    log_text_size_for(ui.available_width(), ui.available_height())
}

fn log_text_size_for(width: f32, height: f32) -> f32 {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let scale = (width / 900.0)
        .min(height / 700.0)
        .clamp(1.0, LOG_TEXT_MAX_SCALE);
    LOG_TEXT_BASE * scale
}

/// True when both the Rust target and every mingw-w64 tool are installed.
///
/// The probe itself lives in `voxel_core::win_toolchain`, shared with `xtask`.
fn windows_toolchain_ready() -> bool {
    voxel_core::win_toolchain::missing().is_empty()
}

/// True when `path` is a file with an execute bit.
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

/// Converts a WSL path to a Windows path for the cross-built exe. Leaves the
/// value alone when `wslpath` is unavailable or the path is already Windows.
#[cfg(unix)]
fn windows_path(path: &str) -> String {
    if !path.starts_with('/') {
        return path.to_owned();
    }
    std::process::Command::new("wslpath")
        .arg("-w")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|converted| !converted.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(not(unix))]
fn windows_path(path: &str) -> String {
    path.to_owned()
}

fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(24, 26, 30);
    visuals.window_fill = Color32::from_rgb(30, 33, 38);
    visuals.extreme_bg_color = Color32::from_rgb(16, 18, 21);
    visuals.selection.bg_fill = Color32::from_rgb(54, 112, 158);
    visuals.hyperlink_color = Color32::from_rgb(111, 186, 230);
    visuals
}

#[cfg(test)]
mod tests {
    use super::{LOG_TEXT_BASE, LOG_TEXT_MAX_SCALE, log_text_size_for};

    #[test]
    fn log_text_keeps_a_readable_floor() {
        assert_eq!(log_text_size_for(900.0, 700.0), LOG_TEXT_BASE);
        assert_eq!(log_text_size_for(600.0, 400.0), LOG_TEXT_BASE);
    }

    #[test]
    fn log_text_grows_with_the_window_and_stops_at_the_cap() {
        let wide = log_text_size_for(1_200.0, 800.0);
        assert!(wide > LOG_TEXT_BASE);
        assert!(wide < LOG_TEXT_BASE * LOG_TEXT_MAX_SCALE);
        assert_eq!(
            log_text_size_for(8_000.0, 8_000.0),
            LOG_TEXT_BASE * LOG_TEXT_MAX_SCALE
        );
    }
}
