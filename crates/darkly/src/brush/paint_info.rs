//! Pen input data and stroke vector storage.
//!
//! `PaintInformation` captures everything the tablet reports for a single
//! sample.  `StrokeRecord` accumulates these as raw vectors for the
//! duration of a stroke, enabling re-rendering with different parameters.

use serde::{Deserialize, Serialize};

/// Per-axis [`PaintInformation::motion`] magnitude, in canvas pixels, below
/// which a dab has not moved from the previous one: there is no "one dab
/// ago" to read, so a reader of the stroke at `-motion` (the live canvas
/// sampler) contributes nothing for it.
pub const STATIONARY_MOTION_PX: f32 = 0.5;

/// Distance, in canvas pixels, under which two samples coincide: there is no
/// travel between them to measure a direction or place dabs along.
pub const COINCIDENT_PX: f32 = 1.0e-3;

/// Direction of travel from `from` to `to` in canvas radians (0 = right), or
/// `fallback` when the two coincide and there is no travel to measure.
///
/// The one direction rule: a hover sample measures it against the hover
/// sample before it, a stroke dab against the dab before it, and a stroke's
/// first dab against the hover the pen arrived from, so the hover preview and
/// the stroke it turns into face the same way.
///
/// After Krita's `KisAlgebra2D::directionBetweenPoints`, by Dmitry Kazakov
/// (<https://invent.kde.org/graphics/krita/-/blob/master/libs/global/kis_algebra_2d.cpp>).
pub fn direction_between(from: [f32; 2], to: [f32; 2], fallback: f32) -> f32 {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    if dx.hypot(dy) > COINCIDENT_PX {
        dy.atan2(dx)
    } else {
        fallback
    }
}

/// All sensor data for a single pen sample.
///
/// Modelled after Krita's `KisPaintInformation`: every field the tablet
/// can provide, plus derived values computed by the stroke engine.
///
/// All values are normalised to 0-1 unless noted otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaintInformation {
    // ── Raw tablet data (0-1 normalised) ────────────────────────────
    /// Canvas-space position in pixels.
    pub pos: [f32; 2],
    /// Pen pressure (0 = no contact, 1 = max).
    pub pressure: f32,
    /// X-axis tilt, normalised from hardware range to 0-1.
    pub x_tilt: f32,
    /// Y-axis tilt, normalised from hardware range to 0-1.
    pub y_tilt: f32,
    /// Barrel rotation (0-1 maps to 0-360°).
    pub rotation: f32,
    /// Tangential pressure (airbrush wheel), 0-1.
    pub tangential_pressure: f32,
    /// Timestamp in seconds since stroke start.
    pub time: f32,

    // ── Derived values (computed by stroke engine) ──────────────────
    /// Pen speed in pixels/second, normalised to 0-1 via a reference max.
    pub speed: f32,
    /// Cumulative distance travelled in pixels (not normalised; used for
    /// spacing calculations, normalised on demand by sensor nodes).
    pub distance: f32,
    /// Drawing angle in radians (0 = right), measured by [`direction_between`].
    /// `derive_sensors` measures it between consecutive samples, which is what
    /// the hover preview shows. A stroke dab measures its own in
    /// `StrokeEngine::place_dab`, against the dab before it, and replaces the
    /// vertex value with the stamp orientation: the stroke's undirected axis,
    /// approached no faster than the brush's turn rate.
    pub drawing_angle: f32,
    /// Per-dab motion vector in canvas pixels: the position delta from the
    /// previous *emitted dab* into this one. Populated by the stroke engine
    /// at dab-emission time (`StrokeEngine::place_dab`); interpolators and
    /// `derive_sensors` leave it zero because at their layer there is no
    /// previous-dab to delta against. Smudge depends on per-dab magnitude
    /// (the smear sample offset = previous dab position); liquify uses
    /// `drawing_angle` only and is indifferent to magnitude.
    pub motion: [f32; 2],
    /// Combined tilt magnitude (0-1), derived from x_tilt and y_tilt.
    pub tilt_magnitude: f32,
    /// Tilt direction in radians.
    pub tilt_direction: f32,
    /// Index of this sample within the current stroke (0-based).
    pub index: u32,

    /// Fade sensor (0-1): normalized distance along the stroke, 0 at start,
    /// 1 at the configured fade length.  Clamps to 1 beyond the fade distance.
    pub fade: f32,
}

/// Speed normalization cutoff: `speed_px_per_sec / MAX_SPEED_PX_PER_SEC`
/// clamped to [0, 1] gives the `speed` sensor value.
pub const MAX_SPEED_PX_PER_SEC: f32 = 4000.0;

impl PaintInformation {
    /// Synthetic pen input for dry-run previews.
    /// Full pressure so pressure-driven sensors show the brush at its
    /// fully-engaged tip: the silhouette the artist expects when previewing
    /// "what this brush looks like."
    pub fn cursor_preview_dummy() -> Self {
        Self {
            pressure: 1.0,
            ..Default::default()
        }
    }

    /// Fill derived sensor fields from this sample's raw data and an
    /// optional previous sample. Used by both the stroke engine and the
    /// hover preview so a compiled graph sees the same sensor values
    /// regardless of which path is driving it.
    ///
    /// Tilt-derived sensors (`tilt_magnitude`, `tilt_direction`) always
    /// fill; they depend only on this sample.
    ///
    /// Segment-derived sensors (`drawing_angle`, `distance`, `speed`) fill
    /// only when `prev` is present. `segment_length` is the chord length
    /// between prev and this sample.
    /// `motion` is NOT filled here; it's a per-dab quantity owned by
    /// `StrokeEngine::place_dab`.
    pub fn derive_sensors(&mut self, prev: Option<&Self>, segment_length: f32) {
        self.tilt_magnitude = (self.x_tilt * self.x_tilt + self.y_tilt * self.y_tilt)
            .sqrt()
            .min(1.0);
        self.tilt_direction = self.y_tilt.atan2(self.x_tilt);

        let Some(prev) = prev else {
            return;
        };

        // Coalesced pointer events at the same coords keep prev's direction
        // rather than snapping to `atan2(0, 0) = 0`, which would flicker the
        // cursor preview between the real direction and 0.
        self.drawing_angle = direction_between(prev.pos, self.pos, prev.drawing_angle);
        self.distance = prev.distance + segment_length;

        let dt = self.time - prev.time;
        if dt > 0.0 {
            let speed_px_per_sec = segment_length / dt;
            self.speed = (speed_px_per_sec / MAX_SPEED_PX_PER_SEC).min(1.0);
        } else {
            self.speed = prev.speed;
        }
    }
}

/// A complete vector record of a stroke, retained for re-rendering.
///
/// Stores raw pre-smoothing events so the stroke can be replayed with
/// different smoothing, dynamics, or brush parameters.  Discarded on
/// the next artist action (Darkly is raster; layer pixels are truth).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StrokeRecord {
    /// Raw pen events in chronological order (pre-smoothing).
    pub events: Vec<PaintInformation>,
    /// Foreground color at stroke start (linear RGBA).
    pub color: [f32; 4],
    /// Identifier of the brush graph used for this stroke.
    pub brush_graph_id: String,
}

impl StrokeRecord {
    pub fn new(color: [f32; 4], brush_graph_id: String) -> Self {
        Self {
            events: Vec::with_capacity(256),
            color,
            brush_graph_id,
        }
    }

    /// Append a raw pen event.
    pub fn push(&mut self, info: PaintInformation) {
        self.events.push(info);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
