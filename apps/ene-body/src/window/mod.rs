//! OS overlay surface.
//!
//! wgpu and each native surface are co-owned by the selected backend. Native
//! backend failure falls back explicitly to headless and is never acceptance
//! evidence.

mod dwm;
mod headless;
mod wayland;

pub use dwm::windows_dwm_probe;
pub use headless::HeadlessOverlay;
pub use wayland::kde_layer_shell_probe;

use crate::ipc::{LocalUiFact, OverlayKind, PlacementBox};

/// Outcome of asking for a real OS overlay. Never pretended as a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayProbe {
    Available,
    Unavailable { reason: String },
}

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

    pub(crate) fn unavailable(reason: impl Into<String>) -> Self {
        Self::Headless(HeadlessOverlay::unavailable(reason.into()))
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
            Self::Headless(inner) => inner.set_visible(visible),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.set_visible(visible),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.set_visible(visible),
        }
    }

    #[must_use]
    pub fn visible(&self) -> bool {
        match self {
            Self::Headless(inner) => inner.visible(),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.visible(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.visible(),
        }
    }

    pub fn set_placement(&mut self, placement: PlacementBox) {
        match self {
            Self::Headless(inner) => inner.set_placement(placement),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.set_placement(placement),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.set_placement(placement),
        }
    }

    #[must_use]
    pub fn placement(&self) -> PlacementBox {
        match self {
            Self::Headless(inner) => inner.placement(),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.placement(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.placement(),
        }
    }

    /// Local UI facts exist so a future OS backend can report drag/resize/hide
    /// without talking to Host. Headless never synthesizes them.
    #[must_use]
    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        match self {
            Self::Headless(inner) => inner.take_local_ui(),
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

    #[must_use]
    pub fn gpu_failure(&self) -> Option<crate::ipc::GpuFailInfo> {
        match self {
            Self::Headless(_) => Some(crate::ipc::GpuFailInfo {
                reason: crate::ipc::GpuFailReason::Surface,
            }),
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.gpu_failure(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.gpu_failure(),
        }
    }

    /// Dispatches native window/compositor events without presenting.
    pub fn pump(&mut self) {
        match self {
            Self::Headless(_) => {}
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(inner) => inner.pump(),
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(inner) => inner.pump(),
        }
    }

    /// True only when a native surface is visible, paced, and able to present.
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
            Self::Headless(inner) => {
                inner
                    .unavailable_reason()
                    .map(|reason| crate::ipc::OverlayUnavailableInfo {
                        requested: if cfg!(target_os = "windows") {
                            OverlayKind::WindowsDwm
                        } else {
                            OverlayKind::KdeLayerShell
                        },
                        reason: reason.to_string(),
                    })
            }
            #[cfg(target_os = "linux")]
            Self::KdeLayerShell(_) => None,
            #[cfg(target_os = "windows")]
            Self::WindowsDwm(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Overlay;
    use crate::ipc::{LocalUiFact, OverlayKind, PlacementBox};

    #[test]
    fn explicit_test_overlay_is_headless() {
        let overlay = Overlay::unavailable("test fixture");
        assert_eq!(overlay.kind(), OverlayKind::Headless);
        assert!(!overlay.visible());
    }

    #[test]
    fn headless_records_show_hide_and_placement() {
        let mut overlay = Overlay::unavailable("test fixture");
        overlay.set_visible(true);
        overlay.set_placement(PlacementBox {
            x: 8,
            y: 16,
            width: 100,
            height: 200,
            scale: 1.5,
        });
        assert!(overlay.visible());
        assert_eq!(overlay.placement().width, 100);
        overlay.set_visible(false);
        assert!(!overlay.visible());
        assert_eq!(overlay.take_local_ui(), None);
    }

    #[test]
    fn headless_can_queue_local_ui_for_contract_tests() {
        let mut overlay = Overlay::unavailable("test fixture");
        match &mut overlay {
            Overlay::Headless(inner) => inner.push_local_ui(LocalUiFact::Hide),
            #[cfg(target_os = "linux")]
            Overlay::KdeLayerShell(_) => panic!("test environment unexpectedly opened Wayland"),
            #[cfg(target_os = "windows")]
            Overlay::WindowsDwm(_) => panic!("test environment unexpectedly opened DWM"),
        }
        assert_eq!(overlay.take_local_ui(), Some(LocalUiFact::Hide));
        assert_eq!(overlay.take_local_ui(), None);
    }
}
