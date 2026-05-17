/**
 * Tracks which `bindingSite` element currently owns keyboard focus, and
 * returns the chain of binding-site ancestors of `document.activeElement`
 * for the hotkey dispatcher to walk innermost-first.
 *
 * A binding site is just a DOM node tagged with a name + ctx-producer via
 * the `bindingSite` Svelte action. The DOM is the source of truth: focus
 * decides which site is active, so we don't track focus state ourselves —
 * we just look up `document.activeElement` at dispatch time.
 */

export interface BindingSiteEntry {
    /** Site name (matches `sites.register(...)` and the `<site>:<chord>` prefix). */
    name: string;
    /** Resolved at dispatch time, optionally given the triggering event. */
    ctx: (e?: Event) => Record<string, unknown>;
}

/** WeakMap from DOM node → site entry. Cleared on action destroy. */
const SITES = new WeakMap<Element, BindingSiteEntry>();

export function registerSite(node: Element, entry: BindingSiteEntry) {
    SITES.set(node, entry);
}

export function unregisterSite(node: Element) {
    SITES.delete(node);
}

export function siteEntryFor(node: Element): BindingSiteEntry | undefined {
    return SITES.get(node);
}

/** Walk from `document.activeElement` outward, returning matching binding-
 *  site ancestors in innermost-first order. */
export function activeSiteChain(): BindingSiteEntry[] {
    const chain: BindingSiteEntry[] = [];
    let el: Element | null = document.activeElement;
    while (el && el !== document.documentElement) {
        const entry = SITES.get(el);
        if (entry) chain.push(entry);
        el = el.parentElement;
    }
    return chain;
}
