//! WASM bridge for the Krita brush inspector.
//!
//! Exposes a [`KritaInspector`] handle: parse a `.kpp` byte buffer once, then
//! pull JSON metadata and individual resource byte arrays as needed. Keeping
//! the parsed state on the Rust side lets the frontend fetch `Uint8Array`
//! resource bytes directly (for `URL.createObjectURL`) without round-tripping
//! base64 through JSON.

use darkly::brush::import::krita::kpp::ParamDecoded;
use darkly::brush::import::krita::{parse_kpp, KritaPreset};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct KritaInspector {
    preset: KritaPreset,
}

#[wasm_bindgen]
impl KritaInspector {
    /// Parse a `.kpp` byte buffer. Returns the inspector handle on success.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<KritaInspector, JsError> {
        let preset = parse_kpp(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Self { preset })
    }

    /// Serialized parsed preset as a JSON string. Resource bytes are not
    /// included — call [`Self::resource_bytes`] for those.
    pub fn metadata(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.preset).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Raw bytes of the embedded resource at `index`. Returns the bytes as
    /// a `Uint8Array` so the frontend can wrap them in a `Blob`.
    pub fn resource_bytes(&self, index: usize) -> Result<Vec<u8>, JsError> {
        self.preset
            .resources
            .get(index)
            .map(|r| r.bytes.clone())
            .ok_or_else(|| JsError::new(&format!("resource index {index} out of range")))
    }

    /// Number of embedded resources. Convenience for the frontend.
    pub fn resource_count(&self) -> usize {
        self.preset.resources.len()
    }

    /// Raw bytes of an inline-image param (e.g. `Texture/Pattern/Pattern`),
    /// looked up by its index in `preset.params`. Returns the bytes as a
    /// `Uint8Array` so the frontend can wrap them in a `Blob` for `<img>`
    /// display.
    pub fn param_image_bytes(&self, index: usize) -> Result<Vec<u8>, JsError> {
        match self.preset.params.get(index).map(|p| &p.decoded) {
            Some(ParamDecoded::EmbeddedImage { bytes, .. }) => Ok(bytes.clone()),
            Some(_) => Err(JsError::new(&format!(
                "param at index {index} is not an embedded image"
            ))),
            None => Err(JsError::new(&format!("param index {index} out of range"))),
        }
    }
}
