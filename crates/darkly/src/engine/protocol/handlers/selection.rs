//! Lasso selection. The other selection ops are `#[handler]`-generated on
//! `engine/filters/selection.rs`; lasso stays hand-written because its wire
//! field is `verts` (the engine param is `vertices`) and it carries a `feather`
//! field the engine ignores — neither maps cleanly to a derived `Req`.

use serde::Deserialize;

use crate::document::SelectionMode;
use crate::engine::protocol::{decode, RequestRegistration, Response};

/// Lasso polygon + selection mode. `feather` is accepted but currently ignored
/// by the engine.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct SelectLassoReq {
    pub verts: Vec<[f32; 2]>,
    pub mode: SelectionMode,
    pub antialias: bool,
    pub feather: f32,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("select_lasso", |engine, payload, _b| {
            let r: SelectLassoReq = decode(payload)?;
            engine.select_lasso(&r.verts, r.mode, r.antialias, r.feather);
            Ok(Response::empty())
        })
        .post()
        .req::<SelectLassoReq>(),
    ]
}
