//! Lightweight process measurements for the stage UI probe.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub rss_bytes: u64,
    pub cpu_user: Duration,
    pub cpu_sys: Duration,
}

impl Snapshot {
    #[must_use]
    pub fn now() -> Self {
        Self {
            rss_bytes: read_rss(),
            cpu_user: read_cpu().0,
            cpu_sys: read_cpu().1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PhaseReport {
    pub name: String,
    pub wall: Duration,
    pub frames: u32,
    pub avg_frame: Duration,
    pub max_frame: Duration,
    pub cpu_user: Duration,
    pub cpu_sys: Duration,
    pub rss_start: u64,
    pub rss_end: u64,
}

pub struct Metrics {
    pub started: Instant,
    pub frames: Vec<Duration>,
    pub last_frame: Instant,
    phase_name: String,
    phase_start: Instant,
    phase_snapshot: Snapshot,
    phase_frames: u32,
    reports: Vec<PhaseReport>,
}

impl Metrics {
    #[must_use]
    pub fn start(phase: &str) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            frames: Vec::new(),
            last_frame: now,
            phase_name: phase.to_owned(),
            phase_start: now,
            phase_snapshot: Snapshot::now(),
            phase_frames: 0,
            reports: Vec::new(),
        }
    }

    pub fn on_frame(&mut self) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        self.frames.push(dt);
        self.phase_frames = self.phase_frames.saturating_add(1);
        if self.frames.len() > 10_000 {
            self.frames.remove(0);
        }
    }

    pub fn rotate_phase(&mut self, next: &str) {
        self.reports.push(self.finish_phase());
        self.phase_name.clear();
        self.phase_name.push_str(next);
        self.phase_start = Instant::now();
        self.phase_snapshot = Snapshot::now();
        self.phase_frames = 0;
        self.frames.clear();
    }

    fn finish_phase(&self) -> PhaseReport {
        let end = Snapshot::now();
        let avg = if self.frames.is_empty() {
            Duration::ZERO
        } else {
            self.frames.iter().sum::<Duration>() / u32::try_from(self.frames.len()).unwrap_or(1)
        };
        let max = self.frames.iter().copied().max().unwrap_or(Duration::ZERO);
        PhaseReport {
            name: self.phase_name.clone(),
            wall: self.phase_start.elapsed(),
            frames: self.phase_frames,
            avg_frame: avg,
            max_frame: max,
            cpu_user: end.cpu_user.saturating_sub(self.phase_snapshot.cpu_user),
            cpu_sys: end.cpu_sys.saturating_sub(self.phase_snapshot.cpu_sys),
            rss_start: self.phase_snapshot.rss_bytes,
            rss_end: end.rss_bytes,
        }
    }

    #[must_use]
    pub fn reports(&mut self) -> Vec<PhaseReport> {
        let mut out = self.reports.clone();
        out.push(self.finish_phase());
        out
    }
}

pub fn print_reports(
    kind: &str,
    adapter: &str,
    backend: &str,
    extra: &str,
    reports: &[PhaseReport],
) {
    println!("=== {kind} ===");
    println!("adapter: {adapter}");
    println!("backend: {backend}");
    println!("{extra}");
    for report in reports {
        println!(
            "phase={name} wall_ms={wall:.1} frames={frames} avg_ms={avg:.2} max_ms={max:.2} cpu_user_ms={user:.1} cpu_sys_ms={sys:.1} rss_start_kib={rss0} rss_end_kib={rss1}",
            name = report.name,
            wall = report.wall.as_secs_f64() * 1000.0,
            frames = report.frames,
            avg = report.avg_frame.as_secs_f64() * 1000.0,
            max = report.max_frame.as_secs_f64() * 1000.0,
            user = report.cpu_user.as_secs_f64() * 1000.0,
            sys = report.cpu_sys.as_secs_f64() * 1000.0,
            rss0 = report.rss_start / 1024,
            rss1 = report.rss_end / 1024,
        );
    }
}

fn read_rss() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib = rest
                .split_whitespace()
                .next()
                .and_then(|tok| tok.parse::<u64>().ok());
            if let Some(kib) = kib {
                return kib.saturating_mul(1024);
            }
        }
    }
    0
}

fn read_cpu() -> (Duration, Duration) {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return (Duration::ZERO, Duration::ZERO);
    };
    let ticks = ticks_per_second();
    let parts: Vec<&str> = stat.split_whitespace().collect();
    // /proc/self/stat: 14=utime 15=stime, after comm which may contain spaces.
    let Some(comm_end) = stat.rfind(')') else {
        return (Duration::ZERO, Duration::ZERO);
    };
    let after: Vec<&str> = stat[comm_end + 1..].split_whitespace().collect();
    let utime = after
        .get(11)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let stime = after
        .get(12)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let _ = parts;
    (
        ticks_to_duration(utime, ticks),
        ticks_to_duration(stime, ticks),
    )
}

fn ticks_per_second() -> u64 {
    100
}

fn ticks_to_duration(ticks: u64, hz: u64) -> Duration {
    if hz == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64({
        #[expect(
            clippy::cast_precision_loss,
            reason = "tick counts fit in f64 for process CPU samples"
        )]
        {
            ticks as f64 / hz as f64
        }
    })
}
