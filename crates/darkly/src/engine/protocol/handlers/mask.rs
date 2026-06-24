//! Modifier (mask) operations.

use crate::engine::protocol::{layer_id, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("add_mask", |engine, payload, _b| {
            engine.add_mask(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("remove_mask", |engine, payload, _b| {
            engine.remove_mask(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("apply_mask", |engine, payload, _b| {
            engine.apply_mask(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("selection_to_mask", |engine, payload, _b| {
            engine.selection_to_mask(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("mask_to_selection", |engine, payload, _b| {
            engine.mask_to_selection(layer_id(payload)?);
            Ok(Response::empty())
        }),
    ]
}
