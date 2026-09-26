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
// The colors section reads its swatch count from the config store for its
// `register*` half; the store reaches the wasm glue, and the builder under
// test takes the count as an injected dep, so it stays out of this file too.
vi.mock('../../../config/store.svelte', () => ({
    config: { number: (_k: string, fallback: number) => fallback },
}));

import { colorNodes, type ColorDeps } from '../sections/colors';
import { brushNodes, RECENT_COUNT, type BrushDeps } from '../sections/brushes';
import type { Color } from '../../../lib/color';
import type { WheelBranch, WheelLeaf } from '../model';
import { NEUTRAL_PALETTE, type PackPalette } from '../../../lib/packPalette';

const RED: Color = { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Color = { r: 0, g: 0, b: 255, a: 255 };

function colorDeps(
    recent: string[],
    count = 5,
): ColorDeps & { set: ReturnType<typeof vi.fn>; wheel: ReturnType<typeof vi.fn> } {
    const set = vi.fn();
    const wheel = vi.fn();
    return {
        recent: () => recent,
        foreground: () => RED,
        background: () => BLUE,
        setForeground: set,
        count: () => count,
        openWheel: wheel,
        set,
        wheel,
    };
}

/** The section leads with the spectrum leaf, so the swatches start at 1. */
const swatches = (nodes: ReturnType<typeof colorNodes>) => nodes.slice(1);

describe('colorNodes', () => {
    it('leads with the spectrum leaf, which the arc puts at the screen-right end', () => {
        // The section's arc runs from theta PI/6 to 5*PI/6 with +y down, and
        // ring 0 lays node i at increasing theta, so index 0 is the rightmost
        // sector: "far right" is the front of the list, not the back.
        const nodes = colorNodes(colorDeps(['#112233ff', '#445566ff']));
        expect(nodes[0].id).toBe('color:spectrum');
        expect(nodes[0].visual).toEqual({ kind: 'spectrum' });
    });

    it('the spectrum leaf opens the color wheel where the gesture committed', () => {
        const deps = colorDeps(['#112233ff', '#445566ff']);
        const nodes = colorNodes(deps);
        (nodes[0] as WheelLeaf).select({ x: 12, y: 34 });
        expect(deps.wheel).toHaveBeenCalledWith({ x: 12, y: 34 });
    });

    it('maps recents to swatch leaves, capped at the configured count', () => {
        const recents = Array.from({ length: 16 }, (_, i) =>
            `#${i.toString(16).padStart(2, '0')}0000ff`);
        // Two counts, so the cap cannot be satisfied by a constant.
        expect(swatches(colorNodes(colorDeps(recents, 5)))).toHaveLength(5);
        const nodes = swatches(colorNodes(colorDeps(recents, 9)));
        expect(nodes).toHaveLength(9);
        expect(nodes.every(n => n.kind === 'leaf' && n.visual.kind === 'swatch')).toBe(true);
        expect((nodes[0] as WheelLeaf).visual).toEqual({ kind: 'swatch', color: recents[0] });
    });

    it('never hands back more swatches than the count, even while seeding', () => {
        // One stored recent leaves the list short, so the pair is seeded; the
        // limit has to apply after that, or a count of 2 yields three swatches.
        expect(swatches(colorNodes(colorDeps(['#112233ff'], 2)))).toHaveLength(2);
    });

    it('seeds the current foreground/background when recents run short', () => {
        const nodes = swatches(colorNodes(colorDeps([])));
        expect(nodes.map(n => (n as WheelLeaf).visual)).toEqual([
            { kind: 'swatch', color: '#ff0000ff' },
            { kind: 'swatch', color: '#0000ffff' },
        ]);
    });

    it('does not seed a duplicate of an already-recent RGB', () => {
        const nodes = swatches(colorNodes(colorDeps(['#ff0000cc'])));
        // Foreground red is already there (alpha ignored); only blue joins.
        expect(nodes.map(n => (n as WheelLeaf).visual)).toEqual([
            { kind: 'swatch', color: '#ff0000cc' },
            { kind: 'swatch', color: '#0000ffff' },
        ]);
    });

    it('select() parses the hex and sets the foreground', () => {
        const deps = colorDeps(['#12345678']);
        const nodes = swatches(colorNodes(deps));
        (nodes[0] as WheelLeaf).select({ x: 0, y: 0 });
        expect(deps.set).toHaveBeenCalledWith({ r: 0x12, g: 0x34, b: 0x56, a: 0x78 });
    });

    it('paints swatches neutral: a color has no pack behind it', () => {
        const nodes = colorNodes(colorDeps(['#112233ff', '#445566ff']));
        expect(nodes.every(n => n.palette === NEUTRAL_PALETTE)).toBe(true);
    });
});

const palette = (chroma: string): PackPalette =>
    ({ chroma, refraction: chroma, surface: chroma });
const P1 = palette('#p1');
const P2 = palette('#p2');
/** What the library store answers for a brush shown outside any pack. */
const STORE = palette('#store');

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
            { id: 'p1', name: 'Dry Media', icon: 'fa6-solid:box', members: ['b2', 'b3'], palette: P1 },
            { id: 'p2', name: 'Empty', icon: 'fa6-solid:box', members: ['gone'], palette: P2 },
        ],
        paletteFor: () => STORE,
        ...over,
        load,
    };
}

describe('brushNodes', () => {
    it('builds exactly Recent then Library, with packs inside Library', () => {
        const nodes = brushNodes(brushDeps());
        expect(nodes.map(n => n.id)).toEqual(['brushes:recent', 'brushes:library']);
        const recent = nodes[0] as WheelBranch;
        expect(recent.children.map(c => c.label)).toEqual(['Charcoal', 'Ink']);
        const library = nodes[1] as WheelBranch;
        expect(library.children.map(c => c.id)).toEqual(['pack:p1']);
        const pack = library.children[0] as WheelBranch;
        expect(pack.children.map(c => c.label)).toEqual(['Charcoal', 'Wash']);
    });

    it('spreads Library around the full circumference', () => {
        const library = brushNodes(brushDeps())[1] as WheelBranch;
        expect(library.spread).toBe('full');
    });

    it('drops dangling member ids and elides empty branches', () => {
        const library = brushNodes(brushDeps())[1] as WheelBranch;
        // p2's only member does not resolve: no branch at all.
        expect(library.children.some(n => n.id === 'pack:p2')).toBe(false);
    });

    it('caps Recent at RECENT_COUNT resolvable brushes', () => {
        const many = Array.from({ length: 9 }, (_, i) =>
            ({ id: `m${i}`, name: `M${i}`, icon: null }));
        const nodes = brushNodes(brushDeps({
            brushes: () => many,
            // A dangling id up front must not cost a shown slot.
            recentIds: () => ['gone', ...many.map(b => b.id)],
        }));
        const recent = nodes[0] as WheelBranch;
        expect(recent.children.map(c => c.label))
            .toEqual(many.slice(0, RECENT_COUNT).map(b => b.name));
    });

    it('omits Recent when nothing recent resolves', () => {
        const nodes = brushNodes(brushDeps({ recentIds: () => ['gone'] }));
        expect(nodes.map(n => n.id)).toEqual(['brushes:library']);
    });

    it('omits Library when no pack resolves', () => {
        const nodes = brushNodes(brushDeps({ packs: () => [] }));
        expect(nodes.map(n => n.id)).toEqual(['brushes:recent']);
    });

    it('carries the brush icon into the leaf visual for fallback rendering', () => {
        const nodes = brushNodes(brushDeps());
        const charcoal = (nodes[0] as WheelBranch).children[0] as WheelLeaf;
        expect(charcoal.visual).toEqual({ kind: 'brush', name: 'Charcoal', icon: 'fa6-solid:pen' });
    });

    it('paints a pack member in that pack\'s colours, not the store\'s answer', () => {
        // Charcoal (b2) sits in two packs. Each fan must show its own, which
        // a `paletteFor`-everywhere implementation would get wrong: the store
        // answers with whichever pack it finds first, for both.
        const nodes = brushNodes(brushDeps({
            packs: () => [
                { id: 'p1', name: 'Dry', icon: '', members: ['b2'], palette: P1 },
                { id: 'p2', name: 'Wet', icon: '', members: ['b2'], palette: P2 },
            ],
        }));
        const packs = (nodes[1] as WheelBranch).children as WheelBranch[];
        expect(packs.map(p => p.palette)).toEqual([P1, P2]);
        expect(packs.map(p => (p.children[0] as WheelLeaf).palette)).toEqual([P1, P2]);
    });

    it('paints a Recent leaf in the palette the library answers with', () => {
        const recent = brushNodes(brushDeps())[0] as WheelBranch;
        expect((recent.children[0] as WheelLeaf).palette).toBe(STORE);
    });

    it('paints the derived branches neutral', () => {
        // Identity, not shape: Recent and Library must keep tracking the same
        // constant the explorer's derived groups wear.
        const nodes = brushNodes(brushDeps());
        expect(nodes.map(n => n.palette)).toEqual([NEUTRAL_PALETTE, NEUTRAL_PALETTE]);
    });

    it('select() loads by name and id', () => {
        const deps = brushDeps();
        const nodes = brushNodes(deps);
        ((nodes[0] as WheelBranch).children[1] as WheelLeaf).select({ x: 0, y: 0 });
        expect(deps.load).toHaveBeenCalledWith('Ink', 'b1');
    });
});
