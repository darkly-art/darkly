/**
 * Pure helpers for site/scope-aware chord resolution. Kept free of Svelte
 * runes and DOM globals so the resolution semantics can be unit-tested
 * without standing up a JSDOM environment. Used by both the keyboard
 * dispatcher in `config/hotkeys.svelte.ts` and the mouse/drag dispatcher
 * in `actions/triggers.ts`.
 *
 * Binding-string grammar (`<site>`, `<toolGroup>`, `<brush>` all optional):
 *
 *     [<site>][@<toolGroup>[@<brush>]]:<chord>
 *     <chord>                              // bare chord = global
 *
 * Examples:
 *   "Delete"                  → global, fires anywhere
 *   "layerPanel:Delete"       → fires only when layerPanel site is active
 *   "canvas@paint:shift+drag" → fires only when click is on canvas AND the
 *                               active tool's group is "paint"
 *   "@paint:KeyB"             → fires only when active tool group is "paint",
 *                               regardless of focus/click site
 *   "canvas@paint@clone:$mod+drag"
 *                             → fires only when click is on canvas AND the
 *                               active tool group is "paint" AND the active
 *                               brush is "clone". Out-ranks the group-scoped
 *                               `canvas@paint:$mod+drag` (color sampler) so a
 *                               brush can claim a modifier the tool group also
 *                               uses (see `specificity`).
 *
 * The dispatcher consumes these to:
 *   1. Build a `chord → ordered ChordEntry[]` index from action defaults
 *      + user overrides + preset overrides.
 *   2. Pick the right entry when a chord fires, given the currently-active
 *      binding-site chain and the active tool's group.
 */

/** Split a binding into its site, tool-scope, brush, and chord parts.
 *  Returns `site`/`scope`/`brush` as `null` when the binding doesn't
 *  specify them.
 *
 *  Examples:
 *   `"Delete"`                  → `{ site: null,         scope: null,    brush: null,    chord: "Delete" }`
 *   `"layerPanel:Delete"`       → `{ site: "layerPanel", scope: null,    brush: null,    chord: "Delete" }`
 *   `"canvas@paint:shift+drag"` → `{ site: "canvas",     scope: "paint", brush: null,    chord: "shift+drag" }`
 *   `"@paint:KeyB"`             → `{ site: null,         scope: "paint", brush: null,    chord: "KeyB" }`
 *   `"canvas@paint@clone:$mod+drag"`
 *                               → `{ site: "canvas",     scope: "paint", brush: "clone", chord: "$mod+drag" }`
 *
 *  The colon is the chord separator; the site part before it is split on
 *  `@` into `site@scope@brush` (each optional). Anything after the first
 *  `:` is the chord verbatim; `@` inside a chord stays put. */
export function parseBinding(raw: string): {
    site: string | null;
    scope: string | null;
    brush: string | null;
    chord: string;
} {
    const colonIdx = raw.indexOf(':');
    if (colonIdx < 0) return { site: null, scope: null, brush: null, chord: raw };
    const sitePart = raw.slice(0, colonIdx);
    const chord = raw.slice(colonIdx + 1);
    const atIdx = sitePart.indexOf('@');
    if (atIdx < 0) {
        return { site: sitePart || null, scope: null, brush: null, chord };
    }
    const site = sitePart.slice(0, atIdx);
    const rest = sitePart.slice(atIdx + 1).split('@');
    const scope = rest[0] || null;
    const brush = rest[1] || null;
    return { site: site || null, scope, brush, chord };
}

/** Ordered entry in a chord's resolution list. */
export interface ChordEntry {
    /** Binding-site name (e.g. `"layerPanel"`, `"canvas"`), or `null` for any. */
    site: string | null;
    /** Active-tool group (e.g. `"paint"`, `"select"`), or `null` for any tool. */
    scope: string | null;
    /** Active brush name (e.g. `"clone"`), or `null` for any brush. */
    brush: string | null;
    actionId: string;
}

/** Specificity score. `site` dominates `scope` dominates `brush` so the
 *  existing site/scope ordering is unchanged, and a brush-scoped entry
 *  out-ranks the otherwise-identical group-scoped one (letting a brush
 *  claim a modifier its tool group also binds, e.g. clone's set-source
 *  vs. the color sampler, both `canvas@paint…:$mod+drag`). Higher fires
 *  first. */
function specificity(e: ChordEntry): number {
    return (e.site !== null ? 4 : 0) + (e.scope !== null ? 2 : 0) + (e.brush !== null ? 1 : 0);
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
            const { site, scope, brush, chord } = parseBinding(raw);
            if (!chord) continue;
            let list = out.get(chord);
            if (!list) { list = []; out.set(chord, list); }
            list.push({ site, scope, brush, actionId });
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
 *    - its `brush` is null OR equals `activeBrush`, AND
 *    - its `site` is null OR appears in `chain`.
 *
 *  Generic on the chain element so tests can pass minimal `{ name }`
 *  shapes without faking ctx producers. */
export function resolveChord<S extends { name: string }>(
    entries: ChordEntry[],
    chain: S[],
    toolGroup: string | null,
    activeBrush: string | null = null,
): { entry: ChordEntry; site: S | null } | null {
    for (const entry of entries) {
        if (entry.scope !== null && entry.scope !== toolGroup) continue;
        if (entry.brush !== null && entry.brush !== activeBrush) continue;
        if (entry.site === null) return { entry, site: null };
        const match = chain.find(s => s.name === entry.site);
        if (match) return { entry, site: match };
    }
    return null;
}
