// Compile-time constant injected by Vite's `define` (see vite.config.ts).
// The git-derived version string, e.g. `v0.3.0-1-gf0c3ea9`. Read it through
// src/version.ts, never directly.
declare const __DARKLY_VERSION__: string;

// Which deploy flavor this build was compiled for. `'app'` only for an explicit
// `vite build --mode app` (the primary application build, seeded with a single
// black layer); every other mode resolves to `'demo'` (the decorative
// demo.darkly.art build). Read it through src/state/freshDocument.ts.
declare const __DARKLY_APP_MODE__: 'demo' | 'app';

// Operator-supplied announcement HTML, baked in by Vite's `define` from the
// `DARKLY_BANNER` environment variable (see vite.config.ts). An empty string
// when the build set none, which is the normal case for CI and desktop
// bundles. Read it through src/state/announcement.svelte.ts, never directly.
declare const __DARKLY_BANNER_HTML__: string;
