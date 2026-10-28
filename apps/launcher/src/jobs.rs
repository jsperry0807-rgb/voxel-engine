//! Child processes and their captured output.
//!
//! The UI thread never waits on a build. Each action becomes a [`Job`] whose
//! stdout and stderr are drained on reader threads into a bounded line buffer,
//! and whose exit status is polled once per frame. On Unix the child starts in
//! its own process group, so Stop also stops a viewer that Cargo has already
//! handed off to.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// Lines kept per job. Cargo is chatty; this is a readable tail, not a log file.
const LINE_LIMIT: usize = 4_000;
/// Finished jobs kept in the picker.
const JOB_LIMIT: usize = 12;

/// How a job ended, or that it is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Passed,
    Failed,
    Stopped,
}

impl Status {
    pub fn is_running(self) -> bool {
        matches!(self, Status::Running)
    }

    /// A short label for the job picker and the status line.
    pub fn label(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Stopped => "stopped",
        }
    }
}

/// One command to start: program, arguments, extra environment, and the label
/// shown in the job picker.
#[derive(Clone)]
pub struct CommandSpec {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub note: Option<String>,
}

impl CommandSpec {
    pub fn new(label: impl Into<String>, program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: Vec::new(),
            note: None,
        }
    }

    pub fn envs(mut self, pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env.extend(pairs);
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// A shell-style preview of exactly what will run.
    pub fn display(&self) -> String {
        let mut parts = Vec::with_capacity(1 + self.env.len() + self.args.len());
        for (key, value) in &self.env {
            parts.push(format!("{key}={}", quote(value)));
        }
        parts.push(quote(&self.program));
        parts.extend(self.args.iter().map(|arg| quote(arg)));
        parts.join(" ")
    }
}

fn quote(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || "\"'\\$`".contains(character));
    if needs_quotes {
        format!("'{}'", value.replace('\'', "'\\''"))
    } else {
        value.to_owned()
    }
}

/// A started process and its combined output.
pub struct Job {
    id: u64,
    label: String,
    command: String,
    note: Option<String>,
    status: Status,
    exit: Option<i32>,
    lines: VecDeque<String>,
    child: Option<Child>,
    events: Receiver<Event>,
    started: Instant,
    finished: Option<Instant>,
}

impl Job {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// The command line shown in the header, environment included.
    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    pub fn elapsed(&self) -> Duration {
        self.finished
            .unwrap_or_else(Instant::now)
            .duration_since(self.started)
    }

    /// Drains everything the readers have delivered since the last frame.
    fn drain(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(Event::Line(line)) => {
                    if self.lines.len() == LINE_LIMIT {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(line);
                }
                Ok(Event::Closed) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Reaps a child that has exited. Late output is still drained afterward.
    fn refresh(&mut self) {
        if self.status != Status::Running {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(exit)) => {
                self.exit = exit.code();
                self.status = if exit.success() {
                    Status::Passed
                } else {
                    Status::Failed
                };
                self.finished = Some(Instant::now());
                self.child = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.lines
                    .push_back(format!("[launcher] could not read exit status: {error}"));
                self.status = Status::Failed;
                self.finished = Some(Instant::now());
                self.child = None;
            }
        }
    }
}

/// Messages from the two reader threads.
enum Event {
    Line(String),
    Closed,
}

/// Owns every job for the session.
pub struct Runner {
    root: PathBuf,
    jobs: Vec<Job>,
    next_id: u64,
}

impl Runner {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    /// Starts a command in the workspace root.
    pub fn start(&mut self, spec: CommandSpec) -> Result<u64, String> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        // A viewer launched by `cargo run` is a grandchild; the group lets
        // Stop reach it.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|error| {
            format!(
                "could not start `{}` in {}: {error}",
                spec.program,
                self.root.display()
            )
        })?;

        let (sender, events) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            pump(stdout, sender.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pump(stderr, sender.clone());
        }
        drop(sender);

        self.prune();
        let id = self.next_id;
        self.next_id += 1;
        let command = spec.display();
        self.jobs.push(Job {
            id,
            label: spec.label,
            command,
            note: spec.note,
            status: Status::Running,
            exit: None,
            lines: VecDeque::new(),
            child: Some(child),
            events,
            started: Instant::now(),
            finished: None,
        });
        Ok(id)
    }

    /// Reaps children and drains output. Call once per frame.
    pub fn poll(&mut self) {
        for job in &mut self.jobs {
            job.drain();
            job.refresh();
        }
    }

    /// Stops a running job, including its process group.
    pub fn stop(&mut self, id: u64) {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) else {
            return;
        };
        if job.status != Status::Running {
            return;
        }
        if let Some(mut child) = job.child.take() {
            terminate(&mut child);
        }
        job.status = Status::Stopped;
        job.finished = Some(Instant::now());
        push_notice(job, "[launcher] stopped");
    }

    /// Discards a job's buffered output without touching the process.
    pub fn clear_output(&mut self, id: u64) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
            job.lines.clear();
        }
    }

    /// Keeps at most [`JOB_LIMIT`] jobs by dropping the oldest finished one.
    fn prune(&mut self) {
        while self.jobs.len() >= JOB_LIMIT {
            match self.jobs.iter().position(|job| !job.status.is_running()) {
                Some(index) => {
                    self.jobs.remove(index);
                }
                None => break,
            }
        }
    }
}

fn push_notice(job: &mut Job, line: &str) {
    if job.lines.len() == LINE_LIMIT {
        job.lines.pop_front();
    }
    job.lines.push_back(line.to_owned());
}

/// Feeds one pipe into the channel, line by line.
fn pump<R: Read + Send + 'static>(stream: R, sender: Sender<Event>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            if sender.send(Event::Line(line)).is_err() {
                return;
            }
        }
        let _ = sender.send(Event::Closed);
    });
}

/// Kills a child and, on Unix, the process group it leads.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // `process_group(0)` made the child a group leader, so a negative pid
        // reaches Cargo and anything it spawned (for example the viewer).
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::Runner;
    use super::{CommandSpec, Status};
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    fn display_quotes_environment_and_arguments() {
        let spec = CommandSpec::new("check", "cargo", &["run", "-p", "voxel-texgen"]).envs([
            ("RUST_LOG".to_owned(), "warn, voxel_viewer=info".to_owned()),
            ("PLAIN".to_owned(), "value".to_owned()),
        ]);
        assert_eq!(
            spec.display(),
            "RUST_LOG='warn, voxel_viewer=info' PLAIN=value cargo run -p voxel-texgen"
        );
    }

    #[test]
    fn running_status_is_the_only_live_one() {
        assert!(Status::Running.is_running());
        assert!(!Status::Passed.is_running());
        assert!(!Status::Failed.is_running());
        assert!(!Status::Stopped.is_running());
    }

    #[cfg(unix)]
    #[test]
    fn job_captures_both_streams_and_the_exit_status() {
        let mut runner = Runner::new(std::env::temp_dir());
        let id = runner
            .start(CommandSpec::new(
                "output",
                "sh",
                &["-c", "echo hello; echo oops >&2; exit 3"],
            ))
            .expect("sh should start");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            runner.poll();
            let job = runner.job(id).expect("job should exist");
            let finished = !job.status().is_running();
            if finished && job.lines().count() >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "job did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        let job = runner.job(id).expect("job should exist");
        assert_eq!(job.status(), Status::Failed);
        assert_eq!(job.exit(), Some(3));
        assert!(job.lines().any(|line| line == "hello"));
        assert!(job.lines().any(|line| line == "oops"));
    }

    #[cfg(unix)]
    #[test]
    fn stop_ends_a_running_job() {
        let mut runner = Runner::new(std::env::temp_dir());
        let id = runner
            .start(CommandSpec::new("sleep", "sleep", &["30"]))
            .expect("sleep should start");
        std::thread::sleep(Duration::from_millis(100));
        runner.stop(id);
        assert_eq!(
            runner.job(id).expect("job should exist").status(),
            Status::Stopped
        );
    }
}
