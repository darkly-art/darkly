use wasm_bindgen::prelude::*;

mod api;
pub use api::DarklyHandle;

#[wasm_bindgen(start)]
pub fn init_darkly() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).ok();
    log::info!("Darkly WASM initialized");
}
