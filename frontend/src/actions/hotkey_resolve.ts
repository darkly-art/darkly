/**
 * Pure helpers for site/scope-aware chord resolution. Kept free of Svelte
 * runes and DOM globals so the resolution semantics can be unit-tested
 * without standing up a JSDOM environment. Used by both the keyboard
 * dispatcher in `config/hotkeys.svelte.ts` and the mouse/drag dispatcher
 * in `actions/triggers.ts`.
 *
 * Binding-string grammar (`<site>` and `<toolGroup>` both optional):
 *
 *     [<site>][@<toolGroup>]:<chord>
 *     <chord>                              // bare chord = global
 *
 * Examples:
 *   "Delete"                  → global, fires anywhere
 *   "layerPanel:Delete"       → fires only when layerPanel site is active
 *   "canvas@paint:shift+drag" → fires only when click is on canvas AND the
 *                               active tool's group is "paint"
 *   "@paint:KeyB"             → fires only when active tool group is "paint",
 *                               regardless of focus/click site
 *
 * The dispatcher consumes these to:
 *   1. Build a `chord → ordered ChordEntry[]` index from action defaults
 *      + user overrides + preset overrides.
 *   2. Pick the right entry when a chord fires, given the currently-active
 *      binding-site chain and the active tool's group.
 */

/** Split a binding into its site, tool-scope, and chord parts.
 *  Returns `site`/`scope` as `null` when the binding doesn't specify them.
 *
 *  Examples:
 *   `"Delete"`                  → `{ site: null,         scope: null,    chord: "Delete" }`
 *   `"layerPanel:Delete"`       → `{ site: "layerPanel", scope: null,    chord: "Delete" }`
 *   `"canvas@paint:shift+drag"` → `{ site: "canvas",     scope: "paint", chord: "shift+drag" }`
 *   `"@paint:KeyB"`             → `{ site: null,         scope: "paint", chord: "KeyB" }`
 *
 *  The colon is the chord separator; `@` (when it appears before the
 *  separator) splits site from tool-scope. Anything after the first `:`
 *  is the chord verbatim — `@` inside a chord stays put. */
export function parseBinding(raw: string): {
    site: string | null;
    scope: string | null;
    chord: string;
} {
    const colonIdx = raw.indexOf(':');
    if (colonIdx < 0) return { site: null, scope: null, chord: raw };
    const sitePart = raw.slice(0, colonIdx);
    const chord = raw.slice(colonIdx + 1);
    const atIdx = sitePart.indexOf('@');
    if (atIdx < 0) {
        return { site: sitePart || null, scope: null, chord };
    }
    const site = sitePart.slice(0, atIdx);
    const scope = sitePart.slice(atIdx + 1);
    return { site: site || null, scope: scope || null, chord };
}

/** Ordered entry in a chord's resolution list. */
export interface ChordEntry {
    /** Binding-site name (e.g. `"layerPanel"`, `"canvas"`), or `null` for any. */
    site: string | null;
    /** Active-tool group (e.g. `"paint"`, `"select"`), or `null` for any tool. */
    scope: string | null;
    actionId: string;
}

/** Specificity score: site+scope > site-only > scope-only > global.
 *  Higher fires first. */
function specificity(e: ChordEntry): number {
    return (e.site !== null ? 2 : 0) + (e.scope !== null ? 1 : 0);
}

/** Build `chord → ordered ChordEntry[]` from an enumeration of
 *  `(actionId, bindings[])`. Each list is sorted most-specific first so
 *  the dispatcher can walk first-match-wins. */
export function buildChordIndex(
    sources: Iterable<{ actionId: string; bindings: string[] }>,
): Map<string, ChordEntry[]> {
    const out = new Map<string, ChordEntry[]>();
    for (const { actionId, bindings } of sources) {
        for (const raw of bindings) {
            const { site, scope, chord } = parseBinding(raw);
            if (!chord) continue;
            let list = out.get(chord);
            if (!list) { list = []; out.set(chord, list); }
            list.push({ site, scope, actionId });
        }
    }
    for (const list of out.values()) {
        list.sort((a, b) => specificity(b) - specificity(a));
    }
    return out;
}

/** Pick which action fires for a chord given the active site chain and
 *  the active tool group. Entries should already be priority-sorted
 *  (most-specific first). Returns the chosen entry plus the matched site
 *  (or `null` when the match was global wrt the chain), or `null` if no
 *  entry resolved.
 *
 *  An entry matches when:
 *    - its `scope` is null OR equals `toolGroup`, AND
 *    - its `site` is null OR appears in `chain`.
 *
 *  Generic on the chain element so tests can pass minimal `{ name }`
 *  shapes without faking ctx producers. */
export function resolveChord<S extends { name: string }>(
    entries: ChordEntry[],
    chain: S[],
    toolGroup: string | null,
): { entry: ChordEntry; site: S | null } | null {
    for (const entry of entries) {
        if (entry.scope !== null && entry.scope !== toolGroup) continue;
        if (entry.site === null) return { entry, site: null };
        const match = chain.find(s => s.name === entry.site);
        if (match) return { entry, site: match };
    }
    return null;
}
