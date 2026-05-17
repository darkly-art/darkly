/**
 * Pure helpers for site-scoped keyboard hotkey resolution. Kept free of
 * Svelte runes and DOM globals so the resolution semantics can be unit-
 * tested without standing up a JSDOM environment.
 *
 * The dispatcher in `config/hotkeys.svelte.ts` consumes these to:
 *   1. Build a `chord → ordered ChordEntry[]` index from action defaults
 *      + user overrides.
 *   2. Pick the right entry to fire when a chord triggers, given the
 *      currently-active binding-site chain.
 */

/** Split a binding into its scope and chord.
 *   `"Delete"`              → `{ site: null,         chord: "Delete" }`
 *   `"layerPanel:Delete"`   → `{ site: "layerPanel", chord: "Delete" }`
 *   `"$mod+Shift+KeyZ"`     → `{ site: null,         chord: "$mod+Shift+KeyZ" }`
 *
 * Note: the split is on the FIRST `:`, so chords containing colons (none
 * exist in tinykeys notation today) would be misparsed. Tighten if that
 * ever changes. */
export function parseBinding(raw: string): { site: string | null; chord: string } {
    const idx = raw.indexOf(':');
    if (idx < 0) return { site: null, chord: raw };
    return { site: raw.slice(0, idx), chord: raw.slice(idx + 1) };
}

/** Ordered entry in a chord's resolution list. */
export interface ChordEntry {
    /** Binding-site name (e.g. `"layerPanel"`), or `null` for global. */
    site: string | null;
    actionId: string;
}

/** Build `chord → ordered ChordEntry[]` from an enumeration of
 *  `(actionId, bindings[])`. Each list is sorted so scoped entries come
 *  before global, so the dispatcher can walk first-match-wins. */
export function buildChordIndex(
    sources: Iterable<{ actionId: string; bindings: string[] }>,
): Map<string, ChordEntry[]> {
    const out = new Map<string, ChordEntry[]>();
    for (const { actionId, bindings } of sources) {
        for (const raw of bindings) {
            const { site, chord } = parseBinding(raw);
            if (!chord) continue;
            let list = out.get(chord);
            if (!list) { list = []; out.set(chord, list); }
            list.push({ site, actionId });
        }
    }
    for (const list of out.values()) {
        list.sort((a, b) => {
            if ((a.site === null) === (b.site === null)) return 0;
            return a.site === null ? 1 : -1;
        });
    }
    return out;
}

/** Pick which action fires for a chord given the active site chain. The
 *  chain is innermost-first; `entries` is already priority-sorted (scoped
 *  before global). Returns the chosen entry plus the matched site (or
 *  `null` when the match was a global fallback), or `null` if no entry
 *  resolved. Generic on the chain element so tests can pass minimal
 *  `{ name }` shapes without faking ctx producers. */
export function resolveChord<S extends { name: string }>(
    entries: ChordEntry[],
    chain: S[],
): { entry: ChordEntry; site: S | null } | null {
    for (const entry of entries) {
        if (entry.site === null) return { entry, site: null };
        const match = chain.find(s => s.name === entry.site);
        if (match) return { entry, site: match };
    }
    return null;
}
