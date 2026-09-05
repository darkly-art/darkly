/**
 * The palette popup's data model: the tree of things one gesture can reach.
 *
 * Sections are the unit of contribution. Each one owns a half of the
 * innermost ring (colors below, brushes above) and produces its nodes fresh
 * per open; the tree is snapshotted for the gesture's lifetime, because a
 * gesture lasts around a second and geometry shifting under the pen would be
 * worse than briefly stale content.
 *
 * The geometry and the gesture machine consume `WheelNode[]` only and never
 * branch on what a node shows; the popup component's sector renderer is the
 * single consumer that switches on `WheelVisual`. A new item that reuses an
 * existing visual kind is therefore purely additive: a node in a section (or
 * a whole new registered section) and nothing else.
 */

export type WheelVisual =
    | { kind: 'swatch'; color: string }
    | { kind: 'brush'; name: string; icon: string | null }
    | { kind: 'icon'; icon: string };

export interface WheelLeaf {
    kind: 'leaf';
    /** Stable within one open: keys sectors and labels test expectations. */
    id: string;
    label: string;
    visual: WheelVisual;
    /** The committed action. Closes over its own stores; the machine only
     *  ever calls it, never inspects it. */
    select(): void;
}

export interface WheelBranch {
    kind: 'branch';
    id: string;
    label: string;
    visual: WheelVisual;
    children: WheelNode[];
}

export type WheelNode = WheelLeaf | WheelBranch;

/** One open's worth of nodes. Root sectors are indexed bottom-half first
 *  (theta in (0, π), screen-down), then top-half; `rootAt` is the sole
 *  owner of that ordering. */
export interface WheelTree {
    top: WheelNode[];
    bottom: WheelNode[];
}

export interface WheelSection {
    id: string;
    half: 'top' | 'bottom';
    /** Called once per open; the result is snapshotted for the gesture. */
    nodes(): WheelNode[];
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
        const top: WheelNode[] = [];
        const bottom: WheelNode[] = [];
        for (const s of this.#sections.values()) {
            (s.half === 'top' ? top : bottom).push(...s.nodes());
        }
        return { top, bottom };
    }
}

export const paletteSections = new SectionRegistry();

/** Root node `i` in the canonical ring-0 sector order. */
export function rootAt(tree: WheelTree, i: number): WheelNode | undefined {
    return i < tree.bottom.length ? tree.bottom[i] : tree.top[i - tree.bottom.length];
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
