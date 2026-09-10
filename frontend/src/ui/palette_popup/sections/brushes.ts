/**
 * The brushes section of the palette popup: the top two thirds of ring 0,
 * as exactly two branches. Recent (left third) fans out to the recent
 * brushes; Library (right third) opens every pack as a full-circumference
 * submenu (packs can be numerous, so they get the whole circle rather than
 * a fan), and each pack fans out to its brush leaves.
 *
 * Committing a leaf loads the brush through `brushGraph.loadBrush`, which
 * records recents itself.
 */
import { brushGraph } from '../../../state/brush_graph.svelte';
import { brushLibrary } from '../../../state/brush_library.svelte';
import { recentBrushes } from '../../../state/recents.svelte';
import { NEUTRAL_PALETTE, type PackPalette } from '../../../lib/packPalette';
import { paletteSections, type WheelNode } from '../model';

/** Brushes shown in the Recent fan, of the 12 recents stored: the tail of
 *  the recency list is cold, and a short fan keeps its sectors wide. */
export const RECENT_COUNT = 5;

/** The fields the wheel needs from `BrushInfo` / `BrushPackInfo`, so tests
 *  can hand in plain objects. */
export interface BrushLike { id: string; name: string; icon: string | null }
export interface PackLike {
    id: string;
    name: string;
    icon: string;
    members: string[];
    palette: PackPalette;
}

export interface BrushDeps {
    recentIds(): string[];
    brushes(): BrushLike[];
    packs(): PackLike[];
    load(name: string, id: string): void;
    /** The colours a brush wears when it is not being shown under a pack. */
    paletteFor(name: string): PackPalette;
}

/** A brush under a pack takes *that* pack's colours, not the library's answer
 *  for which pack holds it: a brush may be in several (packs are groupings,
 *  not folders), and the one the painter navigated through is the one they
 *  are looking at. Only the Recent fan, which has no pack above it, asks. */
function brushLeaf(b: BrushLike, palette: PackPalette, load: BrushDeps['load']): WheelNode {
    return {
        kind: 'leaf',
        id: `brush:${b.id}`,
        label: b.name,
        visual: { kind: 'brush', name: b.name, icon: b.icon },
        palette,
        select: () => load(b.name, b.id),
    };
}

export function brushNodes(deps: BrushDeps): WheelNode[] {
    const byId = new Map(deps.brushes().map(b => [b.id, b]));
    const resolve = (ids: string[]) =>
        ids.map(id => byId.get(id)).filter((b): b is BrushLike => b !== undefined);

    const out: WheelNode[] = [];
    // Resolve before capping, so dangling ids never cost a shown slot.
    const recent = resolve(deps.recentIds()).slice(0, RECENT_COUNT);
    if (recent.length > 0) {
        out.push({
            kind: 'branch',
            id: 'brushes:recent',
            label: 'Recent',
            visual: { kind: 'icon', icon: 'fa6-solid:clock-rotate-left' },
            palette: NEUTRAL_PALETTE,
            children: recent.map(b => brushLeaf(b, deps.paletteFor(b.name), deps.load)),
        });
    }
    const packs: WheelNode[] = [];
    for (const pack of deps.packs()) {
        // Dangling member ids resolve to nothing; a pack with no resolvable
        // members contributes no branch rather than an empty fan.
        const members = resolve(pack.members);
        if (members.length === 0) continue;
        packs.push({
            kind: 'branch',
            id: `pack:${pack.id}`,
            label: pack.name,
            visual: { kind: 'icon', icon: pack.icon },
            palette: pack.palette,
            children: members.map(b => brushLeaf(b, pack.palette, deps.load)),
        });
    }
    if (packs.length > 0) {
        out.push({
            kind: 'branch',
            id: 'brushes:library',
            label: 'Library',
            visual: { kind: 'icon', icon: 'fa6-solid:layer-group' },
            palette: NEUTRAL_PALETTE,
            spread: 'full',
            children: packs,
        });
    }
    return out;
}

export function registerBrushesSection(): void {
    paletteSections.register({
        id: 'brushes',
        // The top two thirds: Recent lands on the left one, Library on the
        // right, meeting at screen-up (theta -π/2).
        arc: { a0: (5 * Math.PI) / 6, span: (4 * Math.PI) / 3 },
        nodes: () => brushNodes({
            recentIds: () => recentBrushes.items,
            brushes: () => brushLibrary.brushes,
            packs: () => brushLibrary.packs,
            load: (name, id) => { void brushGraph.loadBrush(name, id); },
            paletteFor: name => brushLibrary.paletteForBrush(name),
        }),
    });
}
