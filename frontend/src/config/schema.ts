// Config schema and defaults live in Rust (crates/darkly/src/config.rs).
// This file defines JS-only types for the preset system.

export interface Preset {
    name: string;
    description: string;
}
