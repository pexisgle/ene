//! Pure pointer routing: UI → VRM parts → desktop passthrough.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmPart {
    Head,
    Torso,
    LeftHand,
    RightHand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerTarget {
    Ui,
    Vrm(VrmPart),
    Passthrough,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ScreenRect {
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.w
            && point.y < self.y + self.h
            && self.w > 0.0
            && self.h > 0.0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    #[must_use]
    pub fn to_i32(self) -> Option<(i32, i32, i32, i32)> {
        if self.is_empty() {
            return None;
        }
        Some((
            round_i32(self.x),
            round_i32(self.y),
            round_i32(self.w),
            round_i32(self.h),
        ))
    }
}

/// Coarse screen-space colliders for a single body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VrmHitLayout {
    pub head: ScreenRect,
    pub torso: ScreenRect,
    pub left_hand: ScreenRect,
    pub right_hand: ScreenRect,
}

impl VrmHitLayout {
    #[must_use]
    pub fn hit(self, point: ScreenPoint) -> Option<VrmPart> {
        // Hands overlap the torso silhouette in a T-pose; they win so
        // "click the hand" is distinguishable from a torso tap.
        if self.left_hand.contains(point) {
            return Some(VrmPart::LeftHand);
        }
        if self.right_hand.contains(point) {
            return Some(VrmPart::RightHand);
        }
        if self.head.contains(point) {
            return Some(VrmPart::Head);
        }
        if self.torso.contains(point) {
            return Some(VrmPart::Torso);
        }
        None
    }

    #[must_use]
    pub fn parts(self) -> [(VrmPart, ScreenRect); 4] {
        [
            (VrmPart::Head, self.head),
            (VrmPart::Torso, self.torso),
            (VrmPart::LeftHand, self.left_hand),
            (VrmPart::RightHand, self.right_hand),
        ]
    }
}

/// UI wins overlaps; VRM is next; everything else is passthrough.
#[must_use]
pub fn route_pointer(
    cursor: ScreenPoint,
    ui_regions: &[ScreenRect],
    vrm: Option<&VrmHitLayout>,
) -> PointerTarget {
    if ui_regions.iter().any(|rect| rect.contains(cursor)) {
        return PointerTarget::Ui;
    }
    if let Some(part) = vrm.and_then(|layout| layout.hit(cursor)) {
        return PointerTarget::Vrm(part);
    }
    PointerTarget::Passthrough
}

/// OS-level input region is the union of interactive UI and VRM rects.
#[must_use]
pub fn interactive_rects(ui_regions: &[ScreenRect], vrm: Option<&VrmHitLayout>) -> Vec<ScreenRect> {
    let mut out = Vec::new();
    out.extend(ui_regions.iter().copied().filter(|rect| !rect.is_empty()));
    if let Some(layout) = vrm {
        for (_part, rect) in layout.parts() {
            if !rect.is_empty() {
                out.push(rect);
            }
        }
    }
    out
}

#[must_use]
pub fn is_passthrough_region(target: PointerTarget) -> bool {
    matches!(target, PointerTarget::Passthrough)
}

/// Split a normalized-space AABB into head / torso / hands in pixels.
///
/// The character is assumed to occupy the middle of the overlay, matching
/// the orthographic auto-fit used by `ene-vrm`.
#[must_use]
pub fn vrm_layout_from_normalized_aabb(
    aabb: ([f32; 3], [f32; 3]),
    viewport: (u32, u32),
) -> VrmHitLayout {
    let (min, max) = aabb;
    let width = px(viewport.0);
    let height = px(viewport.1);
    let aspect = (width / height).max(0.0001);
    let world_h = 2.6_f32;
    let world_w = world_h * aspect;
    let to_px = |x: f32, y: f32| -> ScreenPoint {
        ScreenPoint {
            x: (x / world_w + 0.5) * width,
            y: (0.5 - y / world_h) * height,
        }
    };
    let top_left = to_px(min[0], max[1]);
    let bottom_right = to_px(max[0], min[1]);
    let x = top_left.x.min(bottom_right.x);
    let y = top_left.y.min(bottom_right.y);
    let w = (top_left.x - bottom_right.x).abs().max(1.0);
    let h = (top_left.y - bottom_right.y).abs().max(1.0);
    split_body_rect(ScreenRect::new(x, y, w, h))
}

/// Placeholder layout used when only the colored triangle is drawn.
#[must_use]
pub fn triangle_placeholder_layout(viewport: (u32, u32)) -> VrmHitLayout {
    let width = px(viewport.0);
    let height = px(viewport.1);
    let body = ScreenRect::new(width * 0.32, height * 0.18, width * 0.36, height * 0.64);
    split_body_rect(body)
}

#[must_use]
fn px(value: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "swapchain pixels are well inside f32"
    )]
    {
        value.max(1) as f32
    }
}

fn round_i32(value: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "input regions are pixel rectangles"
    )]
    {
        value.round() as i32
    }
}

#[must_use]
pub fn split_body_rect(body: ScreenRect) -> VrmHitLayout {
    let head_h = body.h * 0.22;
    let hand_w = body.w * 0.22;
    let hand_h = body.h * 0.16;
    VrmHitLayout {
        head: ScreenRect::new(body.x + body.w * 0.25, body.y, body.w * 0.5, head_h),
        torso: ScreenRect::new(
            body.x + body.w * 0.18,
            body.y + head_h,
            body.w * 0.64,
            body.h * 0.55,
        ),
        left_hand: ScreenRect::new(body.x, body.y + body.h * 0.38, hand_w, hand_h),
        right_hand: ScreenRect::new(
            body.x + body.w - hand_w,
            body.y + body.h * 0.38,
            hand_w,
            hand_h,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui_bubble() -> ScreenRect {
        ScreenRect::new(40.0, 80.0, 280.0, 140.0)
    }

    fn layout() -> VrmHitLayout {
        triangle_placeholder_layout((800, 600))
    }

    #[test]
    fn bubble_click_is_ui_even_over_vrm() {
        let vrm = layout();
        let overlap = ScreenPoint {
            x: ui_bubble().x + 10.0,
            y: ui_bubble().y + 10.0,
        };
        assert_eq!(
            route_pointer(overlap, &[ui_bubble()], Some(&vrm)),
            PointerTarget::Ui
        );
    }

    #[test]
    fn character_click_is_vrm() {
        let vrm = layout();
        let torso = ScreenPoint {
            x: vrm.torso.x + vrm.torso.w * 0.5,
            y: vrm.torso.y + vrm.torso.h * 0.5,
        };
        let target = route_pointer(torso, &[ui_bubble()], Some(&vrm));
        assert!(
            matches!(target, PointerTarget::Vrm(VrmPart::Torso | VrmPart::Head)),
            "expected VRM body, got {target:?}"
        );
    }

    #[test]
    fn transparent_click_is_passthrough() {
        let vrm = layout();
        let miss = ScreenPoint { x: 10.0, y: 10.0 };
        assert_eq!(
            route_pointer(miss, &[ui_bubble()], Some(&vrm)),
            PointerTarget::Passthrough
        );
        assert!(is_passthrough_region(PointerTarget::Passthrough));
    }

    #[test]
    fn hand_beats_torso_on_overlap() {
        let vrm = VrmHitLayout {
            head: ScreenRect::new(100.0, 10.0, 40.0, 40.0),
            torso: ScreenRect::new(80.0, 50.0, 80.0, 120.0),
            left_hand: ScreenRect::new(70.0, 80.0, 30.0, 30.0),
            right_hand: ScreenRect::new(140.0, 80.0, 30.0, 30.0),
        };
        let left = ScreenPoint { x: 80.0, y: 90.0 };
        assert_eq!(vrm.hit(left), Some(VrmPart::LeftHand));
        assert_eq!(
            route_pointer(left, &[], Some(&vrm)),
            PointerTarget::Vrm(VrmPart::LeftHand)
        );
    }

    #[test]
    fn interactive_rects_are_union_of_ui_and_vrm() {
        let vrm = layout();
        let rects = interactive_rects(&[ui_bubble()], Some(&vrm));
        assert!(rects.iter().any(|rect| *rect == ui_bubble()));
        assert!(rects.len() >= 5);
        assert!(rects.iter().all(|rect| !rect.is_empty()));
    }

    #[test]
    fn empty_ui_and_vrm_means_full_passthrough() {
        assert!(interactive_rects(&[], None).is_empty());
        assert_eq!(
            route_pointer(ScreenPoint { x: 1.0, y: 1.0 }, &[], None),
            PointerTarget::Passthrough
        );
    }

    #[test]
    fn aabb_layout_stays_inside_viewport() {
        let layout =
            vrm_layout_from_normalized_aabb(([-0.4, -0.8, -0.1], [0.4, 0.8, 0.1]), (1000, 800));
        for (_part, rect) in layout.parts() {
            assert!(rect.x >= -1.0);
            assert!(rect.y >= -1.0);
            assert!(rect.w > 0.0 && rect.h > 0.0);
            assert!(rect.x + rect.w <= 1001.0);
            assert!(rect.y + rect.h <= 801.0);
        }
    }
}
