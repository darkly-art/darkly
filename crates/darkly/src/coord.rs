//! Typed coordinate spaces.
//!
//! `CanvasPoint` / `CanvasRect` live in document-canvas pixel coordinates and
//! may be negative (paste-extent layers can sit at negative canvas offsets).
//! `LayerPoint` / `LayerRect` live in a specific layer texture's local pixel
//! coordinates and are always non-negative.
//!
//! Conversion between the two requires a `LayerTexture` (or its bounds) — see
//! `crate::gpu::atlas::LayerTexture`.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CanvasPoint {
    pub x: i32,
    pub y: i32,
}

impl CanvasPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        CanvasPoint { x, y }
    }
}

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
pub struct CanvasRect {
    pub origin: CanvasPoint,
    pub width: u32,
    pub height: u32,
}

impl CanvasRect {
    pub const fn new(origin: CanvasPoint, width: u32, height: u32) -> Self {
        CanvasRect {
            origin,
            width,
            height,
        }
    }

    pub const fn from_xywh(x: i32, y: i32, width: u32, height: u32) -> Self {
        CanvasRect {
            origin: CanvasPoint::new(x, y),
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
    /// Uses `div_euclid` so the floor is correct for negative coords —
    /// `(-1).div_euclid(256) = -1`, not 0 (which is what `/` gives in Rust).
    pub fn round_outward(self, chunk: u32) -> CanvasRect {
        if self.is_empty() {
            return self;
        }
        let chunk = chunk as i32;
        let x0 = self.x0().div_euclid(chunk) * chunk;
        let y0 = self.y0().div_euclid(chunk) * chunk;
        // Ceiling outward via euclidean division on the inclusive far edge.
        let x1 = (self.x1() - 1).div_euclid(chunk) * chunk + chunk;
        let y1 = (self.y1() - 1).div_euclid(chunk) * chunk + chunk;
        CanvasRect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn contains(&self, other: CanvasRect) -> bool {
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
    pub fn union(self, other: CanvasRect) -> CanvasRect {
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
        CanvasRect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn intersect(self, other: CanvasRect) -> Option<CanvasRect> {
        let x0 = self.x0().max(other.x0());
        let y0 = self.y0().max(other.y0());
        let x1 = self.x1().min(other.x1());
        let y1 = self.y1().min(other.y1());
        if x1 > x0 && y1 > y0 {
            Some(CanvasRect::from_xywh(
                x0,
                y0,
                (x1 - x0) as u32,
                (y1 - y0) as u32,
            ))
        } else {
            None
        }
    }

    /// Axis-aligned rectangular subtraction: returns 0 to 4 rects whose union
    /// equals `self \ other`.
    pub fn subtract(self, other: Option<CanvasRect>) -> Vec<CanvasRect> {
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
            out.push(CanvasRect::from_xywh(
                self.x0(),
                self.y0(),
                self.width,
                (other.y0() - self.y0()) as u32,
            ));
        }
        // bottom strip
        if other.y1() < self.y1() {
            out.push(CanvasRect::from_xywh(
                self.x0(),
                other.y1(),
                self.width,
                (self.y1() - other.y1()) as u32,
            ));
        }
        // left strip (clipped to other's vertical extent)
        if other.x0() > self.x0() {
            out.push(CanvasRect::from_xywh(
                self.x0(),
                other.y0(),
                (other.x0() - self.x0()) as u32,
                other.height,
            ));
        }
        // right strip
        if other.x1() < self.x1() {
            out.push(CanvasRect::from_xywh(
                other.x1(),
                other.y0(),
                (self.x1() - other.x1()) as u32,
                other.height,
            ));
        }
        out
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

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
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
        // touching at edge — zero area, should be None
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
}
