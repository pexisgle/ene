use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CPU_LIMIT_PERCENT: f64 = 10.0;
const RSS_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUSY_WAIT_ONE_CORE: f64 = 0.5;
const FPS_MINIMUM: f64 = 30.0;
const INTAKE_LIMIT_SECS: f64 = 1.0;
const IDLE_GATE_SECS: f64 = 300.0;

/// The measured GUI operation whose intake and paint the campaign gate
/// requires. Shared with the producer so a renamed label cannot silently turn
/// every campaign Incomplete.
pub const CANCEL_TASK_OPERATION: &str = "cancel_task";

/// Role of a process included in the idle campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    Host,
    Desktop,
    Body,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTarget {
    pub role: ProcessRole,
    pub name: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProcessPoint {
    pub wall_offset_secs: f64,
    pub cpu_user_secs: f64,
    pub cpu_system_secs: f64,
    pub rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub role: ProcessRole,
    pub name: String,
    pub pid: u32,
    pub points: Vec<ProcessPoint>,
    pub cpu_delta_secs: f64,
    pub one_core_equivalent: f64,
    pub rss_mean_bytes: u64,
    pub rss_peak_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CpuRecord {
    pub cpu_seconds_sum: f64,
    pub machine_percent: f64,
    pub one_core_equivalent_sum: f64,
    pub busy_wait_pids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RssRecord {
    pub mean_bytes: u64,
    pub peak_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationSource {
    WaylandWpPresentation {
        body_pid: u32,
        surface_id: String,
    },
    WindowsDisplayTiming {
        body_pid: u32,
        swap_chain: String,
        raw_evidence: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PresentationEvent {
    Presented {
        correlation_id: u64,
        timestamp_ns: u64,
        clock_id: i32,
        output: String,
    },
    Discarded {
        correlation_id: u64,
    },
    Missing {
        correlation_id: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationRecord {
    pub source: PresentationSource,
    pub warmup_secs: f64,
    pub wall_secs: f64,
    pub events: Vec<PresentationEvent>,
    pub presented: u64,
    pub discarded: u64,
    pub missing: u64,
    pub actual_fps: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaylandFeedbackTraceLine {
    pub observed_unix_ns: u64,
    pub feedback: ene_body::ipc::PresentationFeedback,
}

impl PresentationRecord {
    pub fn from_events(
        source: PresentationSource,
        warmup_secs: f64,
        wall_secs: f64,
        events: Vec<PresentationEvent>,
    ) -> Result<Self, MeasurementError> {
        if !wall_secs.is_finite()
            || wall_secs <= 0.0
            || !warmup_secs.is_finite()
            || warmup_secs < 0.0
        {
            return Err(MeasurementError::InvalidWindow);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut presented = 0_u64;
        let mut discarded = 0_u64;
        let mut missing = 0_u64;
        for event in &events {
            let correlation_id = match event {
                PresentationEvent::Presented { correlation_id, .. }
                | PresentationEvent::Discarded { correlation_id }
                | PresentationEvent::Missing { correlation_id, .. } => *correlation_id,
            };
            if !seen.insert(correlation_id) {
                return Err(MeasurementError::DuplicatePresentation(correlation_id));
            }
            match event {
                PresentationEvent::Presented { .. } => {
                    presented = presented
                        .checked_add(1)
                        .ok_or(MeasurementError::NumericOverflow)?;
                }
                PresentationEvent::Discarded { .. } => {
                    discarded = discarded
                        .checked_add(1)
                        .ok_or(MeasurementError::NumericOverflow)?;
                }
                PresentationEvent::Missing { .. } => {
                    missing = missing
                        .checked_add(1)
                        .ok_or(MeasurementError::NumericOverflow)?;
                }
            }
        }
        Ok(Self {
            source,
            warmup_secs,
            wall_secs,
            events,
            presented,
            discarded,
            missing,
            actual_fps: presented as f64 / wall_secs,
        })
    }

    fn passes(&self) -> bool {
        self.rejection().is_none()
    }

    /// The first presentation rejection, so the operator failure line names the
    /// real cause instead of printing numbers that all look passing. `None`
    /// when the presented events are accepted.
    fn rejection(&self) -> Option<&'static str> {
        let mut ids = std::collections::BTreeSet::new();
        let mut presented = 0_u64;
        let mut discarded = 0_u64;
        let mut missing = 0_u64;
        let mut last_timestamp = None;
        let mut timing_domain = None;
        for event in &self.events {
            let id = match event {
                PresentationEvent::Presented { correlation_id, .. }
                | PresentationEvent::Discarded { correlation_id }
                | PresentationEvent::Missing { correlation_id, .. } => *correlation_id,
            };
            if !ids.insert(id) {
                return Some("presentation correlation ids are not unique");
            }
            match event {
                PresentationEvent::Presented {
                    timestamp_ns,
                    clock_id,
                    output,
                    ..
                } => {
                    if output.is_empty() {
                        return Some("presented output is empty");
                    }
                    if last_timestamp.is_some_and(|last| *timestamp_ns <= last) {
                        return Some("presented timestamps are not strictly increasing");
                    }
                    if timing_domain
                        .as_ref()
                        .is_some_and(|domain| domain != &(*clock_id, output.as_str()))
                    {
                        return Some("presented timing domain changed");
                    }
                    last_timestamp = Some(*timestamp_ns);
                    timing_domain = Some((*clock_id, output.as_str()));
                    presented = presented.saturating_add(1);
                }
                PresentationEvent::Discarded { .. } => discarded = discarded.saturating_add(1),
                PresentationEvent::Missing { .. } => missing = missing.saturating_add(1),
            }
        }
        if !self.wall_secs.is_finite() || self.wall_secs <= 0.0 {
            return Some("presentation window is not positive");
        }
        let calculated_fps = presented as f64 / self.wall_secs;
        if presented == 0 || calculated_fps < FPS_MINIMUM {
            return Some("presented FPS is below the minimum");
        }
        if discarded != 0 {
            return Some("frames were discarded");
        }
        if missing != 0 {
            return Some("frames are missing");
        }
        None
    }
}

pub fn wayland_presentation_record(
    body_pid: u32,
    warmup_secs: f64,
    wall_secs: f64,
    feedback: Vec<ene_body::ipc::PresentationFeedback>,
) -> Result<PresentationRecord, MeasurementError> {
    let Some(surface_id) = feedback
        .first()
        .map(|feedback| feedback.surface_id.as_str())
    else {
        return Err(MeasurementError::PresentationTrace(String::from(
            "Wayland feedback is empty",
        )));
    };
    let surfaces = feedback
        .iter()
        .map(|feedback| feedback.surface_id.as_str())
        .collect::<BTreeSet<_>>();
    if surfaces.len() != 1 {
        return Err(MeasurementError::PresentationTrace(String::from(
            "Wayland feedback must correlate to exactly one surface",
        )));
    }
    let surface_id = surface_id.to_string();
    let mut correlated = BTreeMap::<u64, Option<PresentationEvent>>::new();
    for feedback in feedback {
        let terminal = match feedback.outcome {
            ene_body::ipc::PresentationOutcome::Submitted => {
                if correlated.insert(feedback.correlation_id, None).is_some() {
                    return Err(MeasurementError::DuplicatePresentation(
                        feedback.correlation_id,
                    ));
                }
                continue;
            }
            ene_body::ipc::PresentationOutcome::Presented {
                timestamp_ns,
                clock_id,
                output,
            } => PresentationEvent::Presented {
                correlation_id: feedback.correlation_id,
                timestamp_ns,
                clock_id,
                output,
            },
            ene_body::ipc::PresentationOutcome::Discarded => PresentationEvent::Discarded {
                correlation_id: feedback.correlation_id,
            },
            ene_body::ipc::PresentationOutcome::Missing { reason } => PresentationEvent::Missing {
                correlation_id: feedback.correlation_id,
                reason,
            },
        };
        let slot = correlated
            .get_mut(&feedback.correlation_id)
            .ok_or_else(|| {
                MeasurementError::PresentationTrace(format!(
                    "terminal feedback {} has no correlated submission",
                    feedback.correlation_id
                ))
            })?;
        if slot.replace(terminal).is_some() {
            return Err(MeasurementError::DuplicatePresentation(
                feedback.correlation_id,
            ));
        }
    }
    let events = correlated
        .into_iter()
        .map(|(correlation_id, terminal)| {
            terminal.unwrap_or_else(|| PresentationEvent::Missing {
                correlation_id,
                reason: String::from("wp_presentation feedback did not resolve"),
            })
        })
        .collect();
    PresentationRecord::from_events(
        PresentationSource::WaylandWpPresentation {
            body_pid,
            surface_id,
        },
        warmup_secs,
        wall_secs,
        events,
    )
}

/// Imports a PresentMon CSV while retaining the raw trace path and requiring
/// every row to correlate to the selected Body PID and swap chain.
/// `DisplayedTime` must show a positive display duration; the display timestamp
/// is reconstructed from `TimeInSeconds + MsUntilDisplayed` (or the equivalent
/// `CPUStartTime + DisplayLatency`). An explicit dropped row is discarded,
/// while a row without complete display timing is missing.
///
/// # Errors
///
/// Missing required columns, malformed values, or I/O.
pub fn import_presentmon_csv(
    path: &Path,
    body_pid: u32,
    swap_chain: &str,
    warmup_secs: f64,
    wall_secs: f64,
) -> Result<PresentationRecord, MeasurementError> {
    let mut reader = csv::Reader::from_path(path).map_err(|error| {
        MeasurementError::PresentationTrace(format!("{}: {error}", path.display()))
    })?;
    let headers = reader
        .headers()
        .map_err(|error| MeasurementError::PresentationTrace(error.to_string()))?
        .clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .ok_or_else(|| {
                MeasurementError::PresentationTrace(format!("missing PresentMon column {name}"))
            })
    };
    let pid_column = column("ProcessID")?;
    let chain_column = column("SwapChainAddress")?;
    let displayed_column = column("DisplayedTime")?;
    let timing_columns = match (
        headers.iter().position(|header| header == "TimeInSeconds"),
        headers
            .iter()
            .position(|header| header == "MsUntilDisplayed"),
        headers.iter().position(|header| header == "CPUStartTime"),
        headers.iter().position(|header| header == "DisplayLatency"),
    ) {
        (Some(start), Some(latency), _, _) => (start, latency, false),
        (_, _, Some(start), Some(latency)) => (start, latency, true),
        _ => {
            return Err(MeasurementError::PresentationTrace(String::from(
                "missing a complete PresentMon display timing column pair",
            )));
        }
    };
    let dropped_column = headers.iter().position(|header| header == "Dropped");
    let mut events = Vec::new();
    for row in reader.records() {
        let row = row.map_err(|error| MeasurementError::PresentationTrace(error.to_string()))?;
        let pid = row
            .get(pid_column)
            .and_then(|value| value.parse::<u32>().ok());
        if pid != Some(body_pid) || row.get(chain_column) != Some(swap_chain) {
            continue;
        }
        let start_value = row
            .get(timing_columns.0)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let Some(start_value) = start_value else {
            continue;
        };
        let start_secs = if timing_columns.2 {
            start_value / 1_000.0
        } else {
            start_value
        };
        if start_secs < warmup_secs || start_secs >= warmup_secs + wall_secs {
            continue;
        }
        let correlation_id = u64::try_from(events.len())
            .map_err(|_| MeasurementError::NumericOverflow)?
            .saturating_add(1);
        let dropped = dropped_column
            .and_then(|index| row.get(index))
            .is_some_and(|value| {
                let value = value.trim();
                value.eq_ignore_ascii_case("true")
                    || value == "1"
                    || value.eq_ignore_ascii_case("dropped")
            });
        let displayed_text = row.get(displayed_column).unwrap_or_default().trim();
        if dropped || displayed_text.eq_ignore_ascii_case("NA") {
            events.push(PresentationEvent::Discarded { correlation_id });
            continue;
        }
        let displayed_duration = displayed_text
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0);
        let display_latency_ms = row
            .get(timing_columns.1)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        if let (Some(_duration), Some(display_latency_ms)) =
            (displayed_duration, display_latency_ms)
        {
            let timestamp_ns =
                ((start_secs + display_latency_ms / 1_000.0) * 1_000_000_000.0).round();
            let timestamp_ns = if timestamp_ns >= 0.0 && timestamp_ns <= u64::MAX as f64 {
                timestamp_ns as u64
            } else {
                return Err(MeasurementError::PresentationTrace(String::from(
                    "PresentMon display timestamp is outside u64 nanoseconds",
                )));
            };
            events.push(PresentationEvent::Presented {
                correlation_id,
                timestamp_ns,
                clock_id: 0,
                output: String::from("PresentMon display timing"),
            });
        } else {
            events.push(PresentationEvent::Missing {
                correlation_id,
                reason: String::from("PresentMon row has no correlated display timing"),
            });
        }
    }
    PresentationRecord::from_events(
        PresentationSource::WindowsDisplayTiming {
            body_pid,
            swap_chain: swap_chain.to_string(),
            raw_evidence: path.display().to_string(),
        },
        warmup_secs,
        wall_secs,
        events,
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionSample {
    pub operation: String,
    pub input_monotonic_ns: u64,
    pub host_intake_monotonic_ns: u64,
    pub gui_painted_monotonic_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionTraceLine {
    pub observed_unix_ns: u64,
    pub sample: InteractionSample,
}

#[must_use]
pub fn monotonic_ns() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    u64::try_from(ORIGIN.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}

impl InteractionSample {
    #[must_use]
    pub fn intake_latency_secs(&self) -> Option<f64> {
        self.host_intake_monotonic_ns
            .checked_sub(self.input_monotonic_ns)
            .map(|ns| ns as f64 / 1_000_000_000.0)
    }

    #[must_use]
    pub fn painted_latency_secs(&self) -> Option<f64> {
        self.gui_painted_monotonic_ns
            .checked_sub(self.input_monotonic_ns)
            .map(|ns| ns as f64 / 1_000_000_000.0)
    }

    fn passes(&self) -> bool {
        !self.operation.trim().is_empty()
            && self.gui_painted_monotonic_ns >= self.host_intake_monotonic_ns
            && self
                .intake_latency_secs()
                .is_some_and(|latency| latency <= INTAKE_LIMIT_SECS)
            && self
                .painted_latency_secs()
                .is_some_and(|latency| latency <= INTAKE_LIMIT_SECS)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickThroughRecord {
    pub compositor: String,
    pub transparent_click_reached_underlying_window: bool,
    pub maximum_input_block_secs: f64,
    pub raw_evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementEnvironment {
    pub desktop_session: String,
    pub cpu_model: String,
    pub gpu_model: String,
    pub gpu_driver: String,
    pub display: String,
    pub scale: String,
    pub ui_language: String,
    pub asset: String,
    pub render_backend: String,
    pub build_flags: String,
}

impl MeasurementEnvironment {
    fn complete(&self) -> bool {
        [
            &self.desktop_session,
            &self.cpu_model,
            &self.gpu_model,
            &self.gpu_driver,
            &self.display,
            &self.scale,
            &self.ui_language,
            &self.asset,
            &self.render_backend,
            &self.build_flags,
        ]
        .into_iter()
        .all(|value| !value.trim().is_empty())
    }
}

impl ClickThroughRecord {
    fn passes(&self) -> bool {
        !self.compositor.trim().is_empty()
            && self.transparent_click_reached_underlying_window
            && self.maximum_input_block_secs.is_finite()
            && self.maximum_input_block_secs >= 0.0
            && self.maximum_input_block_secs < INTAKE_LIMIT_SECS
            && !self.raw_evidence.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MeasurementVerdict(VerdictKind);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum VerdictKind {
    Unmeasured,
    Incomplete,
    Fail,
    Pass,
}

impl MeasurementVerdict {
    pub const UNMEASURED: Self = Self(VerdictKind::Unmeasured);

    #[must_use]
    pub fn is_pass(self) -> bool {
        self.0 == VerdictKind::Pass
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self.0 {
            VerdictKind::Unmeasured => "Unmeasured",
            VerdictKind::Incomplete => "Incomplete",
            VerdictKind::Fail => "Fail",
            VerdictKind::Pass => "Pass",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasurementRecord {
    pub exact_sha: String,
    pub build_profile: String,
    pub os: String,
    pub kernel_or_build: String,
    pub environment: Option<MeasurementEnvironment>,
    pub logical_cpus: u32,
    pub started_unix_ms: u128,
    pub elapsed_wall_secs: f64,
    pub processes: Vec<ProcessRecord>,
    pub cpu: Option<CpuRecord>,
    pub rss: Option<RssRecord>,
    pub fps: Option<PresentationRecord>,
    pub interactions: Vec<InteractionSample>,
    pub click_through: Option<ClickThroughRecord>,
    pub verdict: MeasurementVerdict,
    pub failures: Vec<String>,
}

impl Default for MeasurementRecord {
    fn default() -> Self {
        Self {
            exact_sha: String::new(),
            build_profile: String::new(),
            os: std::env::consts::OS.to_string(),
            kernel_or_build: String::new(),
            environment: None,
            logical_cpus: 0,
            started_unix_ms: 0,
            elapsed_wall_secs: 0.0,
            processes: Vec::new(),
            cpu: None,
            rss: None,
            fps: None,
            interactions: Vec::new(),
            click_through: None,
            verdict: MeasurementVerdict::UNMEASURED,
            failures: Vec::new(),
        }
    }
}

impl MeasurementRecord {
    pub fn evaluate(&mut self) {
        self.failures.clear();
        let required_roles = [ProcessRole::Host, ProcessRole::Desktop, ProcessRole::Body];
        for role in required_roles {
            if !self.processes.iter().any(|process| process.role == role) {
                self.failures.push(format!("missing {role:?} process"));
            }
        }
        if self.exact_sha.len() != 40
            || !self.exact_sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.build_profile != "release"
        {
            self.failures
                .push(String::from("exact SHA and release profile are required"));
        }
        if self.kernel_or_build.trim().is_empty()
            || self
                .environment
                .as_ref()
                .is_none_or(|environment| !environment.complete())
        {
            self.failures.push(String::from(
                "reproducible environment facts are incomplete",
            ));
        }
        if !self.elapsed_wall_secs.is_finite() {
            self.failures.push(format!(
                "idle wall {}s is not a finite duration",
                self.elapsed_wall_secs
            ));
        } else if self.elapsed_wall_secs < IDLE_GATE_SECS {
            self.failures.push(format!(
                "idle wall {:.3}s is shorter than {IDLE_GATE_SECS:.0}s",
                self.elapsed_wall_secs
            ));
        }
        if self.logical_cpus == 0 {
            self.failures
                .push(String::from("online logical CPU count is zero"));
        }
        let mut incomplete = false;
        if let Some(cpu) = &self.cpu {
            if !cpu.cpu_seconds_sum.is_finite()
                || cpu.cpu_seconds_sum < 0.0
                || !cpu.machine_percent.is_finite()
                || cpu.machine_percent < 0.0
                || !cpu.one_core_equivalent_sum.is_finite()
                || cpu.one_core_equivalent_sum < 0.0
            {
                self.failures
                    .push(String::from("CPU aggregate contains invalid values"));
            } else if cpu.machine_percent > CPU_LIMIT_PERCENT {
                self.failures.push(format!(
                    "machine CPU {:.3}% exceeds {CPU_LIMIT_PERCENT}%",
                    cpu.machine_percent
                ));
            }
            if !cpu.busy_wait_pids.is_empty() {
                self.failures
                    .push(format!("busy-wait PIDs: {:?}", cpu.busy_wait_pids));
            }
        } else {
            self.failures.push(String::from("CPU is unmeasured"));
            incomplete = true;
        }
        if let Some(rss) = &self.rss {
            if rss.peak_bytes > RSS_LIMIT_BYTES {
                self.failures.push(format!(
                    "RSS peak {} exceeds {RSS_LIMIT_BYTES}",
                    rss.peak_bytes
                ));
            }
        } else {
            self.failures.push(String::from("RSS is unmeasured"));
            incomplete = true;
        }
        if let Some(fps) = &self.fps {
            let expected_body = self
                .processes
                .iter()
                .find(|process| process.role == ProcessRole::Body)
                .map(|process| process.pid);
            let measured_body = match &fps.source {
                PresentationSource::WaylandWpPresentation { body_pid, .. }
                | PresentationSource::WindowsDisplayTiming { body_pid, .. } => Some(*body_pid),
            };
            if measured_body != expected_body {
                self.failures.push(String::from(
                    "presentation evidence does not correlate to the measured Body PID",
                ));
            }
            if !fps.passes() {
                let reason = fps
                    .rejection()
                    .map_or(String::new(), |reason| format!(": {reason}"));
                self.failures.push(format!(
                    "presented FPS {:.3}, discarded {}, missing {}{reason}",
                    fps.actual_fps, fps.discarded, fps.missing
                ));
            }
        } else {
            self.failures
                .push(String::from("presented FPS is unmeasured"));
            incomplete = true;
        }
        if let Some(click_through) = &self.click_through {
            if !click_through.passes() {
                self.failures.push(String::from(
                    "compositor click-through/input-hitch probe failed",
                ));
            }
        } else {
            self.failures
                .push(String::from("click-through is unmeasured"));
            incomplete = true;
        }
        if self.interactions.is_empty() {
            self.failures
                .push(String::from("operation intake is unmeasured"));
            incomplete = true;
        } else if !self
            .interactions
            .iter()
            .any(|sample| sample.operation == CANCEL_TASK_OPERATION)
        {
            self.failures
                .push(String::from("cancel_task intake/paint is unmeasured"));
            incomplete = true;
        }
        for interaction in &self.interactions {
            if !interaction.passes() {
                self.failures.push(format!(
                    "operation {} was not painted within 1s",
                    interaction.operation
                ));
            }
        }
        self.verdict = if incomplete {
            MeasurementVerdict(VerdictKind::Incomplete)
        } else if self.failures.is_empty() {
            MeasurementVerdict(VerdictKind::Pass)
        } else {
            MeasurementVerdict(VerdictKind::Fail)
        };
    }

    #[must_use]
    pub fn claims_pass(&self) -> bool {
        self.verdict.is_pass()
    }

    #[must_use]
    pub fn human_report(&self) -> String {
        let mut out = format!(
            "Stage 7 performance measurement\nSHA: {}\nOS: {} {}\nProfile: {}\nWall: {:.3}s; logical CPUs: {}\nVerdict: {}\n",
            self.exact_sha,
            self.os,
            self.kernel_or_build,
            self.build_profile,
            self.elapsed_wall_secs,
            self.logical_cpus,
            self.verdict.label()
        );
        if let Some(cpu) = &self.cpu {
            out.push_str(&format!(
                "CPU: {:.3}% machine; {:.3} core-equivalent; busy-wait {:?}\n",
                cpu.machine_percent, cpu.one_core_equivalent_sum, cpu.busy_wait_pids
            ));
        } else {
            out.push_str("CPU: Unmeasured\n");
        }
        if let Some(rss) = &self.rss {
            out.push_str(&format!(
                "RSS: mean {} bytes; peak {} bytes\n",
                rss.mean_bytes, rss.peak_bytes
            ));
        } else {
            out.push_str("RSS: Unmeasured\n");
        }
        if let Some(fps) = &self.fps {
            out.push_str(&format!(
                "FPS: {:.3}; presented {}; discarded {}; missing {}; wall {:.3}s; source {:?}\n",
                fps.actual_fps,
                fps.presented,
                fps.discarded,
                fps.missing,
                fps.wall_secs,
                fps.source
            ));
        } else {
            out.push_str("FPS: Unmeasured\n");
        }
        if let Some(environment) = &self.environment {
            out.push_str(&format!(
                "Environment: {}; CPU {}; GPU {} / {}; display {} @ {}; language {}; asset {}; backend {}; build {}\n",
                environment.desktop_session,
                environment.cpu_model,
                environment.gpu_model,
                environment.gpu_driver,
                environment.display,
                environment.scale,
                environment.ui_language,
                environment.asset,
                environment.render_backend,
                environment.build_flags
            ));
        } else {
            out.push_str("Environment: Unmeasured\n");
        }
        for process in &self.processes {
            out.push_str(&format!(
                "PID {} {} ({:?}): CPU {:.6}s / {:.4} core; RSS mean {} peak {}\n",
                process.pid,
                process.name,
                process.role,
                process.cpu_delta_secs,
                process.one_core_equivalent,
                process.rss_mean_bytes,
                process.rss_peak_bytes
            ));
        }
        for interaction in &self.interactions {
            out.push_str(&format!(
                "Interaction {}: intake {:.6}s; painted {:.6}s\n",
                interaction.operation,
                interaction.intake_latency_secs().unwrap_or(f64::NAN),
                interaction.painted_latency_secs().unwrap_or(f64::NAN)
            ));
        }
        if let Some(click) = &self.click_through {
            out.push_str(&format!(
                "Click-through: compositor {}; reached {}; max block {:.6}s; evidence {}\n",
                click.compositor,
                click.transparent_click_reached_underlying_window,
                click.maximum_input_block_secs,
                click.raw_evidence
            ));
        } else {
            out.push_str("Click-through: Unmeasured\n");
        }
        for failure in &self.failures {
            out.push_str(&format!("FAIL: {failure}\n"));
        }
        out
    }

    pub fn write_outputs(&self, json: &Path, report: &Path) -> Result<(), MeasurementError> {
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| MeasurementError::Json(error.to_string()))?;
        std::fs::write(json, encoded).map_err(MeasurementError::Io)?;
        std::fs::write(report, self.human_report()).map_err(MeasurementError::Io)
    }
}

pub fn sample_idle(
    targets: &[ProcessTarget],
    duration: Duration,
    interval: Duration,
    exact_sha: String,
    kernel_or_build: String,
) -> Result<MeasurementRecord, MeasurementError> {
    let unique_pids = targets
        .iter()
        .map(|target| target.pid)
        .collect::<BTreeSet<_>>();
    if targets.is_empty()
        || duration.is_zero()
        || interval.is_zero()
        || targets.iter().any(|target| target.pid == 0)
        || unique_pids.len() != targets.len()
    {
        return Err(MeasurementError::InvalidCampaign);
    }
    let logical_cpus = u32::try_from(
        std::thread::available_parallelism()
            .map_err(MeasurementError::Io)?
            .get(),
    )
    .map_err(|_| MeasurementError::NumericOverflow)?;
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| MeasurementError::Clock)?
        .as_millis();
    let start = Instant::now();
    let mut points: BTreeMap<u32, Vec<ProcessPoint>> = targets
        .iter()
        .map(|target| (target.pid, Vec::new()))
        .collect();
    loop {
        let offset = start.elapsed().as_secs_f64();
        for target in targets {
            let point = os::read_process(target.pid, offset)?;
            let process_points = points
                .get_mut(&target.pid)
                .ok_or(MeasurementError::InvalidCampaign)?;
            process_points.push(point);
        }
        if start.elapsed() >= duration {
            break;
        }
        let remaining = duration.saturating_sub(start.elapsed());
        std::thread::sleep(interval.min(remaining));
    }
    let elapsed_wall_secs = start.elapsed().as_secs_f64();
    let mut processes = Vec::with_capacity(targets.len());
    for target in targets {
        let process_points = points
            .remove(&target.pid)
            .ok_or(MeasurementError::InvalidCampaign)?;
        let first = process_points
            .first()
            .ok_or(MeasurementError::InvalidCampaign)?;
        let last = process_points
            .last()
            .ok_or(MeasurementError::InvalidCampaign)?;
        let cpu_delta_secs = (last.cpu_user_secs + last.cpu_system_secs
            - first.cpu_user_secs
            - first.cpu_system_secs)
            .max(0.0);
        let rss_sum = process_points.iter().try_fold(0_u128, |sum, point| {
            sum.checked_add(u128::from(point.rss_bytes))
                .ok_or(MeasurementError::NumericOverflow)
        })?;
        let rss_mean_bytes = u64::try_from(rss_sum / process_points.len() as u128)
            .map_err(|_| MeasurementError::NumericOverflow)?;
        let rss_peak_bytes = process_points
            .iter()
            .map(|point| point.rss_bytes)
            .max()
            .unwrap_or(0);
        processes.push(ProcessRecord {
            role: target.role,
            name: target.name.clone(),
            pid: target.pid,
            points: process_points,
            cpu_delta_secs,
            one_core_equivalent: cpu_delta_secs / elapsed_wall_secs,
            rss_mean_bytes,
            rss_peak_bytes,
        });
    }
    let cpu_seconds_sum = processes.iter().map(|process| process.cpu_delta_secs).sum();
    let machine_percent = 100.0 * cpu_seconds_sum / (elapsed_wall_secs * f64::from(logical_cpus));
    let one_core_equivalent_sum = cpu_seconds_sum / elapsed_wall_secs;
    let busy_wait_pids = processes
        .iter()
        .filter(|process| process.one_core_equivalent >= BUSY_WAIT_ONE_CORE)
        .map(|process| process.pid)
        .collect();
    let rounds = processes
        .first()
        .map(|process| process.points.len())
        .unwrap_or(0);
    let mut rss_totals = vec![0_u64; rounds];
    for process in &processes {
        if process.points.len() != rounds {
            return Err(MeasurementError::UnalignedSamples);
        }
        for (total, point) in rss_totals.iter_mut().zip(&process.points) {
            *total = total
                .checked_add(point.rss_bytes)
                .ok_or(MeasurementError::NumericOverflow)?;
        }
    }
    let rss_total_sum = rss_totals.iter().try_fold(0_u128, |sum, rss| {
        sum.checked_add(u128::from(*rss))
            .ok_or(MeasurementError::NumericOverflow)
    })?;
    let rss_mean = u64::try_from(rss_total_sum / rounds.max(1) as u128)
        .map_err(|_| MeasurementError::NumericOverflow)?;
    let rss_peak = rss_totals.into_iter().max().unwrap_or(0);
    Ok(MeasurementRecord {
        exact_sha,
        build_profile: if cfg!(debug_assertions) {
            String::from("debug")
        } else {
            String::from("release")
        },
        os: std::env::consts::OS.to_string(),
        kernel_or_build,
        logical_cpus,
        started_unix_ms,
        elapsed_wall_secs,
        processes,
        cpu: Some(CpuRecord {
            cpu_seconds_sum,
            machine_percent,
            one_core_equivalent_sum,
            busy_wait_pids,
        }),
        rss: Some(RssRecord {
            mean_bytes: rss_mean,
            peak_bytes: rss_peak,
        }),
        ..MeasurementRecord::default()
    })
}

#[derive(Debug, thiserror::Error)]
pub enum MeasurementError {
    #[error("invalid measurement campaign")]
    InvalidCampaign,
    #[error("invalid presentation window")]
    InvalidWindow,
    #[error("duplicate presentation correlation id {0}")]
    DuplicatePresentation(u64),
    #[error("process samples were not aligned")]
    UnalignedSamples,
    #[error("numeric overflow")]
    NumericOverflow,
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("process {pid} is unavailable: {reason}")]
    ProcessUnavailable { pid: u32, reason: String },
    #[error("I/O: {0}")]
    Io(#[source] std::io::Error),
    #[error("JSON: {0}")]
    Json(String),
    #[error("presentation trace: {0}")]
    PresentationTrace(String),
    #[error("this operating system is unsupported by the sampler")]
    UnsupportedOs,
}

#[cfg(target_os = "linux")]
mod os {
    use super::{MeasurementError, ProcessPoint};

    pub(super) fn read_process(
        pid: u32,
        wall_offset_secs: f64,
    ) -> Result<ProcessPoint, MeasurementError> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|error| {
            MeasurementError::ProcessUnavailable {
                pid,
                reason: error.to_string(),
            }
        })?;
        let close = stat
            .rfind(')')
            .ok_or_else(|| MeasurementError::ProcessUnavailable {
                pid,
                reason: String::from("malformed /proc stat"),
            })?;
        let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        let ticks = |index: usize| -> Result<u64, MeasurementError> {
            fields
                .get(index)
                .ok_or_else(|| MeasurementError::ProcessUnavailable {
                    pid,
                    reason: String::from("short /proc stat"),
                })?
                .parse()
                .map_err(|_| MeasurementError::ProcessUnavailable {
                    pid,
                    reason: String::from("invalid /proc CPU counter"),
                })
        };
        let user_ticks = ticks(11)?;
        let system_ticks = ticks(12)?;
        // SAFETY: sysconf is side-effect free for this constant and has no
        // pointer arguments. A non-positive result is rejected below.
        let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if clock_ticks <= 0 {
            return Err(MeasurementError::ProcessUnavailable {
                pid,
                reason: String::from("_SC_CLK_TCK unavailable"),
            });
        }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|error| {
            MeasurementError::ProcessUnavailable {
                pid,
                reason: error.to_string(),
            }
        })?;
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| MeasurementError::ProcessUnavailable {
                pid,
                reason: String::from("VmRSS unavailable"),
            })?;
        Ok(ProcessPoint {
            wall_offset_secs,
            cpu_user_secs: user_ticks as f64 / clock_ticks as f64,
            cpu_system_secs: system_ticks as f64 / clock_ticks as f64,
            rss_bytes: rss_kib
                .checked_mul(1024)
                .ok_or(MeasurementError::NumericOverflow)?,
        })
    }
}

#[cfg(target_os = "windows")]
mod os {
    use super::{MeasurementError, ProcessPoint};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    fn filetime_seconds(value: FILETIME) -> f64 {
        let ticks = (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
        ticks as f64 / 10_000_000.0
    }

    pub(super) fn read_process(
        pid: u32,
        wall_offset_secs: f64,
    ) -> Result<ProcessPoint, MeasurementError> {
        // SAFETY: flags request read-only process accounting access and pid is
        // supplied by the operator. The returned handle is closed below.
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            return Err(MeasurementError::ProcessUnavailable {
                pid,
                reason: std::io::Error::last_os_error().to_string(),
            });
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>())
                .map_err(|_| MeasurementError::NumericOverflow)?,
            ..Default::default()
        };
        // SAFETY: all output pointers refer to initialized, writable values;
        // `handle` remains valid until CloseHandle.
        let times_ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        // SAFETY: memory points to a correctly sized PROCESS_MEMORY_COUNTERS.
        let memory_ok = unsafe { GetProcessMemoryInfo(handle, &mut memory, memory.cb) };
        // SAFETY: handle was returned by OpenProcess and is closed once.
        unsafe { CloseHandle(handle) };
        if times_ok == 0 || memory_ok == 0 {
            return Err(MeasurementError::ProcessUnavailable {
                pid,
                reason: std::io::Error::last_os_error().to_string(),
            });
        }
        Ok(ProcessPoint {
            wall_offset_secs,
            cpu_user_secs: filetime_seconds(user),
            cpu_system_secs: filetime_seconds(kernel),
            rss_bytes: u64::try_from(memory.WorkingSetSize)
                .map_err(|_| MeasurementError::NumericOverflow)?,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod os {
    use super::{MeasurementError, ProcessPoint};

    pub(super) fn read_process(
        _pid: u32,
        _wall_offset_secs: f64,
    ) -> Result<ProcessPoint, MeasurementError> {
        Err(MeasurementError::UnsupportedOs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_record() -> MeasurementRecord {
        let process = |role, pid| ProcessRecord {
            role,
            name: format!("{role:?}"),
            pid,
            points: Vec::new(),
            cpu_delta_secs: 1.0,
            one_core_equivalent: 0.01,
            rss_mean_bytes: 1024,
            rss_peak_bytes: 2048,
        };
        let events = (0..300)
            .map(|id| PresentationEvent::Presented {
                correlation_id: id,
                timestamp_ns: id * 33_000_000,
                clock_id: 1,
                output: String::from("fixture-output"),
            })
            .collect();
        MeasurementRecord {
            exact_sha: String::from("0123456789abcdef0123456789abcdef01234567"),
            build_profile: String::from("release"),
            os: String::from("fixture"),
            kernel_or_build: String::from("fixture"),
            environment: Some(MeasurementEnvironment {
                desktop_session: String::from("fixture compositor"),
                cpu_model: String::from("fixture CPU"),
                gpu_model: String::from("fixture GPU"),
                gpu_driver: String::from("fixture driver"),
                display: String::from("1920x1080"),
                scale: String::from("1.0"),
                ui_language: String::from("ja"),
                asset: String::from("ene.vrm fixture hash"),
                render_backend: String::from("fixture backend"),
                build_flags: String::from("--release --locked"),
            }),
            logical_cpus: 4,
            started_unix_ms: 1,
            elapsed_wall_secs: 300.0,
            processes: vec![
                process(ProcessRole::Host, 1),
                process(ProcessRole::Desktop, 2),
                process(ProcessRole::Body, 3),
            ],
            cpu: Some(CpuRecord {
                cpu_seconds_sum: 3.0,
                machine_percent: 0.25,
                one_core_equivalent_sum: 0.01,
                busy_wait_pids: Vec::new(),
            }),
            rss: Some(RssRecord {
                mean_bytes: 3 * 1024,
                peak_bytes: 3 * 2048,
            }),
            fps: Some(
                PresentationRecord::from_events(
                    PresentationSource::WaylandWpPresentation {
                        body_pid: 3,
                        surface_id: String::from("wl_surface@7"),
                    },
                    5.0,
                    10.0,
                    events,
                )
                .expect("presentation"),
            ),
            interactions: vec![InteractionSample {
                operation: String::from("cancel_task"),
                input_monotonic_ns: 1_000,
                host_intake_monotonic_ns: 100_000_000,
                gui_painted_monotonic_ns: 200_000_000,
            }],
            click_through: Some(ClickThroughRecord {
                compositor: String::from("fixture"),
                transparent_click_reached_underlying_window: true,
                maximum_input_block_secs: 0.2,
                raw_evidence: String::from("probe.json"),
            }),
            verdict: MeasurementVerdict::UNMEASURED,
            failures: Vec::new(),
        }
    }

    #[test]
    fn pass_is_only_produced_after_all_evidence_is_evaluated() {
        let mut record = passing_record();
        assert!(!record.claims_pass());
        record.evaluate();
        assert!(record.claims_pass(), "{:?}", record.failures);
    }

    #[test]
    fn missing_evidence_does_not_hide_independent_gate_failures() {
        let mut record = passing_record();
        let events = (0..29)
            .map(|id| PresentationEvent::Presented {
                correlation_id: id,
                timestamp_ns: id.saturating_add(1) * 30_000_000,
                clock_id: 1,
                output: String::from("fixture-output"),
            })
            .collect();
        record.fps = Some(
            PresentationRecord::from_events(
                PresentationSource::WaylandWpPresentation {
                    body_pid: 3,
                    surface_id: String::from("wl_surface@7"),
                },
                5.0,
                1.0,
                events,
            )
            .expect("presentation"),
        );
        record.click_through = None;
        record.evaluate();
        assert_eq!(record.verdict.label(), "Incomplete");
        assert!(
            record
                .failures
                .iter()
                .any(|failure| failure == "click-through is unmeasured")
        );
        assert!(
            record
                .failures
                .iter()
                .any(|failure| failure.starts_with("presented FPS 29.000"))
        );
    }

    #[test]
    fn rejected_presentation_names_the_reason_on_the_failure_line() {
        let mut record = passing_record();
        let source = record.fps.as_ref().expect("fps").source.clone();
        // Every presented timestamp is valid and no frame is discarded or
        // missing, so the empty output is the only rejection.
        let events = (0..300)
            .map(|id| PresentationEvent::Presented {
                correlation_id: id,
                timestamp_ns: id * 33_000_000,
                clock_id: 1,
                output: String::new(),
            })
            .collect();
        record.fps =
            Some(PresentationRecord::from_events(source, 5.0, 10.0, events).expect("presentation"));
        record.evaluate();
        let failure = record
            .failures
            .iter()
            .find(|failure| failure.starts_with("presented FPS"))
            .expect("the rejected presentation is reported");
        assert!(
            failure.contains("presented output is empty"),
            "the operator line must name the rejection reason: {failure}"
        );
    }

    #[test]
    fn missing_presentation_feedback_cannot_pass() {
        let mut record = passing_record();
        let source = record.fps.as_ref().expect("fps").source.clone();
        record.fps = Some(
            PresentationRecord::from_events(
                source,
                5.0,
                1.0,
                vec![
                    PresentationEvent::Presented {
                        correlation_id: 1,
                        timestamp_ns: 1,
                        clock_id: 1,
                        output: String::from("out"),
                    },
                    PresentationEvent::Missing {
                        correlation_id: 2,
                        reason: String::from("feedback never resolved"),
                    },
                ],
            )
            .expect("record"),
        );
        record.evaluate();
        assert!(!record.claims_pass());
        assert_eq!(record.verdict.label(), "Fail");
    }

    #[test]
    fn unresolved_body_submission_becomes_missing_feedback() {
        let surface_id = String::from("wl_surface@12");
        let feedback = vec![
            ene_body::ipc::PresentationFeedback {
                surface_id: surface_id.clone(),
                correlation_id: 10,
                outcome: ene_body::ipc::PresentationOutcome::Submitted,
            },
            ene_body::ipc::PresentationFeedback {
                surface_id: surface_id.clone(),
                correlation_id: 10,
                outcome: ene_body::ipc::PresentationOutcome::Presented {
                    timestamp_ns: 99,
                    clock_id: 1,
                    output: String::from("DP-1"),
                },
            },
            ene_body::ipc::PresentationFeedback {
                surface_id,
                correlation_id: 11,
                outcome: ene_body::ipc::PresentationOutcome::Submitted,
            },
        ];
        let record =
            wayland_presentation_record(42, 1.0, 1.0, feedback).expect("correlated feedback");
        assert_eq!(record.presented, 1);
        assert_eq!(record.missing, 1);
        assert!(!record.passes());
    }

    #[test]
    fn dropped_frames_use_presented_count_not_request_count() {
        let events = (0..60)
            .map(|id| {
                if id < 20 {
                    PresentationEvent::Presented {
                        correlation_id: id,
                        timestamp_ns: id,
                        clock_id: 1,
                        output: String::from("out"),
                    }
                } else {
                    PresentationEvent::Discarded { correlation_id: id }
                }
            })
            .collect();
        let fps = PresentationRecord::from_events(
            PresentationSource::WaylandWpPresentation {
                body_pid: 3,
                surface_id: String::from("surface"),
            },
            0.0,
            1.0,
            events,
        )
        .expect("fps");
        assert_eq!(fps.presented, 20);
        assert_eq!(fps.discarded, 40);
        assert_eq!(fps.actual_fps, 20.0);
        assert!(!fps.passes());
    }

    #[test]
    fn any_discarded_feedback_prevents_pass_even_above_thirty_fps() {
        let mut events = (0..31)
            .map(|id| PresentationEvent::Presented {
                correlation_id: id,
                timestamp_ns: id.saturating_add(1) * 30_000_000,
                clock_id: 1,
                output: String::from("out"),
            })
            .collect::<Vec<_>>();
        events.push(PresentationEvent::Discarded { correlation_id: 32 });
        let fps = PresentationRecord::from_events(
            PresentationSource::WaylandWpPresentation {
                body_pid: 3,
                surface_id: String::from("surface"),
            },
            0.0,
            1.0,
            events,
        )
        .expect("fps");
        assert!(fps.actual_fps >= 30.0);
        assert!(!fps.passes());
    }

    #[test]
    fn presentmon_missing_display_timing_is_not_presented() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("presentmon.csv");
        std::fs::write(
            &path,
            "ProcessID,SwapChainAddress,TimeInSeconds,MsUntilDisplayed,DisplayedTime,Dropped\n42,0xabc,1.0,5.0,16.6,false\n42,0xabc,1.1,,,false\n42,0xabc,1.2,,,true\n",
        )
        .expect("trace");
        let record = import_presentmon_csv(&path, 42, "0xabc", 0.0, 2.0).expect("import");
        assert_eq!(record.presented, 1);
        assert_eq!(record.missing, 1);
        assert_eq!(record.discarded, 1);
        assert!(!record.passes());
    }

    #[test]
    fn presentmon_default_millisecond_columns_use_fixed_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("presentmon.csv");
        std::fs::write(
            &path,
            "ProcessID,SwapChainAddress,CPUStartTime,DisplayLatency,DisplayedTime\n42,0xabc,4000,15,16.6\n42,0xabc,5100,20,16.6\n42,0xabc,6100,,NA\n42,0xabc,8000,20,16.6\n",
        )
        .expect("trace");
        let record = import_presentmon_csv(&path, 42, "0xabc", 5.0, 2.0).expect("import");
        assert_eq!(record.presented, 1);
        assert_eq!(record.discarded, 1);
        assert_eq!(record.missing, 0);
        assert!(matches!(
            record.events.first(),
            Some(PresentationEvent::Presented {
                timestamp_ns: 5_120_000_000,
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_sampler_reads_current_process() {
        let point = os::read_process(std::process::id(), 0.0).expect("self sample");
        assert!(point.cpu_user_secs >= 0.0);
        assert!(point.rss_bytes > 0);
    }
}
