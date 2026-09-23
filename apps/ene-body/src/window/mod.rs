mod dwm;
mod headless;
mod wayland;

pub use headless::HeadlessOverlay;

use crate::ipc::{LocalUiFact, OverlayKind, PlacementBox};

#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::render::RenderFailure;

/// Scales a logical extent to physical pixels, never below one pixel.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn physical(logical: u32, scale: f32) -> u32 {
    ((logical as f64 * f64::from(scale)).round() as u64).clamp(1, u64::from(u32::MAX)) as u32
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn gpu_info(failure: RenderFailure) -> crate::ipc::GpuFailInfo {
    crate::ipc::GpuFailInfo {
        reason: match failure {
            RenderFailure::Adapter => crate::ipc::GpuFailReason::NoAdapter,
            RenderFailure::Device => crate::ipc::GpuFailReason::RequestDevice,
            RenderFailure::Surface => crate::ipc::GpuFailReason::Surface,
            RenderFailure::DeviceLost => crate::ipc::GpuFailReason::DeviceLost,
        },
    }
}

/// Reported when `try_gpu` is false: no adapter is ever attempted.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn gpu_disabled() -> crate::ipc::GpuFailInfo {
    crate::ipc::GpuFailInfo {
        reason: crate::ipc::GpuFailReason::NoAdapter,
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn gpu_status(
    renderer: Option<&crate::render::SurfaceRenderer>,
) -> crate::ipc::GpuInitStatus {
    if renderer.is_some() {
        crate::ipc::GpuInitStatus::Ok
    } else {
        crate::ipc::GpuInitStatus::Failed
    }
}

/// Logical-pixel extent of the resize target, anchored to the rightmost
/// visible character pixels in the bottom band (mirrors the Windows
/// `WM_NCHITTEST` grip).
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) const RESIZE_GRIP_LOGICAL_PX: u32 = 32;

/// Initial overlay placement shared by the native backends.
pub const DEFAULT_PLACEMENT: PlacementBox = PlacementBox {
    x: 24,
    y: 24,
    width: 420,
    height: 640,
    scale: 1.0,
};

/// Native backend this build would request; off Linux/Windows there is none.
#[cfg(target_os = "linux")]
const REQUESTED_NATIVE: OverlayKind = OverlayKind::KdeLayerShell;
#[cfg(target_os = "windows")]
const REQUESTED_NATIVE: OverlayKind = OverlayKind::WindowsDwm;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
const REQUESTED_NATIVE: OverlayKind = OverlayKind::Headless;

/// Overlay in this process. Production attempts the native backend and reports
/// an explicit unavailable outcome before using Headless.
#[derive(Debug)]
pub enum Overlay {
    Headless(HeadlessOverlay),
    #[cfg(target_os = "linux")]
    KdeLayerShell(Box<wayland::WaylandOverlay>),
    #[cfg(target_os = "windows")]
    WindowsDwm(Box<dwm::WindowsOverlay>),
}

impl Overlay {
    #[must_use]
    pub fn open(try_gpu: bool) -> Self {
        #[cfg(target_os = "linux")]
        {
            match wayland::WaylandOverlay::open(try_gpu) {
                Ok(overlay) => Self::KdeLayerShell(Box::new(overlay)),
                Err(reason) => Self::Headless(HeadlessOverlay::unavailable(reason)),
            }
        }
        #[cfg(target_os = "windows")]
        {
            match dwm::WindowsOverlay::open(try_gpu) {
                Ok(overlay) => Self::WindowsDwm(Box::new(overlay)),
                Err(reason) => Self::Headless(HeadlessOverlay::unavailable(reason)),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = try_gpu;
            Self::Headless(HeadlessOverlay::unavailable(String::from(
                "no production overlay backend for this OS",
            )))
        }
    }

    #[must_use]
    pub fn kind(&self) -> OverlayKind {
        match self {
            Self::Headless(_) => OverlayKind::Headless,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(_) => OverlayKind::KdeLayerShell,
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(_) => OverlayKind::WindowsDwm,
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        match self {
            Self::Headless(_) => {
                let _ = visible;
            }
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.set_visible(visible),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.set_visible(visible),
        }
    }

    pub fn set_placement(&mut self, placement: PlacementBox) {
        match self {
            Self::Headless(_) => {
                let _ = placement;
            }
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.set_placement(placement),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.set_placement(placement),
        }
    }

    /// Native backends report overlay-local drag/resize/hide as `LocalUiFact`;
    /// Headless never synthesizes them.
    #[must_use]
    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        match self {
            Self::Headless(_) => None,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.take_local_ui(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.take_local_ui(),
        }
    }

    #[must_use]
    pub fn gpu_status(&self) -> crate::ipc::GpuInitStatus {
        match self {
            Self::Headless(_) => crate::ipc::GpuInitStatus::Failed,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.gpu_status(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.gpu_status(),
        }
    }

    /// Headless attempts no surface creation, so there is no GPU failure to
    /// report; [`Self::unavailable_info`] carries the overlay's reason.
    #[must_use]
    pub fn gpu_failure(&self) -> Option<crate::ipc::GpuFailInfo> {
        match self {
            Self::Headless(_) => None,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.gpu_failure(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.gpu_failure(),
        }
    }

    pub fn pump(&mut self) {
        match self {
            Self::Headless(_) => {}
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.pump(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.pump(),
        }
    }

    #[must_use]
    pub fn ready_to_render(&self) -> bool {
        match self {
            Self::Headless(_) => false,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.ready_to_render(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.ready_to_render(),
        }
    }

    pub fn render(&mut self, meshes: &[crate::vrm::RenderMesh]) {
        match self {
            Self::Headless(_) => {}
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.render(meshes),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.render(meshes),
        }
    }

    #[must_use]
    pub fn take_presentation(&mut self) -> Option<crate::ipc::PresentationFeedback> {
        match self {
            Self::Headless(_) => None,
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.take_presentation(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.take_presentation(),
        }
    }

    #[must_use]
    pub fn unavailable_info(&self) -> Option<crate::ipc::OverlayUnavailableInfo> {
        match self {
            Self::Headless(inner) => Some(crate::ipc::OverlayUnavailableInfo {
                requested: REQUESTED_NATIVE,
                reason: inner.reason().to_string(),
            }),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(_) => None,
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(_) => None,
        }
    }
}
