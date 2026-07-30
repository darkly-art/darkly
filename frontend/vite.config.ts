// vite.config runs under esbuild/node at build time and is outside the
// `tsc --noEmit` scope (tsconfig only includes src/**), so we don't pull
// @types/node into the project just to type one Node builtin here.
// @ts-ignore — Node builtin; no @types/node dependency.
import { execSync } from 'node:child_process';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import basicSsl from '@vitejs/plugin-basic-ssl';
import { VitePWA } from 'vite-plugin-pwa';
// @ts-ignore — plain .mjs build tooling, outside the tsc src scope.
import { iconBundlePlugin } from './scripts/gen-icon-bundle.mjs';

// Darkly's version is the latest git tag plus the commit height since it — the
// same v* tags the deploy pipeline (darkly-deploy/) builds releases from.
// `--long` always emits `TAG-COMMITS-gSHA` (height 0 when HEAD *is* the tag).
// No `--always`: on a tagless/shallow checkout we want describe to THROW so the
// catch yields the parseable fallback, not a bare SHA. Surfaced to the app as
// the `__DARKLY_VERSION__` compile-time constant; parsed in src/version.ts.
//
// CANONICAL TWIN: crates/darkly/build.rs bakes the Rust crate's version with the
// identical command and identical "0.0.0-0-gunknown" fallback (a documented DRY
// exception — Cargo and Vite share no runtime). Change one, change the other.
function gitVersion(): string {
    try {
        return execSync('git describe --tags --long', { encoding: 'utf8' }).trim();
    } catch {
        return '0.0.0-0-gunknown';
    }
}

export default defineConfig(({ mode }) => ({
    // Relative asset paths so the same dist/ works when served from a web root
    // ("/") and when loaded via file:// from a packaged desktop bundle.
    base: './',
    define: {
        __DARKLY_VERSION__: JSON.stringify(gitVersion()),
        // Deploy flavor: 'app' only for an explicit `vite build --mode app`.
        // Every other mode (production default, dev server, --mode demo) stays
        // 'demo', so `npm run dev` keeps the decorative demo experience.
        __DARKLY_APP_MODE__: JSON.stringify(mode === 'app' ? 'app' : 'demo'),
    },
    resolve: {
        // Component tests run in jsdom and must load Svelte's browser runtime;
        // production browser builds use the same condition.
        conditions: ['browser'],
    },
    plugins: [
        // Regenerates src/icons/bundle.generated.ts from the icon names found in
        // source — on buildStart (dev + prod) and live on file change in dev.
        iconBundlePlugin(),
        svelte(),
        basicSsl(),
        VitePWA({
            // We surface our own "New version available" toast and call the
            // returned updateSW() on click, so the SW must wait for the prompt
            // rather than auto-activating.
            registerType: 'prompt',
            // Bundled assets the SW must precache that aren't reachable from the
            // module graph (favicon, iOS icon). UI icons are inlined SVGs
            // (Iconify offline bundle) and ship inside the JS module graph.
            includeAssets: [
                'darkly-favicon.png',
                'icons/apple-touch-icon.png',
            ],
            manifest: {
                name: 'Darkly',
                short_name: 'Darkly',
                description:
                    'A GPU-native paint program written in Rust. Runs offline, no login, free and open source forever.',
                theme_color: '#000000',
                background_color: '#000000',
                display: 'standalone',
                start_url: './',
                scope: './',
                icons: [
                    { src: 'icons/pwa-192.png', sizes: '192x192', type: 'image/png' },
                    { src: 'icons/pwa-512.png', sizes: '512x512', type: 'image/png' },
                    {
                        src: 'icons/pwa-maskable-512.png',
                        sizes: '512x512',
                        type: 'image/png',
                        purpose: 'maskable',
                    },
                ],
            },
            workbox: {
                // The ~12 MB WASM blob is the whole app (now including Vello +
                // parley + a bundled font for the text tool) — precache it (and
                // the shell, fonts, icons) so the editor boots fully offline.
                globPatterns: ['**/*.{js,css,html,wasm,woff2,png,svg}'],
                // Default cap is ~2 MB, which would silently skip the WASM.
                maximumFileSizeToCacheInBytes: 16 * 1024 * 1024,
                // SPA: serve the precached index.html for navigations. The
                // relative `base: './'` emits the entry as `index.html`.
                navigateFallback: 'index.html',
            },
            devOptions: {
                // Register the SW on the dev server too, so install/offline can
                // be exercised without a full `build` + `preview`.
                enabled: false,
            },
        }),
    ],
    server: {
        fs: {
            allow: ['..'],
        },
    },
}));
