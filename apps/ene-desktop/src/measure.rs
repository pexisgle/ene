//! Slice F measurement recording format (first-party-desktop §8).
//!
//! This is a skeleton for CPU / RSS / FPS evidence. It does **not** claim a
//! Performance Gate pass. Windows 11, NixOS 26.11 KDE Wayland, live IME, and
//! overlay probes remain 未実施. `wl_surface.frame` and Present() call counts
//! are not FPS evidence.

use serde::{Deserialize, Serialize};

/// One recorded measurement campaign. Default is unmeasured, never pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementRecord {
    pub os: String,
    pub kernel_or_build: String,
    pub logical_cpus: u32,
    pub elapsed_wall_secs: f64,
    pub processes: Vec<ProcessSample>,
    pub cpu: CpuRecord,
    pub rss: RssRecord,
    pub fps: FpsRecord,
    pub verdict: MeasurementVerdict,
}

impl Default for MeasurementRecord {
    fn default() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            kernel_or_build: String::from("unmeasured"),
            logical_cpus: 0,
            elapsed_wall_secs: 0.0,
            processes: Vec::new(),
            cpu: CpuRecord::Unmeasured,
            rss: RssRecord::Unmeasured,
            fps: FpsRecord::Unmeasured {
                reason: String::from(
                    "presented-frame timing is 未実施 (Wayland wp_presentation / PresentMon)",
                ),
            },
            verdict: MeasurementVerdict::Unmeasured,
        }
    }
}

/// Per-PID sample. Body is included; it is never omitted from the sum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub name: String,
    pub pid: u32,
    pub cpu_user_secs: f64,
    pub cpu_system_secs: f64,
    pub rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CpuRecord {
    Unmeasured,
    Sampled {
        cpu_seconds_sum: f64,
        machine_percent: f64,
        one_core_equivalent: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RssRecord {
    Unmeasured,
    Sampled { peak_bytes: u64, mean_bytes: u64 },
}

/// FPS must use presented frames. Missing timing is unmeasured, not zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FpsRecord {
    Unmeasured {
        reason: String,
    },
    Presented {
        presented: u64,
        discarded: u64,
        missing: u64,
        wall_secs: f64,
    },
}

/// Gate outcome. This skeleton never constructs [`Self::Pass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasurementVerdict {
    Unmeasured,
    Incomplete,
    Fail,
    Pass,
}

impl MeasurementRecord {
    /// Idle campaign template. Does not sample; verdict stays unmeasured.
    #[must_use]
    pub fn idle_template() -> Self {
        Self {
            processes: vec![
                named_process("ene-core"),
                named_process("ene-desktop"),
                named_process("ene-body"),
            ],
            ..Self::default()
        }
    }

    #[must_use]
    pub fn claims_pass(&self) -> bool {
        matches!(self.verdict, MeasurementVerdict::Pass)
    }
}

fn named_process(name: &str) -> ProcessSample {
    ProcessSample {
        name: name.to_string(),
        pid: 0,
        cpu_user_secs: 0.0,
        cpu_system_secs: 0.0,
        rss_bytes: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{FpsRecord, MeasurementRecord, MeasurementVerdict};

    #[test]
    fn skeleton_does_not_claim_gate_pass() {
        let record = MeasurementRecord::idle_template();
        assert_eq!(record.verdict, MeasurementVerdict::Unmeasured);
        assert!(!record.claims_pass());
        assert!(matches!(record.fps, FpsRecord::Unmeasured { .. }));
        let json = serde_json::to_string(&record).expect("format");
        assert!(json.contains("ene-body"));
        assert!(json.contains("Unmeasured"));
        assert!(!json.contains("\"Pass\""));
    }
}
