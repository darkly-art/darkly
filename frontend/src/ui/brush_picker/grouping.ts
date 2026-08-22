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

/** Label shown over the brushes no pack holds. */
export const NO_PACK_LABEL = 'In no pack';

export interface BrushGroup {
    /** The pack's id, or `''` for the derived "in no pack" section. */
    id: string;
    label: string;
    icon: string;
    primary: string;
    secondary: string;
    brushes: BrushInfo[];
}

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
            primary: pack.primary,
            secondary: pack.secondary,
            brushes,
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
            primary: 'transparent',
            secondary: 'transparent',
            brushes: loose,
        });
    }
    return out;
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
