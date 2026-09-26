/** Canonical external links to Darkly's web presence. Single source of truth:
 *  imported by the About modal and the Help menu actions so a URL change lands
 *  in one place. */
export const links = {
    website: 'https://darkly.art',
    docs: 'https://darkly.art/docs',
    github: 'https://github.com/darkly-art/darkly',
    discord: 'https://discord.gg/kFz2FGhbpu',
} as const;

/** Open an external URL in a new tab, severing the opener reference. */
export function openExternal(url: string): void {
    window.open(url, '_blank', 'noopener,noreferrer');
}

/** Point every anchor under `root` at a new tab, with the opener severed.
 *
 *  For markup rendered with `{@html}` whose author writes plain `<a href>`:
 *  Darkly holds unsaved work in a WebGPU canvas, so a same-tab navigation
 *  tears the editor down and the artist meets the browser's unsaved-changes
 *  prompt instead of the link they clicked. The two attributes have to live on
 *  the element rather than in a click handler, which cannot cover middle-click
 *  or the context menu's "open in new tab". `rel` carries the same pair of
 *  tokens `openExternal` passes to `window.open`, for the same reason. */
export function hardenExternalAnchors(root: HTMLElement): void {
    for (const a of root.querySelectorAll('a')) {
        a.target = '_blank';
        a.rel = 'noopener noreferrer';
    }
}
