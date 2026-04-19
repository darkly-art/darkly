//! Pen input data and stroke vector storage.
//!
//! `PaintInformation` captures everything the tablet reports for a single
//! sample.  `StrokeRecord` accumulates these as raw vectors for the
//! duration of a stroke — enabling re-rendering with different parameters.

use serde::{Deserialize, Serialize};

/// All sensor data for a single pen sample.
///
/// Modelled after Krita's `KisPaintInformation` — every field the tablet
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
    /// Cumulative distance travelled in pixels (not normalised — used for
    /// spacing calculations, normalised on demand by sensor nodes).
    pub distance: f32,
    /// Drawing angle in radians (direction of pen travel, 0 = right).
    pub drawing_angle: f32,
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

impl PaintInformation {
    /// Synthetic pen input for dry-run previews.
    /// Neutral tablet state with mid-pressure so pressure-driven sensors
    /// show the brush at a typical firm stroke.
    pub fn preview_dummy() -> Self {
        Self {
            pressure: 0.5,
            ..Default::default()
        }
    }
}

/// A complete vector record of a stroke, retained for re-rendering.
///
/// Stores raw pre-smoothing events so the stroke can be replayed with
/// different smoothing, dynamics, or brush parameters.  Discarded on
/// the next user action (Darkly is raster — layer pixels are truth).
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
