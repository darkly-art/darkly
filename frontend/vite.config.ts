import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import basicSsl from '@vitejs/plugin-basic-ssl';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
    // Relative asset paths so the same dist/ works when served from a web root
    // ("/") and when loaded via file:// from a packaged desktop bundle.
    base: './',
    plugins: [
        svelte(),
        basicSsl(),
        VitePWA({
            // We surface our own "New version available" toast and call the
            // returned updateSW() on click, so the SW must wait for the prompt
            // rather than auto-activating.
            registerType: 'prompt',
            // Bundled assets the SW must precache that aren't reachable from the
            // module graph (favicon, iOS icon, and the FontAwesome webfonts the
            // UI icon font depends on offline).
            includeAssets: [
                'darkly-favicon.png',
                'icons/apple-touch-icon.png',
                'fontawesome/webfonts/*.woff2',
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
                // The 6.2 MB WASM blob is the whole app — precache it (and the
                // shell, fonts, icons) so the editor boots fully offline.
                globPatterns: ['**/*.{js,css,html,wasm,woff2,png,svg}'],
                // Default cap is ~2 MB, which would silently skip the WASM.
                maximumFileSizeToCacheInBytes: 10 * 1024 * 1024,
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
});
