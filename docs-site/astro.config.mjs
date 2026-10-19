import { defineConfig } from 'astro/config';
import svelte from '@astrojs/svelte';
import sitemap from '@astrojs/sitemap';
import mdx from '@astrojs/mdx';
import pagefind from './src/shared/integrations/pagefind.mjs';
import { shikiConfig } from './src/shared/shiki.mjs';
import { BASE } from './base.mjs';

export default defineConfig({
    site: 'https://darkly.art',
    base: BASE,
    integrations: [
        svelte(),
        sitemap(),
        // Needed by the one generated docs page that mounts a component:
        // reference/actions.mdx; every other page the pipeline emits is plain
        // markdown.
        mdx(),
        // Last: it reads the built output.
        pagefind(),
    ],
    markdown: { shikiConfig },
});
