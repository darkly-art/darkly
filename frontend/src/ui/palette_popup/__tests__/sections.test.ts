import { describe, it, expect, vi } from 'vitest';

// The section modules import the live stores for their `register*` halves;
// the builders under test take injected deps, so the stores can be inert.
vi.mock('../../../state/app.svelte', () => ({ app: {} }));
vi.mock('../../../state/recents.svelte', () => ({
    recentColors: { items: [] },
    recentBrushes: { items: [] },
}));
vi.mock('../../../state/brush_graph.svelte', () => ({ brushGraph: {} }));
vi.mock('../../../state/brush_library.svelte', () => ({ brushLibrary: {} }));

import { colorNodes, SWATCH_COUNT, type ColorDeps } from '../sections/colors';
import { brushNodes, type BrushDeps } from '../sections/brushes';
import type { Color } from '../../../state/app.svelte';
import type { WheelBranch, WheelLeaf } from '../model';

const RED: Color = { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Color = { r: 0, g: 0, b: 255, a: 255 };

function colorDeps(recent: string[]): ColorDeps & { set: ReturnType<typeof vi.fn> } {
    const set = vi.fn();
    return {
        recent: () => recent,
        foreground: () => RED,
        background: () => BLUE,
        setForeground: set,
        set,
    };
}

describe('colorNodes', () => {
    it('maps recents to swatch leaves, capped at SWATCH_COUNT', () => {
        const recents = Array.from({ length: 16 }, (_, i) =>
            `#${i.toString(16).padStart(2, '0')}0000ff`);
        const nodes = colorNodes(colorDeps(recents));
        expect(nodes).toHaveLength(SWATCH_COUNT);
        expect(nodes.every(n => n.kind === 'leaf' && n.visual.kind === 'swatch')).toBe(true);
        expect((nodes[0] as WheelLeaf).visual).toEqual({ kind: 'swatch', color: recents[0] });
    });

    it('seeds the current foreground/background when recents run short', () => {
        const nodes = colorNodes(colorDeps([]));
        expect(nodes.map(n => (n as WheelLeaf).visual)).toEqual([
            { kind: 'swatch', color: '#ff0000ff' },
            { kind: 'swatch', color: '#0000ffff' },
        ]);
    });

    it('does not seed a duplicate of an already-recent RGB', () => {
        const nodes = colorNodes(colorDeps(['#ff0000cc']));
        // Foreground red is already there (alpha ignored); only blue joins.
        expect(nodes.map(n => (n as WheelLeaf).visual)).toEqual([
            { kind: 'swatch', color: '#ff0000cc' },
            { kind: 'swatch', color: '#0000ffff' },
        ]);
    });

    it('select() parses the hex and sets the foreground', () => {
        const deps = colorDeps(['#12345678']);
        const nodes = colorNodes(deps);
        (nodes[0] as WheelLeaf).select();
        expect(deps.set).toHaveBeenCalledWith({ r: 0x12, g: 0x34, b: 0x56, a: 0x78 });
    });
});

function brushDeps(over: Partial<BrushDeps> = {}): BrushDeps & { load: ReturnType<typeof vi.fn> } {
    const load = vi.fn();
    return {
        recentIds: () => ['b2', 'b1'],
        brushes: () => [
            { id: 'b1', name: 'Ink', icon: null },
            { id: 'b2', name: 'Charcoal', icon: 'fa6-solid:pen' },
            { id: 'b3', name: 'Wash', icon: null },
        ],
        packs: () => [
            { id: 'p1', name: 'Dry Media', icon: 'fa6-solid:box', members: ['b2', 'b3'] },
            { id: 'p2', name: 'Empty', icon: 'fa6-solid:box', members: ['gone'] },
        ],
        ...over,
        load,
    };
}

describe('brushNodes', () => {
    it('builds Recent first, then packs in order, as branches of brush leaves', () => {
        const nodes = brushNodes(brushDeps());
        expect(nodes.map(n => n.id)).toEqual(['brushes:recent', 'pack:p1']);
        const recent = nodes[0] as WheelBranch;
        expect(recent.children.map(c => c.label)).toEqual(['Charcoal', 'Ink']);
        const pack = nodes[1] as WheelBranch;
        expect(pack.children.map(c => c.label)).toEqual(['Charcoal', 'Wash']);
    });

    it('drops dangling member ids and elides empty branches', () => {
        const nodes = brushNodes(brushDeps());
        // p2's only member does not resolve: no branch at all.
        expect(nodes.some(n => n.id === 'pack:p2')).toBe(false);
    });

    it('omits Recent when nothing recent resolves', () => {
        const nodes = brushNodes(brushDeps({ recentIds: () => ['gone'] }));
        expect(nodes.map(n => n.id)).toEqual(['pack:p1']);
    });

    it('carries the brush icon into the leaf visual for fallback rendering', () => {
        const nodes = brushNodes(brushDeps());
        const charcoal = (nodes[0] as WheelBranch).children[0] as WheelLeaf;
        expect(charcoal.visual).toEqual({ kind: 'brush', name: 'Charcoal', icon: 'fa6-solid:pen' });
    });

    it('select() loads by name and id', () => {
        const deps = brushDeps();
        const nodes = brushNodes(deps);
        ((nodes[0] as WheelBranch).children[1] as WheelLeaf).select();
        expect(deps.load).toHaveBeenCalledWith('Ink', 'b1');
    });
});
