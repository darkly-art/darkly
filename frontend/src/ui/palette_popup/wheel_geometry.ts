/**
 * Polar layout and hit-testing for the palette popup: the circular maze as
 * arithmetic.
 *
 * Pure (no DOM, no runes), testable in Vitest's node environment; same reason
 * `brush_explorer/wheel.ts` sits beside its component. The popup component
 * paints from `layoutWheel` and hit-tests exclusively through `sectorAt`, so
 * paint and hit can never disagree.
 *
 * Coordinates are screen space relative to the wheel center, +y down, so
 * `theta = atan2(dy, dx)` puts the bottom half at (0, π) and the top half at
 * (-π, 0). Sector angles may exceed ±π when a child fan straddles the seam;
 * containment is wrap-aware via `angularOffset`.
 *
 * Reference scale: Krita's popup palette (385 px disc, 72→92 px color
 * donut, `kis_popup_palette.h`) and Blender's pie menus (radius 100, 12 px
 * dead zone, 8-item max, `DNA_userdef_types.h` / `interface_intern.hh`).
 */
import { angularOffset } from '../../lib/angle';
import { rootAt, type WheelNode, type WheelTree } from './model';

/** Dead-zone hub radius: the always-available cancel target. Blender's 12 px
 *  is a direction threshold, not a release target; a pen needs a landable
 *  disc, with Krita's 15 px rotation-snap radius as the low bound. */
export const HUB_R = 28;

/** Radial thickness of each ring.
 *
 *  Set by the deepest thing a ring has to hold, which is a brush leaf's chip
 *  laid along the radius (`.chip` in `PalettePopup.svelte`), and then by
 *  wanting as little of it as that allows: a ring's depth is paid four times
 *  over by the time a painter reaches a brush inside a pack, and it is the
 *  reach of the whole gesture. A name costs nothing here, being 9 px of ink
 *  centred in the band whatever the band is.
 *
 *  Ring 0's outer edge sits well inside Krita's 92 px colour-donut outer, and
 *  deliberately: Krita's ring is the whole of its palette, where this is the
 *  first of up to four, and the wheel's outermost edge is still wider than
 *  Krita's 385 px disc. */
export const RING_T = 48;

/** Angular step per child sector, 22.5°: half of the 45° slots Blender's
 *  8-item pie gives at radius ~100, on a wheel whose fans sit further out
 *  (ring 1's midline is ~110 px) and hold more than eight. */
export const CHILD_STEP = Math.PI / 8;

export interface SectorGeom {
    /** Ring index, 0 innermost. */
    ring: number;
    /** Start angle; the sector spans `[a0, a0 + span)` in increasing theta. */
    a0: number;
    span: number;
    r0: number;
    r1: number;
    /** On the outermost expanded ring hits extend past `r1` to infinity
     *  (Blender's angle-dominant selection): overshooting radially never
     *  loses the highlighted sector. Safe because every gesture starts at
     *  the wheel's own center. `r1` stays the drawn edge. */
    unbounded: boolean;
    /** Tree path of the node this sector shows (see `model.nodeAt`). */
    path: number[];
    node: WheelNode;
}

export type Hit =
    | { kind: 'hub' }
    | { kind: 'gap'; ring: number }
    | { kind: 'sector'; sector: SectorGeom };

/** A `Hit` as a comparable key, for identity-guarding per-pointermove state
 *  writes and for highlight comparison in the component. */
export function hitKey(hit: Hit): string {
    switch (hit.kind) {
        case 'hub': return 'hub';
        case 'gap': return `gap:${hit.ring}`;
        case 'sector': return `sector:${hit.sector.path.join('.')}`;
    }
}

/**
 * Every visible sector for the tree under the current expansion `path`.
 *
 * Ring 0 splits each section's arc evenly among its nodes (Krita's
 * `angleSlice = 360 / slotCount`, per arc). Ring k+1 fans the children of
 * `path[k]` about the parent sector's mid-angle with span
 * `min(π, max(n · CHILD_STEP, parentSpan))`: wide enough to land in, never
 * narrower than the parent, never more than a half turn. A parent with
 * `spread: 'full'` instead hands its children the entire circumference.
 */
export function layoutWheel(tree: WheelTree, path: number[]): SectorGeom[] {
    const out: SectorGeom[] = [];

    let base = 0;
    for (const sec of tree.sections) {
        const n = sec.nodes.length;
        if (n === 0) continue;
        const span = sec.span / n;
        sec.nodes.forEach((node, i) => out.push({
            ring: 0,
            a0: sec.a0 + i * span,
            span,
            r0: HUB_R,
            r1: HUB_R + RING_T,
            unbounded: path.length === 0,
            path: [base + i],
            node,
        }));
        base += n;
    }

    let parentSector = out.find(s => s.ring === 0 && s.path[0] === path[0]);
    for (let k = 0; k < path.length; k++) {
        const parent = parentSector?.node;
        if (!parentSector || parent?.kind !== 'branch' || parent.children.length === 0) break;
        const ring = k + 1;
        const n = parent.children.length;
        const span = parent.spread === 'full'
            ? 2 * Math.PI
            : Math.min(Math.PI, Math.max(n * CHILD_STEP, parentSector.span));
        const child = span / n;
        const a0 = parentSector.a0 + parentSector.span / 2 - span / 2;
        let next: SectorGeom | undefined;
        parent.children.forEach((node, i) => {
            const s: SectorGeom = {
                ring,
                a0: a0 + i * child,
                span: child,
                r0: HUB_R + ring * RING_T,
                r1: HUB_R + (ring + 1) * RING_T,
                unbounded: ring === path.length,
                path: [...parentSector!.path, i],
                node,
            };
            out.push(s);
            if (i === path[k + 1]) next = s;
        });
        parentSector = next;
    }
    return out;
}

/**
 * A sector's angular midpoint.
 *
 * Everything anchored to a sector is placed or oriented along it: the badge
 * sits on it, the growth and pop vectors run down it, and a landscape
 * thumbnail rotated by it lies along the outward radial direction.
 *
 * Angles are screen space with +y down, and CSS `rotate()` is positive
 * clockwise in that same frame, so the value serves as a rotation with no
 * sign juggling. It may exceed ±π where a fan straddles the seam; rotation
 * and direction are inherently mod 2π, so nothing normalizes it.
 */
export function midAngle(s: SectorGeom): number {
    return s.a0 + s.span / 2;
}

/** Height a line of `.pack-name` occupies above its baseline, px. Cap height
 *  rather than line height: what is being centered in the band is the ink,
 *  and a line box is mostly the air around it. */
const LABEL_CAP = 9;

/**
 * The arc a sector's name is set along: its inner circumference, and the
 * direction to travel it.
 *
 * `a0 -> a1` is the drawing order, not the sector's own winding, and that is
 * the whole of the trick. Glyphs stand to the left of a path's direction of
 * travel, so an arc drawn in increasing theta carries letters with their tops
 * pointing away from the center, and one drawn in decreasing theta carries
 * them pointing toward it. On the upper half of the wheel the first is right
 * side up and on the lower half the second is, so a name reverses direction as
 * it crosses the horizontal and stays readable the whole way round.
 *
 * `r` follows from the same split. The ink is centered on the band either way,
 * so the baseline is half a cap height inside the middle when the letters grow
 * outward and half a cap height outside it when they grow inward: a baseline is
 * the foot of the ink, not its centre, and which side the ink is on has just
 * been decided.
 */
export function labelArc(s: SectorGeom): { a0: number; a1: number; r: number } {
    const outward = Math.sin(midAngle(s)) < 0;
    const r = (s.r0 + s.r1) / 2 + (outward ? -LABEL_CAP / 2 : LABEL_CAP / 2);
    // A span of a full turn has no start distinct from its end, and an arc
    // command between coincident points draws nothing at all.
    const span = Math.min(s.span, 2 * Math.PI - 1e-3);
    const a1 = s.a0 + span;
    return outward ? { a0: s.a0, a1, r } : { a0: a1, a1: s.a0, r };
}

/** Space between a sector's mark and its name, px along the arc. The card's
 *  row spends 8 between the two; an arc reads tighter, and this is measured
 *  along a curve rather than across a flex gap. */
const LABEL_GAP = 5;

/** Where a sector's mark and its name sit along its arc.
 *
 *  The two are one run, centered on the arc together the way a card's icon and
 *  label are centered in their row: mark, gap, name. The name's own length has
 *  to be measured off the rendered text (SVG lays nothing out for you), which
 *  is why it arrives as an argument rather than being computed here.
 *
 *  `markTurn` is the direction of travel at the mark, which is what stands it
 *  up the same way the glyphs beside it stand, on either half of the wheel. */
export interface LabelPlacement {
    markA: number;
    markR: number;
    markTurn: number;
    /** Distance along the arc to the middle of the name. */
    textOffset: number;
}

export function labelPlacement(
    s: SectorGeom,
    nameLen: number,
    markW: number,
): LabelPlacement {
    const { a0, a1, r } = labelArc(s);
    const sign = a1 > a0 ? 1 : -1;
    const arcLen = Math.abs(a1 - a0) * r;
    const run = markW + LABEL_GAP + nameLen;
    const start = arcLen / 2 - run / 2;
    const markA = a0 + (sign * (start + markW / 2)) / r;
    return {
        markA,
        // The middle of the band, which is where the ink beside it is centered
        // whichever side of its baseline that ink grows.
        markR: (s.r0 + s.r1) / 2,
        markTurn: markA + (sign * Math.PI) / 2,
        textOffset: start + markW + LABEL_GAP + nameLen / 2,
    };
}

/**
 * Resolve a pointer offset from the wheel center to what it is over./**
 * Resolve a pointer offset from the wheel center to what it is over./**
 * Resolve a pointer offset from the wheel center to what it is over.
 *
 * Radius bands pick the ring, clamped to the deepest expanded one (that ring
 * is unbounded outward); angle picks the sector within it, or `gap` between
 * fans. Rings abut, so a band is exactly a ring. Pure polar math, the way
 * Krita's `calculateColorIndex` resolves its color donut: the DOM is never
 * consulted.
 */
export function sectorAt(layout: SectorGeom[], dx: number, dy: number): Hit {
    const r = Math.hypot(dx, dy);
    if (r < HUB_R) return { kind: 'hub' };
    const theta = Math.atan2(dy, dx);
    let deepest = 0;
    for (const s of layout) if (s.ring > deepest) deepest = s.ring;
    const k = Math.min(Math.floor((r - HUB_R) / RING_T), deepest);
    for (const s of layout) {
        if (s.ring !== k) continue;
        if (angularOffset(theta, s.a0) < s.span) return { kind: 'sector', sector: s };
    }
    return { kind: 'gap', ring: k };
}

/**
 * The maze rule: the expansion chain after the pointer lands on `hit`.
 *
 * - hub retracts everything;
 * - a gap at ring k keeps rings through k and retracts deeper ones (on the
 *   outermost ring that degenerates to "unchanged", so overshooting into a
 *   gap never collapses the fan being aimed at);
 * - a branch sector becomes the chain through its ring, expanding its
 *   children and collapsing any sibling subtree in the same assignment;
 * - a leaf terminates the chain at its ring.
 *
 * Threading back inward needs no special case: a sector or gap at ring k
 * truncates the chain to k entries, which is exactly "retrace the rings you
 * came through".
 */
export function advance(path: number[], hit: Hit): number[] {
    switch (hit.kind) {
        case 'hub': return [];
        case 'gap': return path.slice(0, hit.ring);
        case 'sector':
            return hit.sector.node.kind === 'branch'
                ? hit.sector.path
                : hit.sector.path.slice(0, -1);
    }
}
