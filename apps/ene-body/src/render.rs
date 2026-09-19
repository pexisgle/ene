//! wgpu device ownership for this process only.
//!
//! Host and `ene-desktop` must not open a GPU device. A successful adapter
//! here is not an overlay probe pass: there is no OS surface while
//! [`crate::window`] is headless, and presented-FPS is not measured.
//!
//! Init failure is a domain [`crate::ipc::GpuFailInfo`], not a process abort.
//! Hide / headless skip present so idle does not busy-wait.

use std::time::Duration;

use crate::ipc::{GpuFailInfo, GpuFailReason, GpuInitStatus};

/// Bound GPU init so a missing/hung ICD cannot stall IPC forever.
pub const GPU_INIT_TIMEOUT: Duration = Duration::from_secs(3);

/// wgpu occupancy for the overlay process.
#[derive(Debug)]
pub enum Gpu {
    Ready {
        _instance: wgpu::Instance,
        _adapter: wgpu::Adapter,
        _device: wgpu::Device,
        _queue: wgpu::Queue,
    },
    Failed(GpuFailInfo),
}

impl Gpu {
    #[must_use]
    pub fn status(&self) -> GpuInitStatus {
        match self {
            Self::Ready { .. } => GpuInitStatus::Ok,
            Self::Failed(_) => GpuInitStatus::Failed,
        }
    }

    #[must_use]
    pub fn fail_info(&self) -> Option<GpuFailInfo> {
        match self {
            Self::Ready { .. } => None,
            Self::Failed(info) => Some(*info),
        }
    }

    #[must_use]
    pub fn ok(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Attempt a device. Times out into [`GpuFailReason::InitTimeout`].
    pub async fn init() -> Self {
        match tokio::time::timeout(GPU_INIT_TIMEOUT, init_inner()).await {
            Ok(gpu) => gpu,
            Err(_) => Self::Failed(GpuFailInfo {
                reason: GpuFailReason::InitTimeout,
            }),
        }
    }

    /// Skip wgpu entirely (library tests that only exercise IPC/state).
    #[must_use]
    pub fn skipped() -> Self {
        Self::Failed(GpuFailInfo {
            reason: GpuFailReason::NoAdapter,
        })
    }

    /// Present is paused while hidden or while the overlay has no OS surface.
    /// A ready device still does not busy-loop Present() in this slice.
    pub fn present_tick(&self, visible: bool) {
        let _pause: bool = !visible || !self.ok();
    }
}

async fn init_inner() -> Gpu {
    // Instance construction probes the OS and its backends synchronously: on a
    // machine whose driver is absent, half-installed, or hung (a container, a
    // WSL GPU passthrough, a partially removed ICD), that probe can block
    // without ever yielding, so the caller's bound would never fire. Running it
    // on a blocking thread keeps the timeout enforceable and leaves the IPC
    // loop able to answer.
    let instance = match tokio::task::spawn_blocking(wgpu::Instance::default).await {
        Ok(instance) => instance,
        Err(_) => {
            return Gpu::Failed(GpuFailInfo {
                reason: GpuFailReason::NoAdapter,
            });
        }
    };
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        })
        .await
    {
        Ok(adapter) => adapter,
        Err(_) => {
            return Gpu::Failed(GpuFailInfo {
                reason: GpuFailReason::NoAdapter,
            });
        }
    };
    match adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("ene-body"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await
    {
        Ok((device, queue)) => Gpu::Ready {
            _instance: instance,
            _adapter: adapter,
            _device: device,
            _queue: queue,
        },
        Err(_) => Gpu::Failed(GpuFailInfo {
            reason: GpuFailReason::RequestDevice,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{GPU_INIT_TIMEOUT, Gpu};
    use crate::ipc::{GpuFailReason, GpuInitStatus};

    #[test]
    fn skipped_gpu_is_a_fail_status_not_a_panic() {
        let gpu = Gpu::skipped();
        assert_eq!(gpu.status(), GpuInitStatus::Failed);
        assert!(!gpu.ok());
        let info = gpu.fail_info().expect("fail");
        assert_eq!(info.reason, GpuFailReason::NoAdapter);
        gpu.present_tick(true);
        gpu.present_tick(false);
    }

    #[test]
    fn init_timeout_is_bounded() {
        assert_eq!(GPU_INIT_TIMEOUT.as_secs(), 3);
    }
}
