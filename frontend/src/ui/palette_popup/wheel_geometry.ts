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
 * Reference scale, cited in docs/plans/palette-popup.md: Krita's popup
 * palette (385 px disc, 72→92 px color donut, `kis_popup_palette.h`) and
 * Blender's pie menus (radius 100, 12 px dead zone, 8-item max,
 * `DNA_userdef_types.h` / `interface_intern.hh`).
 */
import { angularOffset } from '../../lib/angle';
import { rootAt, type WheelNode, type WheelTree } from './model';

/** Dead-zone hub radius: the always-available cancel target. Blender's 12 px
 *  is a direction threshold, not a release target; a pen needs a landable
 *  disc, with Krita's 15 px rotation-snap radius as the low bound. */
export const HUB_R = 28;

/** Radial thickness of each ring. Ring 0's outer edge (96 px) sits at
 *  Krita's 92 px color-donut outer and just above Blender's 100 px pie
 *  radius at ring 1's midline. */
export const RING_T = 68;

/** Angular step per child sector, 22.5°: Blender's 8-item pie gives 45°
 *  slots at radius ~100; at ring 1's midline (~130 px) 22.5° subtends about
 *  the same arc length. */
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
 * Ring 0 splits each half evenly among its nodes (Krita's
 * `angleSlice = 360 / slotCount`, per half). Ring k+1 fans the children of
 * `path[k]` about the parent sector's mid-angle with span
 * `min(π, max(n · CHILD_STEP, parentSpan))`: wide enough to land in, never
 * narrower than the parent, never more than a half turn.
 */
export function layoutWheel(tree: WheelTree, path: number[]): SectorGeom[] {
    const out: SectorGeom[] = [];

    const halves = [
        { nodes: tree.bottom, base: 0, start: 0 },
        { nodes: tree.top, base: tree.bottom.length, start: -Math.PI },
    ];
    for (const half of halves) {
        const n = half.nodes.length;
        if (n === 0) continue;
        const span = Math.PI / n;
        half.nodes.forEach((node, i) => out.push({
            ring: 0,
            a0: half.start + i * span,
            span,
            r0: HUB_R,
            r1: HUB_R + RING_T,
            unbounded: path.length === 0,
            path: [half.base + i],
            node,
        }));
    }

    let parentSector = out.find(s => s.ring === 0 && s.path[0] === path[0]);
    for (let k = 0; k < path.length; k++) {
        const parent = parentSector?.node;
        if (!parentSector || parent?.kind !== 'branch' || parent.children.length === 0) break;
        const ring = k + 1;
        const n = parent.children.length;
        const span = Math.min(Math.PI, Math.max(n * CHILD_STEP, parentSector.span));
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
 * Resolve a pointer offset from the wheel center to what it is over.
 *
 * Radius bands pick the ring, clamped to the deepest expanded one (that ring
 * is unbounded outward); angle picks the sector within it, or `gap` between
 * fans. Pure polar math, the way Krita's `calculateColorIndex` resolves its
 * color donut: the DOM is never consulted.
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
