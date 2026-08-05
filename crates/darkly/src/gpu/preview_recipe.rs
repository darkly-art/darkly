//! How a previewable variant's preview moves.
//!
//! A recipe is pure declaration — a frame count, a rate, and a set of keyframe
//! tracks over a normalized timeline. A renderer evaluates every track at
//! `t = i / frames` for frame `i`, applies the resulting values, and renders.
//! The loop is the same for every variant, so a renderer never learns what it
//! is animating.
//!
//! Motion is therefore something a type declares about itself, in its own file,
//! beside its parameter schema — see `gpu/veils/vhs.rs` or `gpu/filters/hsv.rs`
//! for worked examples.

use super::params::{ConstParamValue, ParamDef, ParamValue};

/// How a previewable variant's preview moves: `frames` images at `fps`, with
/// every track evaluated at `t = i / frames`.
pub struct PreviewRecipe {
    /// Images in the sequence. Conventionally [`ANIMATED_FRAMES`].
    ///
    /// [`ANIMATED_FRAMES`]: super::preview::ANIMATED_FRAMES
    pub frames: u32,
    /// Playback rate. Conventionally [`PREVIEW_FPS`].
    ///
    /// [`PREVIEW_FPS`]: super::preview::PREVIEW_FPS
    pub fps: u32,
    /// Channels that move. Tracks run concurrently; a variant with two things
    /// worth showing declares two tracks, not two recipes.
    pub tracks: &'static [Track],
}

/// One animated channel.
pub struct Track {
    pub target: TrackTarget,
    /// Keyframes in ascending `t`, the first at `t == 0.0`. Values between
    /// keyframes are blended by [`ParamDef::lerp`]; after the last, the last
    /// keyframe holds.
    pub keys: &'static [Key],
}

/// What a track drives. Three disjoint namespaces, distinguished by the variant
/// rather than by a name lookup that could resolve in either — so a track that
/// names something its host cannot drive is a declaration error, not a silently
/// discarded value.
pub enum TrackTarget {
    /// A parameter in the variant's own `&'static [ParamDef]` schema. Resolved
    /// against that schema and nothing else.
    Param(&'static str),
    /// A knob owned by the layer hosting the preview, not by the variant — a
    /// document property such as opacity. Resolved against the host catalog's
    /// [`PreviewSpec::layer_knobs`], which is empty for catalogs rendered
    /// offscreen.
    Layer(&'static str),
    /// The effect's own time basis, in elapsed seconds. A renderer feeds
    /// consecutive differences to `update_time`, so a variant whose motion is
    /// time-driven (a tape wobble, rain running down glass) declares it in the
    /// same vocabulary as a parameter sweep.
    Time,
}

pub struct Key {
    /// Position on the normalized timeline, `0.0 ..= 1.0`.
    pub t: f32,
    pub value: ConstParamValue,
}

/// A recipe together with the knob namespace its host catalog exposes. A recipe
/// never travels without it, so no consumer has to ask which catalog it came
/// from to know what a [`TrackTarget::Layer`] may name.
#[derive(Clone, Copy)]
pub struct PreviewSpec {
    pub recipe: &'static PreviewRecipe,
    /// Names a [`TrackTarget::Layer`] may address. Empty for catalogs whose
    /// previews have no compositing layer.
    pub layer_knobs: &'static [ParamDef],
}

/// A [`TrackTarget::Layer`] name that the host catalog does not expose.
#[derive(Debug, PartialEq)]
pub struct UnknownLayerKnob(pub &'static str);

impl std::fmt::Display for UnknownLayerKnob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no layer knob named `{}` in this catalog", self.0)
    }
}

impl std::error::Error for UnknownLayerKnob {}

/// The host layer's opacity, expressed in the same schema vocabulary as an
/// effect parameter so keyframes, interpolation and the loop rule are one code
/// path for `opacity` and for `hue` alike.
pub const OPACITY: ParamDef = ParamDef::float("opacity", 0.0, 1.0, 1.0)
    .with_label("Opacity")
    .with_description("How strongly the effect's layer shows through what is beneath it.");

/// The keys bracketing `t`, and the normalized position between them. Before
/// the first key and after the last, that key holds — which is what lets a
/// track cover part of the timeline without declaring the rest.
fn bracket(keys: &[Key], t: f32) -> Option<(&Key, &Key, f32)> {
    let first = keys.first()?;
    let last = keys.last()?;
    if t <= first.t {
        return Some((first, first, 0.0));
    }
    if t >= last.t {
        return Some((last, last, 1.0));
    }
    let hi = keys.iter().position(|k| k.t > t)?;
    let (a, b) = (&keys[hi - 1], &keys[hi]);
    let span = b.t - a.t;
    let u = if span > 0.0 { (t - a.t) / span } else { 1.0 };
    Some((a, b, u))
}

/// Evaluate one track against the def that owns it: lift both bracketing
/// keyframes through the schema, then blend them the way that schema says its
/// kind blends.
fn value_at(def: &ParamDef, keys: &[Key], t: f32) -> Option<ParamValue> {
    let (a, b, u) = bracket(keys, t)?;
    Some(def.lerp(
        &def.value_from_const(a.value),
        &def.value_from_const(b.value),
        u,
    ))
}

impl PreviewRecipe {
    /// Concrete values for `defs` at `t` — exactly `defs.len()` values,
    /// positionally aligned with the schema, which is what `update_filter_params`,
    /// `Void::update_params` and every `pack_uniform` require. Reads
    /// [`TrackTarget::Param`] tracks only, resolved by name against `defs`;
    /// every parameter no track names takes its schema default.
    pub fn params_at(&self, defs: &[ParamDef], t: f32) -> Vec<ParamValue> {
        defs.iter()
            .map(|def| {
                self.tracks
                    .iter()
                    .find(|tr| matches!(tr.target, TrackTarget::Param(n) if n == def.name))
                    .and_then(|tr| value_at(def, tr.keys, t))
                    .unwrap_or_else(|| def.default_value())
            })
            .collect()
    }

    /// Host-layer knob values at `t` — only the knobs a track drives, as
    /// `(name, value)` pairs for a renderer to apply. Reads
    /// [`TrackTarget::Layer`] tracks only, resolved by name against `knobs`. A
    /// name absent from `knobs` is [`UnknownLayerKnob`]: the caller's catalog
    /// cannot drive it, and rendering fails rather than silently producing a
    /// still.
    pub fn layer_at(
        &self,
        knobs: &[ParamDef],
        t: f32,
    ) -> Result<Vec<(&'static str, ParamValue)>, UnknownLayerKnob> {
        self.tracks
            .iter()
            .filter_map(|tr| match tr.target {
                TrackTarget::Layer(name) => Some((name, tr)),
                _ => None,
            })
            .map(|(name, tr)| {
                let def = knobs
                    .iter()
                    .find(|d| d.name == name)
                    .ok_or(UnknownLayerKnob(name))?;
                Ok((
                    name,
                    value_at(def, tr.keys, t).unwrap_or_else(|| def.default_value()),
                ))
            })
            .collect()
    }

    /// Elapsed seconds at `t` from the [`TrackTarget::Time`] track, or `0.0`
    /// when the recipe declares none.
    pub fn time_at(&self, t: f32) -> f32 {
        let seconds = |k: &Key| match k.value {
            ConstParamValue::Float(s) => s,
            ConstParamValue::Int(s) => s as f32,
            _ => 0.0,
        };
        self.tracks
            .iter()
            .find(|tr| matches!(tr.target, TrackTarget::Time))
            .and_then(|tr| bracket(tr.keys, t))
            .map(|(a, b, u)| {
                let (a, b) = (seconds(a), seconds(b));
                a + (b - a) * u
            })
            .unwrap_or(0.0)
    }

    /// Whether the last frame hands back to the first without a jump: true
    /// exactly when every track's last keyframe value equals its first.
    ///
    /// Exact for [`TrackTarget::Param`] and [`TrackTarget::Layer`] tracks,
    /// whose values are written rather than accumulated. A
    /// [`TrackTarget::Time`] track is fed to `update_time` as a *delta*, so
    /// returning to its first key returns the effect to its first state only on
    /// veils that integrate `dt` — one that advances a noise index per call
    /// regardless of `dt` would cut hard at the wrap.
    pub fn loops(&self) -> bool {
        self.tracks
            .iter()
            .all(|tr| match (tr.keys.first(), tr.keys.last()) {
                (Some(a), Some(b)) => a.value == b.value,
                _ => true,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFS: &[ParamDef] = &[
        ParamDef::float("hue", -180.0, 180.0, 0.0),
        ParamDef::float("value", -100.0, 100.0, 0.0),
        ParamDef::boolean("colorize", false),
    ];

    static HUE_SWING: PreviewRecipe = PreviewRecipe {
        frames: 48,
        fps: 24,
        tracks: &[Track {
            target: TrackTarget::Param("hue"),
            keys: &[
                Key {
                    t: 0.0,
                    value: ConstParamValue::Float(0.0),
                },
                Key {
                    t: 0.5,
                    value: ConstParamValue::Float(180.0),
                },
                Key {
                    t: 1.0,
                    value: ConstParamValue::Float(0.0),
                },
            ],
        }],
    };

    /// A recipe drives the parameters it names and leaves every other one at
    /// its schema default, and the vector it returns is exactly `defs.len()`
    /// long and positionally aligned — the contract `update_filter_params` and
    /// every `pack_uniform` index into.
    #[test]
    fn params_at_returns_schema_defaults_for_untracked_parameters() {
        let vals = HUE_SWING.params_at(DEFS, 0.5);
        assert_eq!(vals.len(), DEFS.len());
        assert_eq!(vals[0], ParamValue::Float(180.0));
        assert_eq!(vals[1], DEFS[1].default_value());
        assert_eq!(vals[2], ParamValue::Bool(false));
    }

    /// Between two keyframes the value equals neither of them. Without this a
    /// recipe could silently step and still satisfy every "the frames differ"
    /// assertion downstream, shipping a slideshow.
    #[test]
    fn params_at_interpolates_between_keyframes() {
        let quarter = HUE_SWING.params_at(DEFS, 0.25);
        assert_eq!(quarter[0], ParamValue::Float(90.0));

        // Monotone across the rising half, and back down across the falling one.
        let hue = |t: f32| match HUE_SWING.params_at(DEFS, t)[0] {
            ParamValue::Float(f) => f,
            ref other => panic!("expected a float, got {other:?}"),
        };
        for i in 0..24 {
            let (a, b) = (hue(i as f32 / 48.0), hue((i + 1) as f32 / 48.0));
            assert!(a < b, "hue fell on the rising half at frame {i}");
        }
        for i in 24..47 {
            let (a, b) = (hue(i as f32 / 48.0), hue((i + 1) as f32 / 48.0));
            assert!(a > b, "hue rose on the falling half at frame {i}");
        }

        // A track holds its last keyframe past the end of its own span.
        assert_eq!(hue(2.0), 0.0);
    }

    /// The two namespaces are disjoint: `params_at` sees only `Param` tracks,
    /// `layer_at` only `Layer` ones. A `Layer` name the host catalog does not
    /// expose fails at the point of use rather than being evaluated and thrown
    /// away — which is the difference between a broken asset and a loud error.
    #[test]
    fn layer_at_reads_only_layer_tracks_and_rejects_unknown_knobs() {
        static MIXED: PreviewRecipe = PreviewRecipe {
            frames: 48,
            fps: 24,
            tracks: &[
                Track {
                    target: TrackTarget::Param("hue"),
                    keys: &[Key {
                        t: 0.0,
                        value: ConstParamValue::Float(90.0),
                    }],
                },
                Track {
                    target: TrackTarget::Layer("opacity"),
                    keys: &[
                        Key {
                            t: 0.0,
                            value: ConstParamValue::Float(0.0),
                        },
                        Key {
                            t: 1.0,
                            value: ConstParamValue::Float(1.0),
                        },
                    ],
                },
            ],
        };
        const KNOBS: &[ParamDef] = &[OPACITY];

        // `params_at` ignores the layer track entirely: it returns one value
        // per def, and `opacity` is not one of them.
        let params = MIXED.params_at(DEFS, 0.5);
        assert_eq!(params.len(), DEFS.len());
        assert_eq!(params[0], ParamValue::Float(90.0));

        // `layer_at` ignores the param track and reads only the knob.
        let knobs = MIXED.layer_at(KNOBS, 0.5).expect("opacity is a layer knob");
        assert_eq!(knobs, vec![("opacity", ParamValue::Float(0.5))]);

        // A catalog that exposes no knobs cannot drive one.
        assert_eq!(MIXED.layer_at(&[], 0.5), Err(UnknownLayerKnob("opacity")));

        // A recipe with no layer track resolves to nothing against any knob set.
        assert_eq!(HUE_SWING.layer_at(KNOBS, 0.5), Ok(Vec::new()));
    }

    /// `time_at` reads the time track and nothing else, and answers zero for a
    /// recipe that declares none.
    #[test]
    fn time_at_reads_the_time_track() {
        static CLOCK: PreviewRecipe = PreviewRecipe {
            frames: 48,
            fps: 24,
            tracks: &[Track {
                target: TrackTarget::Time,
                keys: &[
                    Key {
                        t: 0.0,
                        value: ConstParamValue::Float(0.0),
                    },
                    Key {
                        t: 1.0,
                        value: ConstParamValue::Float(2.0),
                    },
                ],
            }],
        };

        assert_eq!(CLOCK.time_at(0.0), 0.0);
        assert_eq!(CLOCK.time_at(0.5), 1.0);
        assert_eq!(CLOCK.time_at(1.0), 2.0);
        for i in 0..48 {
            let (a, b) = (
                CLOCK.time_at(i as f32 / 48.0),
                CLOCK.time_at((i + 1) as f32 / 48.0),
            );
            assert!(b > a, "the clock ran backwards at frame {i}");
        }

        assert_eq!(HUE_SWING.time_at(0.5), 0.0);
    }

    /// `loop` in the asset index is computed from the recipe, never authored:
    /// an author who returns every track to where it started gets a seamless
    /// loop, and one who does not gets an honest `false`.
    #[test]
    fn loops_is_true_exactly_when_every_track_returns_to_its_first_key() {
        assert!(HUE_SWING.loops());

        static ONE_WAY: PreviewRecipe = PreviewRecipe {
            frames: 48,
            fps: 24,
            tracks: &[
                Track {
                    target: TrackTarget::Param("hue"),
                    keys: &[
                        Key {
                            t: 0.0,
                            value: ConstParamValue::Float(0.0),
                        },
                        Key {
                            t: 1.0,
                            value: ConstParamValue::Float(0.0),
                        },
                    ],
                },
                Track {
                    target: TrackTarget::Param("value"),
                    keys: &[
                        Key {
                            t: 0.0,
                            value: ConstParamValue::Float(0.0),
                        },
                        Key {
                            t: 1.0,
                            value: ConstParamValue::Float(60.0),
                        },
                    ],
                },
            ],
        };
        assert!(!ONE_WAY.loops(), "one track that does not return breaks it");
    }
}
