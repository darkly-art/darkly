/**
 * The palette popup's data model: the tree of things one gesture can reach.
 *
 * Sections are the unit of contribution. Each one owns an arc of the
 * innermost ring (today: colors the bottom-center third, brushes the top two
 * thirds) and produces its nodes fresh per open; the tree is snapshotted for
 * the gesture's lifetime, because a gesture lasts around a second and
 * geometry shifting under the pen would be worse than briefly stale content.
 *
 * The geometry and the gesture machine consume `WheelNode[]` only and never
 * branch on what a node shows; the popup component's sector renderer is the
 * single consumer that switches on `WheelVisual`. A new item that reuses an
 * existing visual kind is therefore purely additive: a node in a section (or
 * a whole new registered section) and nothing else.
 */

import type { PackPalette } from '../../lib/packPalette';

export type WheelVisual =
    | { kind: 'swatch'; color: string }
    | { kind: 'brush'; name: string; icon: string | null }
    | { kind: 'icon'; icon: string };

/** The colours a node is drawn in: its pack's, or `NEUTRAL_PALETTE` for one
 *  no pack stands behind (Recent, Library, a color swatch).
 *
 *  Resolved when the tree is snapshotted, not at render time, because the
 *  snapshot is frozen for the gesture and a component reaching back into the
 *  library store would be reading live state through a frozen tree. It is a
 *  property of the node rather than of its `WheelVisual` so that a new visual
 *  kind stays purely additive: the visual says what a node shows, this says
 *  whose it is, and the renderer applies it once without consulting either. */
export interface WheelPainted {
    palette: PackPalette;
}

export interface WheelLeaf extends WheelPainted {
    kind: 'leaf';
    /** Stable within one open: keys sectors and labels test expectations. */
    id: string;
    label: string;
    visual: WheelVisual;
    /** The committed action. Closes over its own stores; the machine only
     *  ever calls it, never inspects it. */
    select(): void;
}

export interface WheelBranch extends WheelPainted {
    kind: 'branch';
    id: string;
    label: string;
    visual: WheelVisual;
    children: WheelNode[];
    /** Children normally fan about the parent's mid-angle, never wider than
     *  a half turn; 'full' spreads them around the entire circumference,
     *  for branches whose child count is unbounded (brush packs). */
    spread?: 'full';
}

export type WheelNode = WheelLeaf | WheelBranch;

/** The name a node shows along its arc, or null for one that shows none.
 *
 *  A brush is identified by its stroke and a swatch by its own colour; only a
 *  branch, whose mark is generic (packs ship a handful of shared `mdi:` marks),
 *  has to be named. Asked here rather than at each consumer because the
 *  geometry needs it to know how much arc a sector wants and the component
 *  needs it to draw one, and those two must never disagree about which sectors
 *  carry text. */
export function wheelLabel(node: WheelNode): string | null {
    return node.kind === 'branch' && node.visual.kind === 'icon' ? node.label : null;
}

/** An arc of ring 0: spans `[a0, a0 + span)` in increasing screen theta
 *  (+y down), wrap-aware, so it may cross the ±π seam. */
export interface WheelArc {
    a0: number;
    span: number;
}

export interface WheelSection {
    id: string;
    /** The slice of ring 0 this section's nodes subdivide evenly. */
    arc: WheelArc;
    /** Called once per open; the result is snapshotted for the gesture. */
    nodes(): WheelNode[];
}

/** One open's worth of nodes, placed. Root sectors are indexed by flattening
 *  the sections' nodes in registration order; `rootAt` is the sole owner of
 *  that ordering. */
export interface PlacedSection extends WheelArc {
    nodes: WheelNode[];
}

export interface WheelTree {
    sections: PlacedSection[];
}

/** Keyed by id so re-registration replaces rather than duplicates, the same
 *  way `actions.register` behaves. */
class SectionRegistry {
    #sections = new Map<string, WheelSection>();

    register(section: WheelSection): void {
        this.#sections.set(section.id, section);
    }

    /** Materialize every section's nodes for one open. */
    snapshot(): WheelTree {
        return {
            sections: [...this.#sections.values()].map(s => ({
                a0: s.arc.a0,
                span: s.arc.span,
                nodes: s.nodes(),
            })),
        };
    }
}

export const paletteSections = new SectionRegistry();

/** Every distinct label the tree will ever draw, in first-seen order.
 *
 *  Walked once per open so that every name can be measured before the ring
 *  holding it is drawn: a fan's widening is decided from these widths, and a
 *  ring that appeared at one width and then changed as its measurements landed
 *  would move under the pen. Distinct, because a width belongs to a string. */
export function wheelLabels(tree: WheelTree): string[] {
    const out: string[] = [];
    const seen = new Set<string>();
    const walk = (nodes: WheelNode[]) => {
        for (const node of nodes) {
            const label = wheelLabel(node);
            if (label !== null && !seen.has(label)) {
                seen.add(label);
                out.push(label);
            }
            if (node.kind === 'branch') walk(node.children);
        }
    };
    for (const sec of tree.sections) walk(sec.nodes);
    return out;
}

/** Root node `i` in the canonical ring-0 sector order. */
export function rootAt(tree: WheelTree, i: number): WheelNode | undefined {
    for (const sec of tree.sections) {
        if (i < sec.nodes.length) return sec.nodes[i];
        i -= sec.nodes.length;
    }
    return undefined;
}

/** The node a geometry path addresses, or undefined for a dangling path. */
export function nodeAt(tree: WheelTree, path: number[]): WheelNode | undefined {
    if (path.length === 0) return undefined;
    let node = rootAt(tree, path[0]);
    for (let d = 1; d < path.length && node; d++) {
        node = node.kind === 'branch' ? node.children[path[d]] : undefined;
    }
    return node;
}
