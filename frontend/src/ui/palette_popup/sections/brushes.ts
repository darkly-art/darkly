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
import { paletteSections, type WheelNode } from '../model';

/** Brushes shown in the Recent fan, of the 12 recents stored: the tail of
 *  the recency list is cold, and a short fan keeps its sectors wide. */
export const RECENT_COUNT = 5;

/** The fields the wheel needs from `BrushInfo` / `BrushPackInfo`, so tests
 *  can hand in plain objects. */
export interface BrushLike { id: string; name: string; icon: string | null }
export interface PackLike { id: string; name: string; icon: string; members: string[] }

export interface BrushDeps {
    recentIds(): string[];
    brushes(): BrushLike[];
    packs(): PackLike[];
    load(name: string, id: string): void;
}

function brushLeaf(b: BrushLike, load: BrushDeps['load']): WheelNode {
    return {
        kind: 'leaf',
        id: `brush:${b.id}`,
        label: b.name,
        visual: { kind: 'brush', name: b.name, icon: b.icon },
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
            children: recent.map(b => brushLeaf(b, deps.load)),
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
            children: members.map(b => brushLeaf(b, deps.load)),
        });
    }
    if (packs.length > 0) {
        out.push({
            kind: 'branch',
            id: 'brushes:library',
            label: 'Library',
            visual: { kind: 'icon', icon: 'fa6-solid:layer-group' },
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
        }),
    });
}
