// Which deploy flavor this build was compiled for. `'app'` only for an explicit
// `vite build --mode app` (the primary application build, seeded with a single
// black layer); every other mode resolves to `'demo'` (the decorative
// demo.darkly.art build). Read it through src/state/freshDocument.ts.
declare const __DARKLY_APP_MODE__: 'demo' | 'app';
