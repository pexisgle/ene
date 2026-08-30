//! Pure OS input-region generation, dirty detection, and routing.

use crate::input::{PointerTarget, ScreenPoint, ScreenRect, VrmHitLayout, VrmPart, route_pointer};

/// Interactive OS region = UI bounds ∪ VRM coarse bounds.
#[must_use]
pub fn build_input_regions(
    ui_regions: &[ScreenRect],
    vrm_regions: &[ScreenRect],
) -> Vec<ScreenRect> {
    let mut out = Vec::with_capacity(ui_regions.len() + vrm_regions.len());
    for rect in ui_regions
        .iter()
        .copied()
        .chain(vrm_regions.iter().copied())
    {
        if !rect.is_empty() {
            out.push(rect);
        }
    }
    out
}

#[must_use]
pub fn vrm_regions(layout: Option<&VrmHitLayout>) -> Vec<ScreenRect> {
    let Some(layout) = layout else {
        return Vec::new();
    };
    layout
        .parts()
        .into_iter()
        .map(|(_part, rect)| rect)
        .filter(|rect| !rect.is_empty())
        .collect()
}

/// True when any rect moved/resized by more than `threshold_px`, or the
/// set of regions changed.
#[must_use]
pub fn regions_dirty(previous: &[ScreenRect], next: &[ScreenRect], threshold_px: f32) -> bool {
    if previous.len() != next.len() {
        return true;
    }
    previous
        .iter()
        .zip(next)
        .any(|(old, new)| rect_delta(*old, *new) > threshold_px)
}

#[must_use]
pub fn rect_delta(old: ScreenRect, new: ScreenRect) -> f32 {
    (old.x - new.x)
        .abs()
        .max((old.y - new.y).abs())
        .max((old.w - new.w).abs())
        .max((old.h - new.h).abs())
}

/// Rate-limit helper: allow an OS update if dirty and enough time elapsed.
#[must_use]
pub fn should_apply_region(
    dirty: bool,
    last_apply: Option<std::time::Instant>,
    min_interval: std::time::Duration,
    now: std::time::Instant,
) -> bool {
    if !dirty {
        return false;
    }
    last_apply.is_none_or(|prev| now.saturating_duration_since(prev) >= min_interval)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerRoute {
    pub ui_hit: bool,
    pub vrm_hit: Option<VrmPart>,
    pub os_region_hit: bool,
    pub target: PointerTarget,
}

#[must_use]
pub fn classify_pointer(
    cursor: ScreenPoint,
    ui_regions: &[ScreenRect],
    vrm: Option<&VrmHitLayout>,
) -> PointerRoute {
    let target = route_pointer(cursor, ui_regions, vrm);
    let os = build_input_regions(ui_regions, &vrm_regions(vrm));
    PointerRoute {
        ui_hit: matches!(target, PointerTarget::Ui),
        vrm_hit: match target {
            PointerTarget::Vrm(part) => Some(part),
            PointerTarget::Ui | PointerTarget::Passthrough => None,
        },
        os_region_hit: os.iter().any(|rect| rect.contains(cursor)),
        target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{triangle_placeholder_layout, vrm_layout_from_normalized_aabb};

    fn bubble() -> ScreenRect {
        ScreenRect::new(40.0, 80.0, 280.0, 140.0)
    }

    fn menu() -> ScreenRect {
        ScreenRect::new(40.0, 230.0, 180.0, 90.0)
    }

    fn layout() -> VrmHitLayout {
        triangle_placeholder_layout((800, 600))
    }

    #[test]
    fn union_is_ui_and_vrm() {
        let vrm = layout();
        let rects = build_input_regions(&[bubble()], &vrm_regions(Some(&vrm)));
        assert!(rects.contains(&bubble()));
        assert_eq!(rects.len(), 5);
    }

    #[test]
    fn hidden_ui_drops_bubble() {
        let vrm = layout();
        let rects = build_input_regions(&[], &vrm_regions(Some(&vrm)));
        assert!(!rects.iter().any(|rect| *rect == bubble()));
        assert_eq!(rects.len(), 4);
    }

    #[test]
    fn empty_scene_is_empty_region() {
        assert!(build_input_regions(&[], &[]).is_empty());
    }

    #[test]
    fn ui_only() {
        let route = classify_pointer(ScreenPoint { x: 50.0, y: 90.0 }, &[bubble()], None);
        assert!(route.ui_hit);
        assert!(route.os_region_hit);
        assert_eq!(route.target, PointerTarget::Ui);
    }

    #[test]
    fn vrm_only() {
        let vrm = layout();
        let torso = ScreenPoint {
            x: vrm.torso.x + vrm.torso.w * 0.5,
            y: vrm.torso.y + vrm.torso.h * 0.5,
        };
        let route = classify_pointer(torso, &[], Some(&vrm));
        assert!(!route.ui_hit);
        assert!(route.vrm_hit.is_some());
        assert!(route.os_region_hit);
        assert!(matches!(route.target, PointerTarget::Vrm(_)));
    }

    #[test]
    fn overlap_prefers_ui() {
        let vrm = VrmHitLayout {
            head: ScreenRect::new(40.0, 20.0, 40.0, 40.0),
            torso: ScreenRect::new(40.0, 70.0, 80.0, 120.0),
            left_hand: ScreenRect::new(20.0, 90.0, 24.0, 24.0),
            right_hand: ScreenRect::new(100.0, 90.0, 24.0, 24.0),
        };
        let route = classify_pointer(
            ScreenPoint {
                x: bubble().x + 20.0,
                y: bubble().y + 20.0,
            },
            &[bubble()],
            Some(&vrm),
        );
        assert_eq!(route.target, PointerTarget::Ui);
        assert!(route.os_region_hit);
        assert!(bubble().contains(ScreenPoint {
            x: bubble().x + 20.0,
            y: bubble().y + 20.0,
        }));
        assert!(vrm.torso.contains(ScreenPoint {
            x: bubble().x + 20.0,
            y: bubble().y + 20.0,
        }));
    }

    #[test]
    fn background_is_passthrough_and_outside_os_region() {
        let vrm = layout();
        let route = classify_pointer(ScreenPoint { x: 8.0, y: 8.0 }, &[bubble()], Some(&vrm));
        assert_eq!(route.target, PointerTarget::Passthrough);
        assert!(!route.os_region_hit);
    }

    #[test]
    fn multiple_ui_regions() {
        let point = ScreenPoint {
            x: menu().x + 10.0,
            y: menu().y + 10.0,
        };
        let route = classify_pointer(point, &[bubble(), menu()], None);
        assert_eq!(route.target, PointerTarget::Ui);
    }

    #[test]
    fn multiple_vrm_parts_hands_win() {
        let vrm = VrmHitLayout {
            head: ScreenRect::new(100.0, 10.0, 40.0, 40.0),
            torso: ScreenRect::new(80.0, 50.0, 80.0, 120.0),
            left_hand: ScreenRect::new(70.0, 80.0, 30.0, 30.0),
            right_hand: ScreenRect::new(140.0, 80.0, 30.0, 30.0),
        };
        let route = classify_pointer(ScreenPoint { x: 80.0, y: 90.0 }, &[], Some(&vrm));
        assert_eq!(route.target, PointerTarget::Vrm(VrmPart::LeftHand));
    }

    #[test]
    fn moving_vrm_is_dirty_over_threshold() {
        let a = triangle_placeholder_layout((800, 600));
        let mut b = a;
        b.torso.x += 8.0;
        assert!(regions_dirty(
            &vrm_regions(Some(&a)),
            &vrm_regions(Some(&b)),
            2.0
        ));
        b.torso.x = a.torso.x + 1.0;
        assert!(!regions_dirty(
            &vrm_regions(Some(&a)),
            &vrm_regions(Some(&b)),
            2.0
        ));
    }

    #[test]
    fn world_to_screen_aabb_is_finite() {
        let layout =
            vrm_layout_from_normalized_aabb(([-0.3, -0.9, 0.0], [0.3, 0.9, 0.2]), (800, 600));
        for (_part, rect) in layout.parts() {
            assert!(rect.w > 0.0 && rect.h > 0.0);
            assert!(rect.x + rect.w < 900.0);
        }
    }

    #[test]
    fn rate_limit_blocks_until_interval() {
        let t0 = std::time::Instant::now();
        let interval = std::time::Duration::from_millis(50);
        assert!(should_apply_region(true, None, interval, t0));
        assert!(!should_apply_region(
            true,
            Some(t0),
            interval,
            t0 + std::time::Duration::from_millis(10)
        ));
        assert!(should_apply_region(
            true,
            Some(t0),
            interval,
            t0 + std::time::Duration::from_millis(50)
        ));
        assert!(!should_apply_region(false, None, interval, t0));
    }
}
