//! Process-level CPU and resident-memory readings.
//!
//! Heap counters ([`crate::mem`]) say what the data structures cost; these say
//! what the *process* costs, including allocator slack, thread stacks, mapped
//! pages and the runtime. All readings are best-effort: a platform without
//! `/proc` (or without `getrusage`) reports `None` instead of failing the run.

use std::time::{Duration, Instant};

/// Process-wide resident set size, in bytes.
#[cfg(target_os = "linux")]
pub fn rss_bytes() -> Option<u64> {
    proc_status_bytes("VmRSS:")
}

/// Peak process-wide resident set size, in bytes.
#[cfg(target_os = "linux")]
pub fn peak_rss_bytes() -> Option<u64> {
    proc_status_bytes("VmHWM:")
}

#[cfg(not(target_os = "linux"))]
pub fn rss_bytes() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn peak_rss_bytes() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn proc_status_bytes(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib * 1024);
        }
    }
    None
}

/// CPU model name, for the report header.
#[cfg(target_os = "linux")]
pub fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    info.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, value)| value.trim().to_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn cpu_model() -> Option<String> {
    None
}

/// User + system CPU time consumed by the whole process.
#[cfg(unix)]
pub fn cpu_time() -> Option<Duration> {
    // SAFETY: `getrusage` fills every field of the zeroed struct on success.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `RUSAGE_SELF` and a valid out-pointer.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let user = usage.ru_utime.tv_sec as u64 * 1_000_000 + usage.ru_utime.tv_usec as u64;
    let system = usage.ru_stime.tv_sec as u64 * 1_000_000 + usage.ru_stime.tv_usec as u64;
    Some(Duration::from_micros(user + system))
}

#[cfg(not(unix))]
pub fn cpu_time() -> Option<Duration> {
    None
}

/// Wall and CPU time for one measured region.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub wall: Duration,
    pub cpu: Option<Duration>,
}

impl Timing {
    /// CPU utilization in percent. `None` when the platform has no CPU clock.
    pub fn cpu_percent(&self) -> Option<f64> {
        let wall = self.wall.as_secs_f64();
        let cpu = self.cpu?.as_secs_f64();
        (wall > 0.0).then_some(cpu / wall * 100.0)
    }
}

/// Runs `f`, returning its value plus wall and process CPU time.
pub fn time<T>(f: impl FnOnce() -> T) -> (T, Timing) {
    let cpu_start = cpu_time();
    let start = Instant::now();
    let out = f();
    let wall = start.elapsed();
    let cpu = match (cpu_start, cpu_time()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a)),
        _ => None,
    };
    (out, Timing { wall, cpu })
}
