//! Monitor discovery and overlay target resolution.

use std::cmp::{max, min};

use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;

pub const MODE_PRIMARY: &str = "primary";
pub const MODE_SELECTED: &str = "selected";
pub const MODE_POINTER: &str = "pointer";
pub const MODE_ALL: &str = "all";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMonitorMode {
    Primary,
    Selected,
    Pointer,
    All,
}

impl OverlayMonitorMode {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value {
            MODE_SELECTED => Self::Selected,
            MODE_POINTER => Self::Pointer,
            MODE_ALL => Self::All,
            _ => Self::Primary,
        }
    }

    #[must_use]
    pub const fn setting(self) -> &'static str {
        match self {
            Self::Primary => MODE_PRIMARY,
            Self::Selected => MODE_SELECTED,
            Self::Pointer => MODE_POINTER,
            Self::All => MODE_ALL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub position: [i32; 2],
    pub size: [u32; 2],
}

impl MonitorRect {
    #[must_use]
    pub fn right(self) -> i64 {
        i64::from(self.position[0]) + i64::from(self.size[0])
    }

    #[must_use]
    pub fn bottom(self) -> i64 {
        i64::from(self.position[1]) + i64::from(self.size[1])
    }

    #[must_use]
    pub fn contains(self, point: [i32; 2]) -> bool {
        let x = i64::from(point[0]);
        let y = i64::from(point[1]);
        x >= i64::from(self.position[0])
            && x < self.right()
            && y >= i64::from(self.position[1])
            && y < self.bottom()
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let left = min(i64::from(self.position[0]), i64::from(other.position[0]));
        let top = min(i64::from(self.position[1]), i64::from(other.position[1]));
        let right = max(self.right(), other.right());
        let bottom = max(self.bottom(), other.bottom());
        Self {
            position: [saturating_i32(left), saturating_i32(top)],
            size: [saturating_u32(right - left), saturating_u32(bottom - top)],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: String,
    pub name: Option<String>,
    pub position: [i32; 2],
    pub size: [u32; 2],
    pub scale_factor: f64,
    pub is_primary: bool,
    pub ordinal: usize,
}

impl MonitorInfo {
    #[must_use]
    pub const fn rect(&self) -> MonitorRect {
        MonitorRect {
            position: self.position,
            size: self.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMonitorTarget {
    pub rect: MonitorRect,
    pub monitor: Option<MonitorInfo>,
    pub fallback: bool,
}

#[must_use]
pub fn stable_id(handle: &MonitorHandle) -> String {
    let position = handle.position();
    stable_id_from_parts(handle.name().as_deref(), [position.x, position.y])
}

#[must_use]
pub fn stable_id_from_parts(name: Option<&str>, position: [i32; 2]) -> String {
    name.filter(|value| !value.trim().is_empty()).map_or_else(
        || format!("position:{},{}", position[0], position[1]),
        |value| format!("name:{value}"),
    )
}

#[must_use]
pub fn inventory(event_loop: &ActiveEventLoop) -> Vec<MonitorInfo> {
    let primary_id = event_loop.primary_monitor().as_ref().map(stable_id);
    let mut monitors = Vec::new();
    for handle in event_loop.available_monitors() {
        let id = stable_id(&handle);
        if monitors
            .iter()
            .any(|monitor: &MonitorInfo| monitor.id == id)
        {
            continue;
        }
        let position = handle.position();
        let size = handle.size();
        let scale_factor = handle.scale_factor();
        monitors.push(MonitorInfo {
            is_primary: primary_id.as_ref().is_some_and(|primary| primary == &id),
            id,
            name: handle.name(),
            position: [position.x, position.y],
            size: [size.width.max(1), size.height.max(1)],
            scale_factor: if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            },
            ordinal: 0,
        });
    }
    if let Some(primary) = event_loop.primary_monitor() {
        let id = stable_id(&primary);
        if !monitors.iter().any(|monitor| monitor.id == id) {
            let position = primary.position();
            let size = primary.size();
            let scale_factor = primary.scale_factor();
            monitors.push(MonitorInfo {
                id,
                name: primary.name(),
                position: [position.x, position.y],
                size: [size.width.max(1), size.height.max(1)],
                scale_factor: if scale_factor.is_finite() && scale_factor > 0.0 {
                    scale_factor
                } else {
                    1.0
                },
                is_primary: true,
                ordinal: 0,
            });
        }
    }
    monitors.sort_by_key(|monitor| (monitor.position[1], monitor.position[0], monitor.id.clone()));
    for (ordinal, monitor) in monitors.iter_mut().enumerate() {
        monitor.ordinal = ordinal;
    }
    monitors
}

#[must_use]
pub fn union_rect(monitors: &[MonitorInfo]) -> Option<MonitorRect> {
    monitors
        .iter()
        .map(MonitorInfo::rect)
        .reduce(MonitorRect::union)
}

#[must_use]
pub fn find_saved_monitor<'a>(
    monitors: &'a [MonitorInfo],
    saved_id: &str,
    saved_name: &str,
    saved_position: [i32; 2],
) -> Option<&'a MonitorInfo> {
    if !saved_id.is_empty()
        && let Some(monitor) = monitors.iter().find(|monitor| monitor.id == saved_id)
    {
        return Some(monitor);
    }
    if !saved_name.is_empty()
        && let Some(monitor) = monitors
            .iter()
            .find(|monitor| monitor.name.as_deref() == Some(saved_name))
    {
        return Some(monitor);
    }
    if saved_name.is_empty() && !saved_id.is_empty() {
        monitors
            .iter()
            .find(|monitor| monitor.position == saved_position)
    } else {
        None
    }
}

#[must_use]
pub fn resolve_target(
    monitors: &[MonitorInfo],
    mode: OverlayMonitorMode,
    saved_id: &str,
    saved_name: &str,
    saved_position: [i32; 2],
    pointer: Option<[i32; 2]>,
    pointer_fallback_id: Option<&str>,
) -> Option<ResolvedMonitorTarget> {
    if monitors.is_empty() {
        return None;
    }
    if mode == OverlayMonitorMode::All {
        return union_rect(monitors).map(|rect| ResolvedMonitorTarget {
            rect,
            monitor: None,
            fallback: false,
        });
    }
    let primary = || {
        monitors
            .iter()
            .find(|monitor| monitor.is_primary)
            .or_else(|| monitors.first())
    };
    let (monitor, fallback) = match mode {
        OverlayMonitorMode::Primary | OverlayMonitorMode::All => (primary(), false),
        OverlayMonitorMode::Selected => {
            let selected = find_saved_monitor(monitors, saved_id, saved_name, saved_position);
            (selected.or_else(primary), selected.is_none())
        }
        OverlayMonitorMode::Pointer => {
            let under_pointer = pointer.and_then(|point| {
                monitors
                    .iter()
                    .find(|monitor| monitor.rect().contains(point))
            });
            let remembered =
                pointer_fallback_id.and_then(|id| monitors.iter().find(|monitor| monitor.id == id));
            (
                under_pointer.or(remembered).or_else(primary),
                under_pointer.is_none() && remembered.is_none(),
            )
        }
    };
    monitor.cloned().map(|monitor| ResolvedMonitorTarget {
        rect: monitor.rect(),
        monitor: Some(monitor),
        fallback,
    })
}

fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

fn saturating_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, position: [i32; 2], size: [u32; 2], is_primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.to_owned(),
            name: Some(id.to_owned()),
            position,
            size,
            scale_factor: 1.0,
            is_primary,
            ordinal: 0,
        }
    }

    #[test]
    fn mode_parsing_defaults_to_primary() {
        assert_eq!(
            OverlayMonitorMode::from_setting(MODE_PRIMARY),
            OverlayMonitorMode::Primary
        );
        assert_eq!(
            OverlayMonitorMode::from_setting(MODE_SELECTED),
            OverlayMonitorMode::Selected
        );
        assert_eq!(
            OverlayMonitorMode::from_setting("unknown"),
            OverlayMonitorMode::Primary
        );
    }

    #[test]
    fn named_monitor_id_survives_resolution_changes() {
        assert_eq!(
            stable_id_from_parts(Some("\\\\.\\DISPLAY2"), [1920, 0]),
            "name:\\\\.\\DISPLAY2"
        );
        assert_eq!(stable_id_from_parts(None, [-1920, 0]), "position:-1920,0");
    }

    #[test]
    fn union_covers_negative_and_adjacent_monitor_coordinates() {
        let left = MonitorRect {
            position: [-1920, 0],
            size: [1920, 1080],
        };
        let right = MonitorRect {
            position: [0, 0],
            size: [2560, 1440],
        };
        let union = left.union(right);
        assert_eq!(union.position, [-1920, 0]);
        assert_eq!(union.size, [4480, 1440]);
    }

    #[test]
    fn missing_selected_monitor_falls_back_to_primary() {
        let monitors = vec![monitor("primary", [0, 0], [1920, 1080], true)];
        let target = resolve_target(
            &monitors,
            OverlayMonitorMode::Selected,
            "secondary",
            "Secondary",
            [1920, 0],
            None,
            None,
        )
        .expect("primary monitor exists");
        assert_eq!(
            target.monitor.as_ref().map(|monitor| monitor.id.as_str()),
            Some("primary")
        );
        assert!(target.fallback);
    }

    #[test]
    fn selected_monitor_does_not_report_a_fallback() {
        let monitors = vec![
            monitor("primary", [0, 0], [1920, 1080], true),
            monitor("secondary", [1920, 0], [2560, 1440], false),
        ];
        let target = resolve_target(
            &monitors,
            OverlayMonitorMode::Selected,
            "secondary",
            "",
            [0, 0],
            None,
            None,
        )
        .expect("selected monitor exists");
        assert_eq!(
            target.monitor.as_ref().map(|monitor| monitor.id.as_str()),
            Some("secondary")
        );
        assert!(!target.fallback);
    }

    #[test]
    fn named_monitor_missing_at_saved_position_falls_back() {
        let monitors = vec![
            monitor("primary", [0, 0], [1920, 1080], true),
            monitor("replacement", [1920, 0], [2560, 1440], false),
        ];
        let target = resolve_target(
            &monitors,
            OverlayMonitorMode::Selected,
            "name:secondary",
            "secondary",
            [1920, 0],
            None,
            None,
        )
        .expect("primary monitor exists");
        assert_eq!(
            target.monitor.as_ref().map(|monitor| monitor.id.as_str()),
            Some("primary")
        );
        assert!(target.fallback);
    }

    #[test]
    fn pointer_resolution_prefers_monitor_under_pointer() {
        let monitors = vec![
            monitor("primary", [0, 0], [1920, 1080], true),
            monitor("secondary", [1920, 0], [2560, 1440], false),
        ];
        let target = resolve_target(
            &monitors,
            OverlayMonitorMode::Pointer,
            "",
            "",
            [0, 0],
            Some([2200, 400]),
            None,
        )
        .expect("pointer monitor exists");
        assert_eq!(
            target.monitor.as_ref().map(|monitor| monitor.id.as_str()),
            Some("secondary")
        );
        assert!(!target.fallback);
    }
}
