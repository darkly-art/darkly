/**
 * The brushes half of the palette popup: a Recent branch first, then one
 * branch per pack, each fanning out to brush leaves.
 *
 * Committing a leaf loads the brush through `brushGraph.loadBrush`, which
 * records recents itself.
 */
import { brushGraph } from '../../../state/brush_graph.svelte';
import { brushLibrary } from '../../../state/brush_library.svelte';
import { recentBrushes } from '../../../state/recents.svelte';
import { paletteSections, type WheelNode } from '../model';

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
    const recent = resolve(deps.recentIds());
    if (recent.length > 0) {
        out.push({
            kind: 'branch',
            id: 'brushes:recent',
            label: 'Recent',
            visual: { kind: 'icon', icon: 'fa6-solid:clock-rotate-left' },
            children: recent.map(b => brushLeaf(b, deps.load)),
        });
    }
    for (const pack of deps.packs()) {
        // Dangling member ids resolve to nothing; a pack with no resolvable
        // members contributes no branch rather than an empty fan.
        const members = resolve(pack.members);
        if (members.length === 0) continue;
        out.push({
            kind: 'branch',
            id: `pack:${pack.id}`,
            label: pack.name,
            visual: { kind: 'icon', icon: pack.icon },
            children: members.map(b => brushLeaf(b, deps.load)),
        });
    }
    return out;
}

export function registerBrushesSection(): void {
    paletteSections.register({
        id: 'brushes',
        half: 'top',
        nodes: () => brushNodes({
            recentIds: () => recentBrushes.items,
            brushes: () => brushLibrary.brushes,
            packs: () => brushLibrary.packs,
            load: (name, id) => { void brushGraph.loadBrush(name, id); },
        }),
    });
}
