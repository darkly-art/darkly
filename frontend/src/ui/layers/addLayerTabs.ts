import type { CatalogEntry, Catalog } from '../../engine/protocol_gen';
import { parseMenuSegment, type Action } from '../../actions/registry';
import { groupByCategory, matchesQuery } from '../../lib/groupByCategory';
import type { AddSource } from './addSources';

/** One card in a tab — a catalog entry plus the source that spawns it. */
export interface AddCard {
    entry: CatalogEntry;
    source: AddSource;
    /** Catalog to request a preview from; empty for synthetic cards. */
    catalog: string;
}

export interface AddTab {
    title: string;
    cards: AddCard[];
}

/** What `buildTabs` needs to know about the world, so it can be tested without
 *  mounting a component or booting an engine. */
export interface TabDeps {
    sources: readonly AddSource[];
    /** The catalog with this id, or undefined if the engine hasn't sent it. */
    catalog: (id: string) => Catalog | undefined;
    /** The registered action with this id, or undefined. */
    action: (id: string) => Pick<Action, 'displayName' | 'description' | 'icon' | 'menuPath'> | undefined;
}

/** Sort key for a source: the order declared on its action's `Layer:N` segment.
 *  Sources whose action declares no order fall to the end, in declaration
 *  order, which is what `parseMenuSegment` already means everywhere else. */
function railOrder(source: AddSource, deps: TabDeps): number {
    const path = deps.action(source.action)?.menuPath;
    const last = path?.[path.length - 1];
    const order = last ? parseMenuSegment(last).order : undefined;
    return order ?? Number.MAX_SAFE_INTEGER;
}

/** The one card a catalog-less source contributes, built from its action so the
 *  tab reads the same as the Layer menu entry it replaces. */
function syntheticCard(source: AddSource, deps: TabDeps): AddCard | null {
    const action = deps.action(source.action);
    if (!action) return null;
    return {
        entry: {
            type: '',
            displayName: action.displayName ?? source.action,
            icon: action.icon || null,
            description: action.description ?? null,
            category: null,
            hotkeyAction: source.action,
            params: [],
            supportsPreview: false,
            captureKind: null,
            addable: true,
        } as CatalogEntry,
        source,
        catalog: '',
    };
}

/**
 * Derive the modal's tab rail.
 *
 * Sources order by their action's menu position, so the rail and the Layer menu
 * cannot drift. A source contributes one tab per distinct `category` its
 * entries declare, or a single tab when none does — the rule that makes Filters
 * and Veils two tabs of one source once the registries merge, with no change
 * here.
 *
 * Entries declaring `addable: false` are dropped: an effect registered in two
 * registries names its own add path, so the picker offers it once.
 */
export function buildTabs(deps: TabDeps): AddTab[] {
    const ordered = [...deps.sources].sort((a, b) => railOrder(a, deps) - railOrder(b, deps));
    // Keyed by title so two sources naming the same tab land in one group —
    // which is how "New Group" sits beside "New Layer" under Normal — and so a
    // category interleaved across a catalog collects rather than repeating.
    const byTitle = new Map<string, AddCard[]>();

    const add = (title: string, cards: AddCard[]) => {
        const existing = byTitle.get(title);
        if (existing) existing.push(...cards);
        else byTitle.set(title, cards);
    };

    for (const source of ordered) {
        if (!source.catalog) {
            const card = syntheticCard(source, deps);
            if (card) add(source.title ?? card.entry.displayName, [card]);
            continue;
        }

        const catalog = deps.catalog(source.catalog);
        if (!catalog) continue;
        const offered = catalog.entries.filter(e => e.addable !== false);
        if (offered.length === 0) continue;

        const fallback = source.title ?? catalog.title ?? source.action;
        for (const group of groupByCategory(offered, e => e.category, fallback)) {
            add(
                group.category,
                group.items.map(entry => ({ entry, source, catalog: source.catalog })),
            );
        }
    }

    return [...byTitle.entries()].map(([title, cards]) => ({ title, cards }));
}

/** Narrow every tab by a query, dropping tabs left empty. Searches name,
 *  description and category so a token can match any facet, and never
 *  resurrects an entry the addability gate removed. */
export function filterTabs(tabs: readonly AddTab[], query: string): AddTab[] {
    if (!query.trim()) return [...tabs];
    return tabs
        .map(tab => ({
            title: tab.title,
            cards: tab.cards.filter(card => {
                const e = card.entry;
                const haystack = [e.displayName, e.description ?? '', e.category ?? '', tab.title].join(' ');
                return matchesQuery(haystack, query);
            }),
        }))
        .filter(tab => tab.cards.length > 0);
}
