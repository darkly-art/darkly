//! Unit regression for `PaintInformation::derive_sensors`. The cursor
//! preview path (`brush_graph::regenerate_brush_cursor_preview_with_pen`)
//! calls this every pointer event with the chord between the previous
//! and current hover positions. Coalesced events can land at the same
//! coordinates, which used to make `atan2(0, 0) = 0` snap drawing_angle
//! to 0 for that frame, flickering the hover cursor between the real
//! direction and 0° on alternating events.
//!
//! `derive_sensors` preserves the previous sample's `drawing_angle` when
//! the chord is below `COINCIDENT_PX`, through `direction_between`, the
//! direction rule the stroke engine measures its dabs with too. These tests
//! pin that behavior.

use darkly::brush::paint_info::{direction_between, PaintInformation, COINCIDENT_PX};

/// The one direction rule: the chord's direction, or the fallback when the
/// two points coincide.
#[test]
fn direction_between_measures_the_chord_or_falls_back() {
    use std::f32::consts::FRAC_PI_2;
    assert!((direction_between([0.0, 0.0], [0.0, 5.0], 1.0) - FRAC_PI_2).abs() < 1e-6);
    assert!(
        (direction_between([3.0, 3.0], [1.0, 3.0], 0.0).abs() - std::f32::consts::PI).abs() < 1e-6
    );
    assert_eq!(direction_between([2.0, 2.0], [2.0, 2.0], 0.7), 0.7);
    assert_eq!(
        direction_between([2.0, 2.0], [2.0 + COINCIDENT_PX / 2.0, 2.0], 0.7),
        0.7,
        "a move under the coincidence floor has no direction to measure"
    );
}

#[test]
fn zero_delta_preserves_drawing_angle() {
    let mut prev = PaintInformation {
        pos: [10.0, 10.0],
        ..Default::default()
    };
    prev.drawing_angle = 0.7;

    let mut cur = PaintInformation {
        pos: prev.pos, // identical position, chord = 0
        ..Default::default()
    };
    cur.derive_sensors(Some(&prev), 0.0);

    assert_eq!(
        cur.drawing_angle, prev.drawing_angle,
        "zero-delta segment must preserve prev's drawing_angle, not \
         snap to atan2(0, 0) = 0",
    );
}

#[test]
fn real_motion_overwrites_drawing_angle() {
    let mut prev = PaintInformation {
        pos: [10.0, 10.0],
        ..Default::default()
    };
    prev.drawing_angle = 0.7;

    let mut cur = PaintInformation {
        pos: [20.0, 10.0], // +x motion → drawing_angle should be 0
        ..Default::default()
    };
    cur.derive_sensors(Some(&prev), 10.0);

    assert!(
        cur.drawing_angle.abs() < 1.0e-5,
        "rightward motion should set drawing_angle to ~0, got {}",
        cur.drawing_angle,
    );
}

#[test]
fn first_point_leaves_drawing_angle_at_default() {
    let mut cur = PaintInformation {
        pos: [10.0, 10.0],
        ..Default::default()
    };
    cur.derive_sensors(None, 0.0);

    assert_eq!(
        cur.drawing_angle, 0.0,
        "first point has no segment, so drawing_angle stays at its default 0",
    );
}
