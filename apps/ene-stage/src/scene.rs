//! Presentation-independent Stage scene.
//!
//! [`VisualGeometry`] is what may be drawn (including glow, shadow, padding).
//! [`InteractionGeometry`] is what may receive pointer input. They are never
//! the same type.

use glam::Vec2;

/// Axis-aligned rectangle in physical pixels, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PxRect {
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    #[must_use]
    pub fn contains(self, point: Vec2) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.w
            && point.y < self.y + self.h
    }

    #[must_use]
    pub fn padded(self, padding: f32) -> Self {
        Self {
            x: self.x - padding,
            y: self.y - padding,
            w: (self.w + padding * 2.0).max(0.0),
            h: (self.h + padding * 2.0).max(0.0),
        }
    }

    #[must_use]
    pub fn max_extent_delta(self, other: Self) -> f32 {
        (self.x - other.x)
            .abs()
            .max((self.y - other.y).abs())
            .max((self.w - other.w).abs())
            .max((self.h - other.h).abs())
    }
}

/// What the compositor may draw. Never used as an OS input region on Wayland.
#[derive(Debug, Clone, PartialEq)]
pub enum VisualPrimitive {
    AvatarBounds {
        soul_id: String,
        aabb: PxRect,
    },
    Bubble {
        id: String,
        rect: PxRect,
    },
    Effect {
        id: String,
        rect: PxRect,
        padding: f32,
    },
}

impl VisualPrimitive {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::AvatarBounds { soul_id, .. } => soul_id,
            Self::Bubble { id, .. } | Self::Effect { id, .. } => id,
        }
    }

    #[must_use]
    pub fn visual_rect(&self) -> PxRect {
        match self {
            Self::AvatarBounds { aabb, .. } | Self::Bubble { rect: aabb, .. } => *aabb,
            Self::Effect { rect, padding, .. } => rect.padded(*padding),
        }
    }
}

/// How overlay UI wants the interaction controller to treat a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiHitMode {
    DisplayOnly,
    Clickable,
    Focusable,
}

/// Coarse VRM part used for in-process picking. OS regions stay unions of AABBs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarPart {
    Body,
    Head,
    Torso,
    LeftHand,
    RightHand,
}

/// What may receive a pointer event. Display-only UI is omitted from the OS region.
#[derive(Debug, Clone, PartialEq)]
pub enum InteractionPrimitive {
    Ui {
        id: String,
        rect: PxRect,
        mode: UiHitMode,
    },
    AvatarPart {
        soul_id: String,
        part: AvatarPart,
        aabb: PxRect,
    },
}

impl InteractionPrimitive {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Ui { id, .. } => id,
            Self::AvatarPart { soul_id, .. } => soul_id,
        }
    }

    #[must_use]
    pub const fn os_input_rect(&self) -> Option<PxRect> {
        match self {
            Self::Ui {
                mode: UiHitMode::DisplayOnly,
                ..
            } => None,
            Self::Ui { rect, .. } => Some(*rect),
            Self::AvatarPart { aabb, .. } => Some(*aabb),
        }
    }

    #[must_use]
    pub const fn hit_rect(&self) -> PxRect {
        match self {
            Self::Ui { rect, .. } => *rect,
            Self::AvatarPart { aabb, .. } => *aabb,
        }
    }
}

/// Result of in-process picking. UI wins over VRM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneHit {
    Ui { id: String, mode: UiHitMode },
    Avatar { soul_id: String, part: AvatarPart },
    None,
}

/// Screen-space bone anchors. Scene stores pixels; Slint only reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageAnchorKind {
    Head,
    Chest,
    LeftHand,
    RightHand,
}

/// One projected bone in physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct StageAnchor {
    pub soul_id: String,
    pub kind: StageAnchorKind,
    pub position: Vec2,
    pub offscreen: bool,
    pub behind_camera: bool,
}

/// Toolkit- and OS-independent Stage scene.
#[derive(Debug, Clone, Default)]
pub struct StageScene {
    visuals: Vec<VisualPrimitive>,
    interactions: Vec<InteractionPrimitive>,
    anchors: Vec<StageAnchor>,
    hidden: Vec<String>,
    dirty: bool,
}

impl StageScene {
    pub fn set_anchors(&mut self, anchors: Vec<StageAnchor>) {
        if self.anchors != anchors {
            self.anchors = anchors;
            self.dirty = true;
        }
    }

    #[must_use]
    pub fn anchors(&self) -> &[StageAnchor] {
        &self.anchors
    }
}

impl StageScene {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_visuals(&mut self, visuals: Vec<VisualPrimitive>) {
        if self.visuals != visuals {
            self.visuals = visuals;
            self.dirty = true;
        }
    }

    pub fn set_interactions(&mut self, interactions: Vec<InteractionPrimitive>) {
        if self.interactions != interactions {
            self.interactions = interactions;
            self.dirty = true;
        }
    }

    pub fn hide(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.hidden.iter().any(|existing| existing == &id) {
            self.hidden.push(id);
            self.dirty = true;
        }
    }

    pub fn show(&mut self, id: &str) {
        let before = self.hidden.len();
        self.hidden.retain(|existing| existing != id);
        if self.hidden.len() != before {
            self.dirty = true;
        }
    }

    #[must_use]
    pub fn overlay_ui_flags(&self) -> (bool, bool) {
        let mut clickable = false;
        let mut focusable = false;
        for prim in &self.interactions {
            if self.is_hidden(prim.id()) {
                continue;
            }
            if let InteractionPrimitive::Ui { mode, .. } = prim {
                match mode {
                    UiHitMode::Focusable => focusable = true,
                    UiHitMode::Clickable => clickable = true,
                    UiHitMode::DisplayOnly => {}
                }
            }
        }
        (clickable, focusable)
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }

    #[must_use]
    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|existing| existing == id)
    }

    /// Drawn footprint, including effect padding. Hidden ids are omitted.
    #[must_use]
    pub fn visual_geometry(&self) -> VisualGeometry {
        VisualGeometry {
            rects: self
                .visuals
                .iter()
                .filter(|prim| !self.is_hidden(prim.id()))
                .map(VisualPrimitive::visual_rect)
                .filter(|rect| !rect.is_empty())
                .collect(),
        }
    }

    /// Pointer footprint sent to OS backends. Display-only and hidden ids are omitted.
    #[must_use]
    pub fn interaction_geometry(&self) -> InteractionGeometry {
        InteractionGeometry {
            rects: self
                .interactions
                .iter()
                .filter(|prim| !self.is_hidden(prim.id()))
                .filter_map(InteractionPrimitive::os_input_rect)
                .filter(|rect| !rect.is_empty())
                .collect(),
        }
    }

    #[must_use]
    pub fn hit(&self, point: Vec2) -> SceneHit {
        for prim in self.interactions.iter().rev() {
            if self.is_hidden(prim.id()) {
                continue;
            }
            if !prim.hit_rect().contains(point) {
                continue;
            }
            return match prim {
                InteractionPrimitive::Ui { id, mode, .. } => SceneHit::Ui {
                    id: id.clone(),
                    mode: *mode,
                },
                InteractionPrimitive::AvatarPart { soul_id, part, .. } => SceneHit::Avatar {
                    soul_id: soul_id.clone(),
                    part: *part,
                },
            };
        }
        SceneHit::None
    }
}

/// Drawn rectangles after hiding and padding.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VisualGeometry {
    pub rects: Vec<PxRect>,
}

/// Input rectangles after hiding display-only primitives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractionGeometry {
    pub rects: Vec<PxRect>,
}

impl InteractionGeometry {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// True when every rect moved less than `threshold` px vs `previous`.
    #[must_use]
    pub fn within_threshold(&self, previous: &Self, threshold: f32) -> bool {
        if self.rects.len() != previous.rects.len() {
            return false;
        }
        self.rects
            .iter()
            .zip(&previous.rects)
            .all(|(now, was)| now.max_extent_delta(*was) < threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avatar(soul: &str, rect: PxRect) -> InteractionPrimitive {
        InteractionPrimitive::AvatarPart {
            soul_id: soul.to_owned(),
            part: AvatarPart::Body,
            aabb: rect,
        }
    }

    #[test]
    fn visual_only_effect_does_not_enter_interaction_geometry() {
        let mut scene = StageScene::new();
        scene.set_visuals(vec![VisualPrimitive::Effect {
            id: "glow".to_owned(),
            rect: PxRect::new(10.0, 10.0, 20.0, 20.0),
            padding: 8.0,
        }]);
        scene.set_interactions(vec![avatar("a", PxRect::new(0.0, 0.0, 10.0, 10.0))]);
        let visual = scene.visual_geometry();
        assert_eq!(visual.rects[0], PxRect::new(2.0, 2.0, 36.0, 36.0));
        assert_eq!(scene.interaction_geometry().rects.len(), 1);
    }

    #[test]
    fn hidden_ui_drops_from_both_geometries() {
        let mut scene = StageScene::new();
        scene.set_visuals(vec![VisualPrimitive::Bubble {
            id: "bubble".to_owned(),
            rect: PxRect::new(0.0, 0.0, 40.0, 20.0),
        }]);
        scene.set_interactions(vec![InteractionPrimitive::Ui {
            id: "bubble".to_owned(),
            rect: PxRect::new(0.0, 0.0, 40.0, 20.0),
            mode: UiHitMode::Clickable,
        }]);
        scene.hide("bubble");
        assert!(scene.visual_geometry().rects.is_empty());
        assert!(scene.interaction_geometry().is_empty());
        assert_eq!(scene.hit(Vec2::new(5.0, 5.0)), SceneHit::None);
    }

    #[test]
    fn display_only_ui_is_visual_but_not_os_input() {
        let mut scene = StageScene::new();
        scene.set_visuals(vec![VisualPrimitive::Bubble {
            id: "speech".to_owned(),
            rect: PxRect::new(0.0, 0.0, 40.0, 20.0),
        }]);
        scene.set_interactions(vec![InteractionPrimitive::Ui {
            id: "speech".to_owned(),
            rect: PxRect::new(0.0, 0.0, 40.0, 20.0),
            mode: UiHitMode::DisplayOnly,
        }]);
        assert_eq!(scene.visual_geometry().rects.len(), 1);
        assert!(scene.interaction_geometry().is_empty());
        assert_eq!(
            scene.hit(Vec2::new(5.0, 5.0)),
            SceneHit::Ui {
                id: "speech".to_owned(),
                mode: UiHitMode::DisplayOnly,
            }
        );
    }

    #[test]
    fn ui_wins_over_overlapping_avatar() {
        let mut scene = StageScene::new();
        scene.set_interactions(vec![
            avatar("a", PxRect::new(0.0, 0.0, 100.0, 100.0)),
            InteractionPrimitive::Ui {
                id: "btn".to_owned(),
                rect: PxRect::new(10.0, 10.0, 20.0, 20.0),
                mode: UiHitMode::Clickable,
            },
        ]);
        assert_eq!(
            scene.hit(Vec2::new(15.0, 15.0)),
            SceneHit::Ui {
                id: "btn".to_owned(),
                mode: UiHitMode::Clickable,
            }
        );
        assert_eq!(
            scene.hit(Vec2::new(80.0, 80.0)),
            SceneHit::Avatar {
                soul_id: "a".to_owned(),
                part: AvatarPart::Body,
            }
        );
    }

    #[test]
    fn empty_geometry_hits_none() {
        let scene = StageScene::new();
        assert!(scene.visual_geometry().rects.is_empty());
        assert!(scene.interaction_geometry().is_empty());
        assert_eq!(scene.hit(Vec2::ZERO), SceneHit::None);
    }

    #[test]
    fn moving_rects_mark_dirty_and_threshold() {
        let mut scene = StageScene::new();
        scene.set_interactions(vec![avatar("a", PxRect::new(0.0, 0.0, 10.0, 10.0))]);
        assert!(scene.take_dirty());
        assert!(!scene.take_dirty());
        let previous = scene.interaction_geometry();
        scene.set_interactions(vec![avatar("a", PxRect::new(1.0, 0.0, 10.0, 10.0))]);
        let now = scene.interaction_geometry();
        assert!(now.within_threshold(&previous, 2.0));
        assert!(!now.within_threshold(&previous, 0.5));
        assert!(scene.take_dirty());
    }
}
