/**
 * Darkly's version — the single frontend home for it. The raw
 * `git describe --tags --long` string injected by Vite as `__DARKLY_VERSION__`
 * (see vite.config.ts), e.g. `v0.3.0-2-g1eabe67`. Shown verbatim in the About
 * modal so it exactly matches the version the Rust crate stamps into saved
 * files (crates/darkly/src/lib.rs `VERSION`) — one copyable, round-trippable
 * string, no decorative characters.
 *
 * `typeof` guard so importing this module never throws even if `define` isn't
 * applied in some context — the value is replaced inline at build/test time.
 */
export const darklyVersion =
    typeof __DARKLY_VERSION__ === 'string' ? __DARKLY_VERSION__ : '0.0.0-0-gunknown';
