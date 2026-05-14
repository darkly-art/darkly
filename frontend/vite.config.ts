import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import basicSsl from '@vitejs/plugin-basic-ssl';

export default defineConfig({
    // Relative asset paths so the same dist/ works when served from a web root
    // ("/") and when loaded via file:// from a packaged desktop bundle.
    base: './',
    plugins: [svelte(), basicSsl()],
    server: {
        fs: {
            allow: ['..'],
        },
    },
});
