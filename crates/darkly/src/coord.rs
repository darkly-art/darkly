//! Typed coordinate spaces.
//!
//! `CanvasPoint` / `CanvasRect` live in **plane** coordinates, the absolute
//! document frame that does not move on crop. They may be negative (paste-extent
//! layers can sit at negative canvas offsets).
//!
//! `WindowPoint` / `WindowRect` live in **window-local** coordinates, origin at
//! the canvas-window top-left, i.e. plane `canvas_origin`. The window-sized
//! selection texture and floating-preview texture live here. Convert to plane
//! with `to_canvas(canvas_origin)` (the invariant is
//! `plane = window_local + canvas_origin`).
//!
//! `LayerPoint` / `LayerRect` live in a specific layer texture's local pixel
//! coordinates and are always non-negative. Conversion to plane requires a
//! `LayerTexture` (or its bounds); see `crate::gpu::atlas::LayerTexture`.
//!
//! `Point<S>` / `Rect<S>` are generic over a zero-size coordinate-space tag
//! `S` ([`Plane`] / [`Window`]). The space-agnostic rect/point algebra is
//! written once here; `CanvasPoint`/`CanvasRect`/`WindowPoint`/`WindowRect` are
//! type aliases. `Rect<Plane>` and `Rect<Window>` are distinct, incompatible
//! types; mixing frames is a compile error, and the only space-specific code
//! is the `to_canvas` / `to_window` conversions on the concrete instantiations.

use std::marker::PhantomData;

/// Coordinate-space tag for the **plane** frame, the absolute document frame
/// that does not move on crop. May be negative.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Plane;

/// Coordinate-space tag for the **window-local** frame, origin at the
/// canvas-window top-left (plane `canvas_origin`). The window-sized selection
/// and floating-preview textures are indexed in this frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Window;

/// A point in coordinate space `S`. See the module docs and [`Plane`] /
/// [`Window`] for the frames. The `serde(bound)` drops the auto-added
/// `S: Serialize/Deserialize` bound, and `serde(skip)` on the tag keeps the
/// emitted JSON identical to a bare `{ x, y }`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "", deserialize = ""))]
pub struct Point<S> {
    pub x: i32,
    pub y: i32,
    #[serde(skip)]
    _space: PhantomData<S>,
}

impl<S> Point<S> {
    pub const fn new(x: i32, y: i32) -> Self {
        Point {
            x,
            y,
            _space: PhantomData,
        }
    }
}

/// An axis-aligned rect in coordinate space `S`. Carries the full space-agnostic
/// rect algebra; the plane/window conversions live on the concrete `Rect<Plane>`
/// / `Rect<Window>` instantiations below.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(bound(serialize = "", deserialize = ""))]
pub struct Rect<S> {
    pub origin: Point<S>,
    pub width: u32,
    pub height: u32,
}

impl<S: Copy> Rect<S> {
    pub const fn new(origin: Point<S>, width: u32, height: u32) -> Self {
        Rect {
            origin,
            width,
            height,
        }
    }

    pub const fn from_xywh(x: i32, y: i32, width: u32, height: u32) -> Self {
        Rect {
            origin: Point::new(x, y),
            width,
            height,
        }
    }

    pub fn x0(&self) -> i32 {
        self.origin.x
    }
    pub fn y0(&self) -> i32 {
        self.origin.y
    }
    pub fn x1(&self) -> i32 {
        self.origin.x + self.width as i32
    }
    pub fn y1(&self) -> i32 {
        self.origin.y + self.height as i32
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Round the rect's edges outward to a multiple of `chunk` pixels.
    /// Origin floors toward more-negative; far edge ceils toward more-positive.
    /// Uses `div_euclid` so the floor is correct for negative coords:
    /// `(-1).div_euclid(256) = -1`, not 0 (which is what `/` gives in Rust).
    pub fn round_outward(self, chunk: u32) -> Rect<S> {
        if self.is_empty() {
            return self;
        }
        let chunk = chunk as i32;
        let x0 = self.x0().div_euclid(chunk) * chunk;
        let y0 = self.y0().div_euclid(chunk) * chunk;
        // Ceiling outward via euclidean division on the inclusive far edge.
        let x1 = (self.x1() - 1).div_euclid(chunk) * chunk + chunk;
        let y1 = (self.y1() - 1).div_euclid(chunk) * chunk + chunk;
        Rect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn contains(&self, other: Rect<S>) -> bool {
        if other.is_empty() {
            return true;
        }
        if self.is_empty() {
            return false;
        }
        other.x0() >= self.x0()
            && other.y0() >= self.y0()
            && other.x1() <= self.x1()
            && other.y1() <= self.y1()
    }

    /// Smallest rect containing both. Empty rects are ignored (an empty `self`
    /// returns `other` and vice versa).
    pub fn union(self, other: Rect<S>) -> Rect<S> {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x0().min(other.x0());
        let y0 = self.y0().min(other.y0());
        let x1 = self.x1().max(other.x1());
        let y1 = self.y1().max(other.y1());
        Rect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn intersect(self, other: Rect<S>) -> Option<Rect<S>> {
        let x0 = self.x0().max(other.x0());
        let y0 = self.y0().max(other.y0());
        let x1 = self.x1().min(other.x1());
        let y1 = self.y1().min(other.y1());
        if x1 > x0 && y1 > y0 {
            Some(Rect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
        } else {
            None
        }
    }

    /// Build a rect from two corners. A reversed or off-extent axis yields
    /// extent 0 (an empty rect), never an underflow; this is the single home
    /// for the "clamp a bounding box and difference the edges" arithmetic that
    /// callers must not re-roll by hand. Inputs must be finite (callers casting
    /// from `f32` must clamp `NaN`/∞ first); `saturating_sub` keeps even extreme
    /// `i32` corners safe in debug.
    pub fn from_corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect<S> {
        Rect::from_xywh(
            x0,
            y0,
            x1.saturating_sub(x0).max(0) as u32,
            y1.saturating_sub(y0).max(0) as u32,
        )
    }

    /// Snap a float AABB outward to integer pixels (floor the near edge, ceil
    /// the far edge) and intersect with `self` (the valid extent). Returns the
    /// tight integer sub-rect, or `None` if they don't overlap. The single home
    /// for the float-footprint "clamp to extent" boilerplate. Inputs must be
    /// finite.
    pub fn clamp_f32(self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Rect<S>> {
        Rect::from_corners(
            x0.floor() as i32,
            y0.floor() as i32,
            x1.ceil() as i32,
            y1.ceil() as i32,
        )
        .intersect(self)
    }

    /// Axis-aligned rectangular subtraction: returns 0 to 4 rects whose union
    /// equals `self \ other`.
    pub fn subtract(self, other: Option<Rect<S>>) -> Vec<Rect<S>> {
        if self.is_empty() {
            return Vec::new();
        }
        let other = match other.and_then(|o| self.intersect(o)) {
            Some(o) => o,
            None => return vec![self],
        };
        let mut out = Vec::with_capacity(4);
        // top strip
        if other.y0() > self.y0() {
            out.push(Rect::from_xywh(
                self.x0(),
                self.y0(),
                self.width,
                (other.y0() - self.y0()) as u32,
            ));
        }
        // bottom strip
        if other.y1() < self.y1() {
            out.push(Rect::from_xywh(
                self.x0(),
                other.y1(),
                self.width,
                (self.y1() - other.y1()) as u32,
            ));
        }
        // left strip (clipped to other's vertical extent)
        if other.x0() > self.x0() {
            out.push(Rect::from_xywh(
                self.x0(),
                other.y0(),
                (other.x0() - self.x0()) as u32,
                other.height,
            ));
        }
        // right strip
        if other.x1() < self.x1() {
            out.push(Rect::from_xywh(
                other.x1(),
                other.y0(),
                (self.x1() - other.x1()) as u32,
                other.height,
            ));
        }
        out
    }
}

impl WindowPoint {
    /// Lift this window-local point into plane coordinates.
    pub fn to_canvas(self, canvas_origin: CanvasPoint) -> CanvasPoint {
        CanvasPoint::new(self.x + canvas_origin.x, self.y + canvas_origin.y)
    }
}

impl CanvasPoint {
    /// Express this plane point in window-local coordinates anchored at
    /// `canvas_origin`. Inverse of [`WindowPoint::to_canvas`].
    pub fn to_window(self, canvas_origin: CanvasPoint) -> WindowPoint {
        WindowPoint::new(self.x - canvas_origin.x, self.y - canvas_origin.y)
    }
}

impl WindowRect {
    /// Lift this window-local rect into plane coordinates anchored at
    /// `canvas_origin`. Inverse of [`CanvasRect::to_window`].
    pub fn to_canvas(self, canvas_origin: CanvasPoint) -> CanvasRect {
        CanvasRect::new(
            self.origin.to_canvas(canvas_origin),
            self.width,
            self.height,
        )
    }
}

impl CanvasRect {
    /// Express this plane rect in window-local coordinates anchored at
    /// `canvas_origin`. Inverse of [`WindowRect::to_canvas`].
    pub fn to_window(self, canvas_origin: CanvasPoint) -> WindowRect {
        WindowRect::new(
            self.origin.to_window(canvas_origin),
            self.width,
            self.height,
        )
    }
}

// On the wire a `Point`/`Rect` is a plain `{ x, y }` / `{ origin, width, height }`
// regardless of its phantom coordinate space, so to the typed TS client they are
// inline object expressions, not declared, named types (which would collide
// across `S` instantiations that share a TS shape). The space marker erases at
// the boundary, mirroring its `#[serde(skip)]` `PhantomData`.
#[cfg(feature = "ts-export")]
impl<S: 'static> ts_rs::TS for Point<S> {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(_: &ts_rs::Config) -> String {
        "{ x: number, y: number }".to_string()
    }
    fn inline(cfg: &ts_rs::Config) -> String {
        <Self as ts_rs::TS>::name(cfg)
    }
}

#[cfg(feature = "ts-export")]
impl<S: 'static> ts_rs::TS for Rect<S> {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(cfg: &ts_rs::Config) -> String {
        format!(
            "{{ origin: {}, width: number, height: number }}",
            <Point<S> as ts_rs::TS>::name(cfg)
        )
    }
    fn inline(cfg: &ts_rs::Config) -> String {
        <Self as ts_rs::TS>::name(cfg)
    }
}

/// A point in the **plane** frame (see [`Plane`]).
pub type CanvasPoint = Point<Plane>;
/// A rect in the **plane** frame (see [`Plane`]).
pub type CanvasRect = Rect<Plane>;
/// A point in the **window-local** frame (see [`Window`]).
pub type WindowPoint = Point<Window>;
/// A rect in the **window-local** frame (see [`Window`]).
pub type WindowRect = Rect<Window>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayerPoint {
    pub x: u32,
    pub y: u32,
}

impl LayerPoint {
    pub const fn new(x: u32, y: u32) -> Self {
        LayerPoint { x, y }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayerRect {
    pub origin: LayerPoint,
    pub width: u32,
    pub height: u32,
}

impl LayerRect {
    pub const fn new(origin: LayerPoint, width: u32, height: u32) -> Self {
        LayerRect {
            origin,
            width,
            height,
        }
    }

    pub const fn from_xywh(x: u32, y: u32, width: u32, height: u32) -> Self {
        LayerRect {
            origin: LayerPoint::new(x, y),
            width,
            height,
        }
    }

    pub fn x0(&self) -> u32 {
        self.origin.x
    }
    pub fn y0(&self) -> u32 {
        self.origin.y
    }
    pub fn x1(&self) -> u32 {
        self.origin.x + self.width
    }
    pub fn y1(&self) -> u32 {
        self.origin.y + self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// True when `other`'s pixels are all inside `self`. An empty `other` is
    /// vacuously contained; an empty `self` contains only an empty `other`.
    pub fn contains(&self, other: LayerRect) -> bool {
        if other.is_empty() {
            return true;
        }
        if self.is_empty() {
            return false;
        }
        other.x0() >= self.x0()
            && other.y0() >= self.y0()
            && other.x1() <= self.x1()
            && other.y1() <= self.y1()
    }

    pub fn intersect(self, other: LayerRect) -> Option<LayerRect> {
        let x0 = self.x0().max(other.x0());
        let y0 = self.y0().max(other.y0());
        let x1 = self.x1().min(other.x1());
        let y1 = self.y1().min(other.y1());
        if x1 > x0 && y1 > y0 {
            Some(LayerRect::from_xywh(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
        CanvasRect::from_xywh(x, y, w, h)
    }

    #[test]
    fn union_with_empty_is_identity() {
        let a = r(10, 10, 20, 20);
        let empty = r(0, 0, 0, 0);
        assert_eq!(a.union(empty), a);
        assert_eq!(empty.union(a), a);
    }

    #[test]
    fn union_disjoint_extends() {
        let a = r(0, 0, 10, 10);
        let b = r(20, 20, 10, 10);
        assert_eq!(a.union(b), r(0, 0, 30, 30));
    }

    #[test]
    fn intersect_disjoint_is_none() {
        assert_eq!(r(0, 0, 10, 10).intersect(r(20, 20, 10, 10)), None);
    }

    #[test]
    fn intersect_touching_is_none() {
        // touching at edge: zero area, should be None
        assert_eq!(r(0, 0, 10, 10).intersect(r(10, 0, 10, 10)), None);
    }

    #[test]
    fn intersect_overlap() {
        let i = r(0, 0, 10, 10).intersect(r(5, 5, 10, 10)).unwrap();
        assert_eq!(i, r(5, 5, 5, 5));
    }

    #[test]
    fn intersect_contained() {
        let i = r(0, 0, 100, 100).intersect(r(20, 30, 5, 5)).unwrap();
        assert_eq!(i, r(20, 30, 5, 5));
    }

    #[test]
    fn from_corners_normal() {
        assert_eq!(CanvasRect::from_corners(5, 10, 15, 30), r(5, 10, 10, 20));
    }

    #[test]
    fn from_corners_reversed_is_empty() {
        // Reversed corners on either axis collapse to extent 0, not underflow.
        let rect = CanvasRect::from_corners(15, 30, 5, 10);
        assert!(rect.is_empty());
        assert_eq!(rect, r(15, 30, 0, 0));
    }

    #[test]
    fn from_corners_extreme_i32_no_overflow() {
        // A far-off-extent corner (e.g. from a saturating f32 cast) must not
        // panic; the reversed axis simply yields 0.
        let rect = CanvasRect::from_corners(i32::MAX, 0, i32::MIN, 10);
        assert!(rect.is_empty());
    }

    #[test]
    fn clamp_f32_disjoint_is_none() {
        let extent = r(0, 0, 100, 100);
        assert_eq!(extent.clamp_f32(500.0, 500.0, 550.0, 550.0), None);
        assert_eq!(extent.clamp_f32(-50.0, -50.0, -10.0, -10.0), None);
    }

    #[test]
    fn clamp_f32_partial_clamps_to_extent() {
        let extent = r(0, 0, 100, 100);
        // Straddles the bottom-right corner; rounds outward then clips to extent.
        let i = extent.clamp_f32(80.5, 80.5, 130.0, 130.0).unwrap();
        assert_eq!(i, r(80, 80, 20, 20));
    }

    #[test]
    fn clamp_f32_inside_rounds_outward() {
        let extent = r(0, 0, 100, 100);
        // Fully inside: floor near edge, ceil far edge.
        let i = extent.clamp_f32(10.2, 10.8, 20.1, 20.9).unwrap();
        assert_eq!(i, r(10, 10, 11, 11));
    }

    #[test]
    fn subtract_none_returns_self() {
        let a = r(0, 0, 10, 10);
        assert_eq!(a.subtract(None), vec![a]);
    }

    #[test]
    fn subtract_disjoint_returns_self() {
        let a = r(0, 0, 10, 10);
        assert_eq!(a.subtract(Some(r(20, 20, 5, 5))), vec![a]);
    }

    #[test]
    fn subtract_identical_returns_empty() {
        let a = r(0, 0, 10, 10);
        assert!(a.subtract(Some(a)).is_empty());
    }

    #[test]
    fn subtract_contained_returns_four_strips() {
        let a = r(0, 0, 100, 100);
        let b = r(40, 40, 20, 20);
        let parts = a.subtract(Some(b));
        assert_eq!(parts.len(), 4);
        // Combined area should equal a's area minus b's area.
        let total_area: u64 = parts.iter().map(|r| r.width as u64 * r.height as u64).sum();
        let expected = a.width as u64 * a.height as u64 - b.width as u64 * b.height as u64;
        assert_eq!(total_area, expected);
    }

    #[test]
    fn subtract_corner_overlap() {
        let a = r(0, 0, 10, 10);
        // b covers top-left corner; expect 2 rects (right strip + bottom strip)
        let parts = a.subtract(Some(r(-5, -5, 10, 10)));
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn contains_empty_other_is_true() {
        assert!(r(10, 10, 5, 5).contains(r(0, 0, 0, 0)));
    }

    #[test]
    fn contains_self_in_self_is_true() {
        let a = r(0, 0, 10, 10);
        assert!(a.contains(a));
    }

    #[test]
    fn contains_partial_overlap_is_false() {
        assert!(!r(0, 0, 10, 10).contains(r(5, 5, 10, 10)));
    }

    #[test]
    fn negative_offset_round_trip() {
        let a = r(-256, -256, 512, 512);
        assert_eq!(a.x0(), -256);
        assert_eq!(a.x1(), 256);
        assert_eq!(a.union(a), a);
    }

    // ------------------------------------------------------------------
    // round_outward
    // ------------------------------------------------------------------

    #[test]
    fn round_outward_already_aligned_is_identity() {
        let a = r(0, 0, 256, 256);
        assert_eq!(a.round_outward(256), a);
    }

    #[test]
    fn round_outward_grows_far_edge_to_chunk() {
        // 1px on the far side rounds the whole side up.
        let a = r(0, 0, 257, 1);
        assert_eq!(a.round_outward(256), r(0, 0, 512, 256));
    }

    #[test]
    fn bounds_align_to_256_handles_negative_canvas_coords() {
        // Plan-named regression: `(-1, -1, 1, 1)` must round outward to
        // origin (-256, -256), not (0, 0). This is the div_euclid trap.
        let a = r(-1, -1, 1, 1);
        let r256 = a.round_outward(256);
        assert_eq!(r256.x0(), -256);
        assert_eq!(r256.y0(), -256);
        assert_eq!(r256.width, 256);
        assert_eq!(r256.height, 256);
    }

    #[test]
    fn round_outward_negative_origin_just_inside_alignment() {
        // x0=-257 → floors to -512; x1=-1 → ceils to 0.
        let a = r(-257, -257, 256, 256);
        let r256 = a.round_outward(256);
        assert_eq!(r256.x0(), -512);
        assert_eq!(r256.y0(), -512);
        assert_eq!(r256.x1(), 0);
        assert_eq!(r256.y1(), 0);
    }

    #[test]
    fn round_outward_preserves_empty() {
        let a = r(10, 10, 0, 5);
        assert_eq!(a.round_outward(256), a);
    }

    // ------------------------------------------------------------------
    // LayerRect: texture-local rect helpers
    // ------------------------------------------------------------------

    fn lr(x: u32, y: u32, w: u32, h: u32) -> LayerRect {
        LayerRect::from_xywh(x, y, w, h)
    }

    #[test]
    fn layer_rect_edges() {
        let a = lr(10, 20, 30, 40);
        assert_eq!(a.x0(), 10);
        assert_eq!(a.y0(), 20);
        assert_eq!(a.x1(), 40);
        assert_eq!(a.y1(), 60);
    }

    #[test]
    fn layer_rect_contains_self_and_empty() {
        let a = lr(0, 0, 10, 10);
        assert!(a.contains(a));
        assert!(a.contains(lr(0, 0, 0, 0)));
    }

    #[test]
    fn layer_rect_contains_subrect() {
        assert!(lr(0, 0, 100, 100).contains(lr(40, 40, 20, 20)));
    }

    #[test]
    fn layer_rect_contains_rejects_partial_overlap() {
        assert!(!lr(0, 0, 10, 10).contains(lr(5, 5, 10, 10)));
    }

    #[test]
    fn layer_rect_contains_rejects_disjoint() {
        assert!(!lr(0, 0, 10, 10).contains(lr(20, 20, 5, 5)));
    }

    #[test]
    fn layer_rect_intersect_disjoint_is_none() {
        assert_eq!(lr(0, 0, 10, 10).intersect(lr(20, 20, 5, 5)), None);
    }

    #[test]
    fn layer_rect_intersect_touching_is_none() {
        assert_eq!(lr(0, 0, 10, 10).intersect(lr(10, 0, 10, 10)), None);
    }

    #[test]
    fn layer_rect_intersect_overlap() {
        let i = lr(0, 0, 10, 10).intersect(lr(5, 5, 10, 10)).unwrap();
        assert_eq!(i, lr(5, 5, 5, 5));
    }

    // ------------------------------------------------------------------
    // WindowRect ↔ CanvasRect: plane = window_local + canvas_origin
    // ------------------------------------------------------------------

    #[test]
    fn window_to_canvas_round_trips() {
        let origin = CanvasPoint::new(40, 15);
        let w = WindowRect::from_xywh(3, 7, 20, 12);
        let plane = w.to_canvas(origin);
        assert_eq!(plane, CanvasRect::from_xywh(43, 22, 20, 12));
        // round-trip back to window-local recovers the original
        assert_eq!(plane.to_window(origin), w);
    }

    #[test]
    fn window_to_canvas_zero_origin_is_identity() {
        // Until the first crop `canvas_origin == (0, 0)` and the two frames
        // coincide, which is exactly why origin bugs hide in fresh docs.
        let origin = CanvasPoint::new(0, 0);
        let w = WindowRect::from_xywh(5, 9, 11, 13);
        assert_eq!(w.to_canvas(origin), CanvasRect::from_xywh(5, 9, 11, 13));
    }

    #[test]
    fn window_point_to_canvas_shifts_by_origin() {
        let origin = CanvasPoint::new(-12, 30);
        let p = WindowPoint::new(8, 4);
        assert_eq!(p.to_canvas(origin), CanvasPoint::new(-4, 34));
    }

    #[test]
    fn window_rect_intersect_and_union() {
        let a = WindowRect::from_xywh(0, 0, 10, 10);
        let b = WindowRect::from_xywh(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Some(WindowRect::from_xywh(5, 5, 5, 5)));
        assert_eq!(a.union(b), WindowRect::from_xywh(0, 0, 15, 15));
    }

    #[test]
    fn canvas_point_to_window_is_inverse_of_to_canvas() {
        let origin = CanvasPoint::new(40, -15);
        let p = CanvasPoint::new(43, -8);
        let w = p.to_window(origin);
        assert_eq!(w, WindowPoint::new(3, 7));
        assert_eq!(w.to_canvas(origin), p);
    }

    #[test]
    fn window_rect_shares_generic_algebra() {
        // contains/round_outward are now available on the window-local frame
        // too (they came for free from the generic `Rect<S>`).
        let outer = WindowRect::from_xywh(0, 0, 100, 100);
        assert!(outer.contains(WindowRect::from_xywh(10, 10, 5, 5)));
        assert_eq!(
            WindowRect::from_xywh(0, 0, 257, 1).round_outward(256),
            WindowRect::from_xywh(0, 0, 512, 256)
        );
    }
}
