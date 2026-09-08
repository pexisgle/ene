use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Backend {
    pub name: &'static str,
    pub executable: &'static str,
    pub install_hint: &'static str,
}

pub(crate) const WMCTRL: Backend = Backend {
    name: "wmctrl",
    executable: "wmctrl",
    install_hint: "install wmctrl; for Debian/Ubuntu: sudo apt install wmctrl",
};

pub(crate) const HYPRLAND: Backend = Backend {
    name: "hyprctl",
    executable: "hyprctl",
    install_hint: "install Hyprland's hyprctl client",
};

pub(crate) const SWAY: Backend = Backend {
    name: "swaymsg",
    executable: "swaymsg",
    install_hint: "install Sway's swaymsg client",
};

pub(crate) const WIN32: Backend = Backend {
    name: "win32",
    executable: "win32",
    install_hint: "the Win32 window API is unavailable",
};

pub(crate) const UNSUPPORTED: Backend = Backend {
    name: "none",
    executable: "",
    install_hint: "window listing is unsupported",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendAvailability {
    Available(Backend),
    Missing(Backend),
    Unsupported(Backend, &'static str),
}

impl BackendAvailability {
    pub(crate) const fn backend(&self) -> &Backend {
        match self {
            Self::Available(backend) | Self::Missing(backend) | Self::Unsupported(backend, _) => {
                backend
            }
        }
    }

    pub(crate) const fn available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    pub(crate) fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Available(_) => None,
            Self::Missing(backend) => Some(match backend.name {
                "wmctrl" => "wmctrl is not installed",
                "hyprctl" => "hyprctl is not installed",
                "swaymsg" => "swaymsg is not installed",
                _ => "window-list backend is missing",
            }),
            Self::Unsupported(_, reason) => Some(*reason),
        }
    }
}

pub(crate) fn resolve(
    candidates: &[Backend],
    probe: impl Fn(&str) -> Option<PathBuf>,
) -> BackendAvailability {
    candidates
        .iter()
        .copied()
        .find_map(|candidate| probe(candidate.executable).map(|_| candidate))
        .map_or_else(
            || BackendAvailability::Missing(candidates[0]),
            BackendAvailability::Available,
        )
}

#[cfg(test)]
mod tests {
    use super::{BackendAvailability, HYPRLAND, SWAY, WMCTRL, resolve};
    use std::collections::HashMap;

    #[test]
    fn resolves_first_present_backend_and_keeps_fallback_for_missing() {
        let paths = HashMap::from([("swaymsg", "/bin/swaymsg")]);
        let found = |executable: &str| paths.get(executable).map(|path| (**path).into());

        assert_eq!(
            resolve(&[WMCTRL, HYPRLAND, SWAY], found),
            BackendAvailability::Available(SWAY)
        );
        assert_eq!(
            resolve(&[WMCTRL, HYPRLAND], found),
            BackendAvailability::Missing(WMCTRL)
        );
    }
}
