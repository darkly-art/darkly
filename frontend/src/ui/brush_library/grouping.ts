/**
 * How the brush picker lays brushes out under their packs.
 *
 * Pure, so it is testable without a DOM — the same reason `placement.ts` sits
 * beside the component rather than inside it.
 *
 * A brush may be in any number of packs and renders under each of them: packs
 * are groupings, not folders. That is what makes the rendered cell list differ
 * from the filtered brush list, and why keyboard navigation must index the
 * cells rather than the filter.
 */
import type { BrushInfo, BrushPackInfo } from '../../engine/protocol_gen';
import { NEUTRAL_PALETTE, type PackPalette } from '../../lib/packPalette';

/** Label shown over the brushes no pack holds. */
export const NO_PACK_LABEL = 'In no pack';

export interface BrushGroup {
    /** The pack's id, `''` for the derived "in no pack" section, or
     *  {@link RECENTS_ID}. Use it as a list key, never to decide behaviour. */
    id: string;
    label: string;
    icon: string;
    /** The four colours this group is drawn in. A derived group borrows
     *  `NEUTRAL_PALETTE`, so every consumer has a real palette to read and none
     *  has to ask whether a pack is behind it. */
    palette: PackPalette;
    brushes: BrushInfo[];
    /**
     * The pack behind this group, or `null` for a derived one (Recents, "in no
     * pack").
     *
     * Carried here so a consumer never has to look a pack up by group id and
     * branch on whether it found one — that is the consumer-side classification
     * the Modularity Principle bans, and it is what the permission booleans on
     * `BrushPackInfo` exist to make unnecessary. A card reads
     * `group.pack?.can_edit_members` and never sees an id.
     */
    pack: BrushPackInfo | null;
}

/** Id of the pinned Recents group.
 *
 *  Recents is **not** a pack: `recents.svelte.ts` is frontend-only and never
 *  crosses the wasm boundary, while packs are engine-owned and persisted. It is
 *  synthesized here so the list and the wheel can treat it like any other
 *  group, with `pack: null` marking that nothing may be edited about it. */
export const RECENTS_ID = 'recents';
export const RECENTS_LABEL = 'Recent';

/**
 * Group `filtered` by pack, in the packs' own order, then append the brushes
 * no pack holds.
 *
 * A pack with no visible members yields no group at all — which is what keeps
 * an empty Favorites from rendering as a broken-looking empty heading, and
 * what hides packs the current search excludes entirely.
 *
 * `resolveIcon` maps a pack's declared icon to one the renderer actually has;
 * an imported pack may name anything.
 */
export function groupByPack(
    filtered: BrushInfo[],
    packs: BrushPackInfo[],
    resolveIcon: (icon: string) => string,
    noPackIcon: string,
): BrushGroup[] {
    const visible = new Map(filtered.map(b => [b.id, b]));
    const out: BrushGroup[] = [];
    const grouped = new Set<string>();

    for (const pack of packs) {
        const brushes: BrushInfo[] = [];
        for (const id of pack.members) {
            const brush = visible.get(id);
            if (!brush) continue;
            brushes.push(brush);
            grouped.add(id);
        }
        if (brushes.length === 0) continue;
        out.push({
            id: pack.id,
            label: pack.name,
            icon: resolveIcon(pack.icon),
            palette: pack.palette,
            brushes,
            pack,
        });
    }

    // The complement of the one membership relation, computed rather than
    // stored — a brush does not depend on a pack to exist.
    const loose = filtered.filter(b => !grouped.has(b.id));
    if (loose.length > 0) {
        out.push({
            id: '',
            label: NO_PACK_LABEL,
            icon: noPackIcon,
            palette: NEUTRAL_PALETTE,
            brushes: loose,
            pack: null,
        });
    }
    return out;
}

/**
 * Prepend a Recents group of at most `limit` brushes, newest first.
 *
 * `recentIds` is newest-first and id-keyed. Ids that no longer resolve — a
 * deleted brush, or one the current search excludes — are skipped rather than
 * placeheld, and nothing resolving yields no group at all, the same rule
 * `groupByPack` applies to an empty pack. That is what keeps an empty Recents
 * from rendering as a broken heading on a first run.
 *
 * A brush here also appears under its packs, which is a third source of the
 * duplicate-cell hazard this module's header warns about.
 */
export function withRecents(
    groups: BrushGroup[],
    recentIds: string[],
    visible: BrushInfo[],
    limit: number,
    icon: string,
): BrushGroup[] {
    const byId = new Map(visible.map(b => [b.id, b]));
    const brushes: BrushInfo[] = [];
    for (const id of recentIds) {
        if (brushes.length >= limit) break;
        const brush = byId.get(id);
        if (brush) brushes.push(brush);
    }
    if (brushes.length === 0) return groups;
    return [
        {
            id: RECENTS_ID,
            label: RECENTS_LABEL,
            icon,
            palette: NEUTRAL_PALETTE,
            brushes,
            pack: null,
        },
        ...groups,
    ];
}

/** Every brush id mapped to the names of the packs holding it. Membership
 *  lives on the pack, so searching by pack name means reading it from that
 *  side. */
export function packNamesByBrush(packs: BrushPackInfo[]): Map<string, string[]> {
    const map = new Map<string, string[]>();
    for (const pack of packs) {
        for (const member of pack.members) {
            const existing = map.get(member);
            if (existing) existing.push(pack.name);
            else map.set(member, [pack.name]);
        }
    }
    return map;
}

/** Whitespace-tokenized substring match — `"soft round"` matches "Soft Round"
 *  but `"soft xxx"` does not. Searches name, the packs a brush is in, and its
 *  tags, so a brush is findable by any facet. */
export function matchesQuery(
    brush: BrushInfo,
    query: string,
    packNames: Map<string, string[]>,
): boolean {
    const tokens = query.toLowerCase().trim().split(/\s+/).filter(t => t.length > 0);
    if (tokens.length === 0) return true;
    const haystack = [
        brush.name,
        ...(packNames.get(brush.id) ?? []),
        ...(brush.tags ?? []),
    ]
        .join(' ')
        .toLowerCase();
    return tokens.every(t => haystack.includes(t));
}
