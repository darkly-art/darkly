/*
 * Keys in the docs: an action's chords as markup, and the reader's choice of
 * preset and key convention that picks among them.
 *
 * Every key is rendered for every (preset, convention) combination, grouped by
 * text, and CSS shows the one group the choice names. The choice is one token
 * (`Krita-mac`), held as `data-hotkeys` on <html>; DocsLayout's pre-paint
 * script sets it from storage before anything with a key is parsed, so no key
 * is ever painted and then swapped. Without JavaScript there is no attribute,
 * and the `hk-default` variant shows. Only the default variant is indexed for
 * search; every other one carries `data-pagefind-ignore`.
 *
 * Imported by scripts/gen-docs.mjs under Node's type stripping as well as by
 * components, so this file uses erasable syntax only and imports nothing.
 */

export interface Chord {
    mac: string;
    other: string;
}

/** A row's chords by preset. A preset that binds nothing is absent. */
export type Chords = Partial<Record<string, readonly Chord[]>>;

/** `mac` renders Apple's modifier glyphs, `other` the Windows / Linux names. */
export type Os = 'mac' | 'other';
export const OSES: readonly Os[] = ['other', 'mac'];

// Photoshop by default: the keymap most people arriving here already know.
export const DEFAULT_PRESET = 'Photoshop';
export const DEFAULT_OS: Os = 'other';
export const STORAGE_KEY = 'darkly-hotkeys';
export const ATTR = 'data-hotkeys';

/*
 * The one OS detection on the site, tested against the user agent alone.
 * iPadOS in desktop mode reports `Macintosh`, and iPhone and iPod name
 * themselves, so `navigator.platform` (deprecated) adds nothing.
 */
export const MAC_UA = 'Mac|iPhone|iPad|iPod|Darwin';
export const isMac = (): boolean => new RegExp(MAC_UA, 'i').test(navigator.userAgent);

/** The default preset, or the first the export carries if it ever drops out. */
export const defaultPreset = (presets: readonly string[]): string =>
    presets.includes(DEFAULT_PRESET) ? DEFAULT_PRESET : presets[0];

export interface Choice {
    token: string;
    preset: string;
    os: Os;
}

/*
 * A choice as one token: the attribute's value, the stored value, and the
 * class suffix (`hk-<token>`). A preset's display name may hold any character,
 * so it is slugged, which also keeps the token safe inside a CSS selector.
 */
export const choiceToken = (preset: string, os: Os): string =>
    `${preset.replace(/[^A-Za-z0-9_]/g, '_')}-${os}`;

export const choices = (presets: readonly string[]): Choice[] =>
    presets.flatMap((preset) => OSES.map((os) => ({ token: choiceToken(preset, os), preset, os })));

/** One convention's text for a preset's chords, repeats dropped; `''` when unbound. */
export function chordText(chords: readonly Chord[] | undefined, os: Os): string {
    return [...new Set((chords ?? []).map((c) => c[os]))].join(' / ');
}

/*
 * Numeric references for everything HTML or markdown could read as syntax: a
 * lone `*` or `_`, a `[` and a later `]`, a table's `|`. That keeps text from
 * being parsed, in a `.md` page and through `{@html}`, but not from Astro's
 * smartypants, which runs on the decoded text; see `kbdMarkdown`.
 */
export const esc = (text: string): string =>
    text.replace(/[&<>"'*_`[\]\\|~]/g, (c) => `&#${c.charCodeAt(0)};`);

/** A key for `{@html}`, where no markdown runs. */
export const kbdHtml = (text: string): string => `<kbd class="hk-key">${esc(text)}</kbd>`;

/*
 * A key for a markdown page: a code span inside the `<kbd>`. Between inline
 * HTML tags markdown still runs, and smartypants would curl a `'` and join
 * `--`; code is the one inline construct both leave verbatim. The fence is one
 * backtick longer than the longest run in the text, padded when the text
 * starts or ends with one. A `|` would still split a table row and a newline
 * would end it, so neither is accepted.
 */
export function kbdMarkdown(text: string): string {
    if (/[|\n]/.test(text)) throw new Error(`key '${text}' holds a '|' or a newline, which a table row cannot`);
    const fence = '`'.repeat(Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length)) + 1);
    const pad = /^`|`$/.test(text) ? ' ' : '';
    return `<kbd class="hk-key">${fence}${pad}${text}${pad}${fence}</kbd>`;
}

/*
 * Variants of one thing, each covering the choice tokens it is shown for, as
 * one line of markup (spliced into a table row or a list item, a newline would
 * end it). One variant needs no switching and is emitted bare.
 */
function variantsHtml(variants: Map<string, string[]>, presets: readonly string[]): string {
    if (variants.size === 1) return [...variants.keys()][0];
    const def = choiceToken(defaultPreset(presets), DEFAULT_OS);
    const spans = [...variants].map(([html, tokens]) => {
        const isDefault = tokens.includes(def);
        const cls = ['hk-v', ...tokens.map((t) => `hk-${t}`), ...(isDefault ? ['hk-default'] : [])];
        return `<span class="${cls.join(' ')}"${isDefault ? '' : ' data-pagefind-ignore'}>${html}</span>`;
    });
    return `<span class="hk">${spans.join('')}</span>`;
}

interface HotkeyOptions {
    /** Markup for a combination that binds nothing. */
    unbound?: string;
    /** `kbdHtml` or `kbdMarkdown`, for where the markup lands. */
    key?: (text: string) => string;
}

/** An action's key in every combination, grouped by text. */
export function hotkeyHtml(chords: Chords, presets: readonly string[], options: HotkeyOptions = {}): string {
    const { unbound = '', key = kbdHtml } = options;
    const variants = new Map<string, string[]>();
    for (const { token, preset, os } of choices(presets)) {
        const text = chordText(chords[preset], os);
        const html = text ? key(text) : unbound;
        variants.set(html, [...(variants.get(html) ?? []), token]);
    }
    return variantsHtml(variants, presets);
}

/** The chosen preset's name, selected the same way as its keys. */
export function presetLabelHtml(presets: readonly string[]): string {
    return variantsHtml(
        new Map(presets.map((p) => [esc(p), OSES.map((os) => choiceToken(p, os))])),
        presets,
    );
}

/** What shows which variant: written to src/generated/hotkeys.css by gen-docs.mjs. */
export function variantCss(presets: readonly string[]): string {
    return [
        '.hk-v{display:none}',
        ...choices(presets).map(({ token }) => `html[${ATTR}="${token}"] .hk-${token}{display:inline}`),
        `html:not([${ATTR}]) .hk-default{display:inline}`,
        '',
    ].join('\n');
}

// ---------------------------------------------------------------- browser side

/** The choice the page shows, read back from the attribute. */
export function current(presets: readonly string[]): Choice {
    const all = choices(presets);
    const token = document.documentElement.getAttribute(ATTR);
    return all.find((c) => c.token === token) ?? all.find((c) => c.token === choiceToken(defaultPreset(presets), DEFAULT_OS))!;
}

export function choose(token: string): void {
    try {
        localStorage.setItem(STORAGE_KEY, token);
    } catch {}
    document.documentElement.setAttribute(ATTR, token);
}

/** Calls `fn` whenever the choice changes; returns the unsubscribe. */
export function onChoice(fn: () => void): () => void {
    const observer = new MutationObserver(fn);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: [ATTR] });
    return () => observer.disconnect();
}
