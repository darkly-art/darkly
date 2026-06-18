//! Modifier (mask) operations.

use crate::engine::protocol::{layer_id, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "add_mask",
            handle: |engine, payload, _b| {
                engine.add_mask(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "remove_mask",
            handle: |engine, payload, _b| {
                engine.remove_mask(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "apply_mask",
            handle: |engine, payload, _b| {
                engine.apply_mask(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "selection_to_mask",
            handle: |engine, payload, _b| {
                engine.selection_to_mask(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "mask_to_selection",
            handle: |engine, payload, _b| {
                engine.mask_to_selection(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
    ]
}
