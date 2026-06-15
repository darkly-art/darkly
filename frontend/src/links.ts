/** Canonical external links to Darkly's web presence. Single source of truth —
 *  imported by the About modal and the Help menu actions so a URL change lands
 *  in one place. */
export const links = {
    website: 'https://darkly.art',
    docs: 'https://darkly.art/docs',
    github: 'https://github.com/darkly-art/darkly',
} as const;

/** Open an external URL in a new tab, severing the opener reference. */
export function openExternal(url: string): void {
    window.open(url, '_blank', 'noopener,noreferrer');
}
