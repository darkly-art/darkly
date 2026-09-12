/**
 * Wheel trees the suites share.
 *
 * A fixture used by one file lives in that file. These are the shapes both the
 * geometry suite and the machine suite need to see, and a tree copied into two
 * files is a tree that drifts between them: the geometry's claim about how a
 * fan is dealt and the machine's claim that a pen can sweep it have to be
 * claims about the same fan.
 */
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';
import type { WheelBranch, WheelLeaf, WheelNode, WheelTree } from '../model';

const paint = { visual: { kind: 'icon', icon: '' }, palette: NEUTRAL_PALETTE } as const;

export const leaf = (id: string): WheelLeaf =>
    ({ kind: 'leaf', id, label: id, ...paint, select: () => {} });

export const branch = (id: string, children: WheelNode[]): WheelBranch =>
    ({ kind: 'branch', id, label: id, ...paint, children });

/** A leaf carrying a chip rather than a glyph, which is what a brush wears and
 *  what makes its label run cost `CHIP_ARC` instead of `MARK`. */
export const brushLeaf = (id: string): WheelLeaf => ({
    kind: 'leaf',
    id,
    label: id,
    visual: { kind: 'brush', name: id, icon: null },
    palette: NEUTRAL_PALETTE,
    select: () => {},
});

/**
 * The Recent branch's shape: the brushes section's two thirds of ring 0 split
 * between Recent and Library, with `names` as Recent's brushes on ring 1.
 *
 * The tightest fan the wheel ships, and so the one a name runs out of arc in
 * first: bounded (unlike the Library's packs, which have the circumference),
 * on the innermost ring a fan can occupy, and holding at most `RECENT_COUNT`
 * members to borrow room from.
 */
export const recentTree = (names: string[]): WheelTree => ({
    sections: [{
        a0: (5 * Math.PI) / 6,
        span: (4 * Math.PI) / 3,
        nodes: [
            branch('recent', names.map(brushLeaf)),
            branch('library', [leaf('p0')]),
        ],
    }],
});
