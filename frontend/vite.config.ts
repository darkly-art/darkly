import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
    plugins: [svelte()],
    server: {
        port: 8080,
        host: "0.0.0.0",
    },
});
