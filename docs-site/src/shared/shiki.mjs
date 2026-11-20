/*
 * Code highlighting for rendered markdown, as Astro's `markdown.shikiConfig`.
 *
 * Both themes on every token, switched by a CSS variable rather than a second
 * stylesheet. `defaultColor: false` is what hands that switch to the page, and
 * styles/prose.css drives it off `[data-theme]` so code follows the theme
 * toggle with no flash. Those rules depend on this setting, which is why the
 * two are shared together.
 *
 * Only `shikiConfig` is exported, so each site's own markdown plugins stay in
 * its own config: `markdown: { shikiConfig, remarkPlugins: [...] }`.
 */
export const shikiConfig = {
    themes: { light: 'github-light', dark: 'github-dark' },
    defaultColor: false,
    wrap: false,
};
