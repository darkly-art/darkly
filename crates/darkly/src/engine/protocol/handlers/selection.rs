//! Lasso selection. The other selection ops are `#[handler]`-generated on
//! `engine/filters/selection.rs`; lasso stays hand-written because its wire
//! field is `verts` (the engine param is `vertices`) and it carries a `feather`
//! field the engine ignores — neither maps cleanly to a derived `Req`.

use serde::Deserialize;

use crate::document::SelectionMode;
use crate::engine::protocol::{decode, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
        kind: "select_lasso",
        handle: |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                verts: Vec<[f32; 2]>,
                mode: SelectionMode,
                antialias: bool,
                feather: f32,
            }
            let r: Req = decode(payload)?;
            engine.select_lasso(&r.verts, r.mode, r.antialias, r.feather);
            Ok(Response::empty())
        },
    }]
}
