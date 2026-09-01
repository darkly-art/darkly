/**
 * Grouping and search over lists of categorized things.
 *
 * The brush picker, the brush-builder node menus and the add-layer modal all
 * present the same shape — a flat list carrying an optional category, grouped
 * into sections with a fallback bucket, narrowed by a typed query. This module
 * is that shape, so the arithmetic lives in one place and stays testable
 * without mounting a component.
 *
 * Grouping is map-based, not adjacency-based: a catalog sorted by `type_id`
 * interleaves its categories, and adjacency grouping would emit the same
 * category several times.
 */

/** One category and the items that declared it, in first-seen order. */
export interface CategoryGroup<T> {
    category: string;
    items: T[];
}

/**
 * Group by category, preserving first-seen order for both groups and members.
 *
 * `categoryOf` returning an empty value puts the item under `fallback`, so a
 * list where nothing declares a category yields exactly one group.
 */
export function groupByCategory<T>(
    items: readonly T[],
    categoryOf: (item: T) => string | null | undefined,
    fallback: string,
): CategoryGroup<T>[] {
    const map = new Map<string, T[]>();
    for (const item of items) {
        const key = categoryOf(item) || fallback;
        const existing = map.get(key);
        if (existing) existing.push(item);
        else map.set(key, [item]);
    }
    return [...map.entries()].map(([category, groupItems]) => ({
        category,
        items: groupItems,
    }));
}

/**
 * Whitespace-tokenized substring match — `"soft round"` matches "Soft Round"
 * but `"soft xxx"` does not. Every token must appear somewhere in the haystack,
 * so tokens can match across different facets of the same item.
 *
 * An empty or whitespace-only query matches everything.
 */
export function matchesQuery(haystack: string, query: string): boolean {
    const tokens = query.toLowerCase().trim().split(/\s+/).filter(t => t.length > 0);
    if (tokens.length === 0) return true;
    const hay = haystack.toLowerCase();
    return tokens.every(t => hay.includes(t));
}
