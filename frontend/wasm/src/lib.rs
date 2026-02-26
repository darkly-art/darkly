use wasm_bindgen::prelude::*;

pub mod api;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Info).ok();
    log::info!("Darkly WASM initialized");
}
