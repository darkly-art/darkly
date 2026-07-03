/**
 * Pure tiling-tree model for the workspace docking system.
 *
 * Ported from Graphite's `WorkspacePanelLayout`
 * (`editor/src/messages/portfolio/utility_types.rs`, functions
 * `insert_split_adjacent` / `prune` / `split_panel_group` / `find_group`).
 * We keep the same "axis implicit from depth" trick — a `split`'s orientation
 * is `depth % 2 === 0 ? horizontal : vertical`, alternating with nesting depth,
 * so no HSplit/VSplit variant is ever stored.
 *
 * Everything here is pure (no `$state`, no DOM) so it can be unit-tested and
 * shared verbatim across every open window. The reactive store
 * (`workspaces.svelte.ts`) clones a plain snapshot, calls one of these ops,
 * then reassigns the root to trigger Svelte reactivity.
 */

/** A top-level dockable panel. Additive: register a new one in
 *  `registerPanels.ts` and extend this union.
 *
 *  `document` is the canvas/viewport itself — a first-class panel (as in
 *  Graphite), so the whole window tiles and horizontal splitting (canvas |
 *  panels) is meaningful. It is a non-closable, non-poppable singleton kept
 *  present by {@link ensureDocument}. */
export type PanelType = 'document' | 'layers' | 'properties';

export interface PanelGroupState {
    tabs: PanelType[];
    activeTabIndex: number;
}

/** A node in the split tree: either a tab-stack leaf (`group`) or an
 *  n-ary `split` whose axis is implied by its depth. */
export type Subdivision =
    | { kind: 'group'; id: number; state: PanelGroupState }
    | { kind: 'split'; children: SplitChild[] };

/** A child slot inside a `split`. `size` is a flex-grow weight; siblings'
 *  sizes sum to ≈1. */
export interface SplitChild {
    subdivision: Subdivision;
    size: number;
}

export interface WorkspaceLayout {
    root: Subdivision;
}

export type DockingSplitDirection = 'left' | 'right' | 'top' | 'bottom';

/** Even share for a fresh adjacent split when no source-size hint is given. */
const DEFAULT_SPLIT_SHARE = 0.5;

/** Preferred minimum pixel extent of a panel slot, enforced (and relaxed when
 *  the region is too small to satisfy every slot) by the gutter-resize clamp. */
export const MIN_PANEL_PX = 80;

/** Root-row share given to the canvas (`document`) in the default layout; the
 *  rest goes to the Layers/Properties column. Mirrors Graphite's
 *  `DOCUMENT_PANEL_SHARE`. */
const DOCUMENT_SHARE = 0.8;

// ---------------------------------------------------------------------------
// Construction helpers
// ---------------------------------------------------------------------------

export function makeGroup(id: number, tabs: PanelType[], activeTabIndex = 0): Subdivision {
    return { kind: 'group', id, state: { tabs, activeTabIndex } };
}

/** The main window's default arrangement — Graphite's shape:
 *
 *  ```
 *  Row [ Document | Column[ Layers 0.6, Properties 0.4 ] ]
 *  ```
 *
 *  Root is a `horizontal` split (depth 0): the canvas (`document`) takes
 *  {@link DOCUMENT_SHARE} on the left, and a `vertical` sub-split (depth 1) of
 *  Layers-over-Properties takes the rest on the right. The 0.6/0.4 ratio
 *  preserves the historical Layers/Properties proportion. */
export function defaultMainLayout(docId: number, layersId: number, propsId: number): WorkspaceLayout {
    return {
        root: {
            kind: 'split',
            children: [
                { size: DOCUMENT_SHARE, subdivision: makeGroup(docId, ['document']) },
                {
                    size: 1 - DOCUMENT_SHARE,
                    subdivision: {
                        kind: 'split',
                        children: [
                            { size: 0.6, subdivision: makeGroup(layersId, ['layers']) },
                            { size: 0.4, subdivision: makeGroup(propsId, ['properties']) },
                        ],
                    },
                },
            ],
        },
    };
}

export function cloneSubdivision(node: Subdivision): Subdivision {
    if (node.kind === 'group') {
        return { kind: 'group', id: node.id, state: { tabs: [...node.state.tabs], activeTabIndex: node.state.activeTabIndex } };
    }
    return { kind: 'split', children: node.children.map((c) => ({ size: c.size, subdivision: cloneSubdivision(c.subdivision) })) };
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

export function findGroup(node: Subdivision, id: number): Extract<Subdivision, { kind: 'group' }> | null {
    if (node.kind === 'group') return node.id === id ? node : null;
    for (const child of node.children) {
        const found = findGroup(child.subdivision, id);
        if (found) return found;
    }
    return null;
}

export function firstGroupId(node: Subdivision): number | null {
    if (node.kind === 'group') return node.id;
    for (const child of node.children) {
        const found = firstGroupId(child.subdivision);
        if (found !== null) return found;
    }
    return null;
}

/** Every `PanelType` present anywhere in the tree (in traversal order). */
export function collectPanelTypes(node: Subdivision): PanelType[] {
    if (node.kind === 'group') return [...node.state.tabs];
    return node.children.flatMap((c) => collectPanelTypes(c.subdivision));
}

/** True when the tree contains no tabs at all — an unhittable empty workspace. */
export function isEmptyLayout(node: Subdivision): boolean {
    return collectPanelTypes(node).length === 0;
}

/** Resolve a `split` by its path of child indices from the root (root itself is
 *  `[]`). Used to address a specific split's gutter for resize without giving
 *  splits ids. Returns null if the path doesn't land on a `split`. */
export function resolveSplitByPath(root: Subdivision, path: number[]): Extract<Subdivision, { kind: 'split' }> | null {
    let node: Subdivision = root;
    for (const idx of path) {
        if (node.kind !== 'split') return null;
        const child = node.children[idx];
        if (!child) return null;
        node = child.subdivision;
    }
    return node.kind === 'split' ? node : null;
}

function containsGroup(node: Subdivision, id: number): boolean {
    if (node.kind === 'group') return node.id === id;
    return node.children.some((c) => containsGroup(c.subdivision, id));
}

// ---------------------------------------------------------------------------
// Tab-level ops (mutate in place)
// ---------------------------------------------------------------------------

/** Remove `tab` from the given group. Adjusts `activeTabIndex` so the same
 *  visible tab stays active where possible. Leaves the (possibly empty) group
 *  in place — callers `prune` afterwards. */
export function removeTab(root: Subdivision, groupId: number, tab: PanelType): boolean {
    const group = findGroup(root, groupId);
    if (!group) return false;
    const idx = group.state.tabs.indexOf(tab);
    if (idx === -1) return false;
    group.state.tabs.splice(idx, 1);
    if (idx < group.state.activeTabIndex) group.state.activeTabIndex--;
    group.state.activeTabIndex = Math.max(0, Math.min(group.state.activeTabIndex, group.state.tabs.length - 1));
    return true;
}

/** Insert `tab` into the group (deduped — panels are singletons) and make it
 *  active. `atIndex` clamps into range; omit to append. */
export function insertTab(root: Subdivision, groupId: number, tab: PanelType, atIndex?: number): boolean {
    const group = findGroup(root, groupId);
    if (!group) return false;
    if (group.state.tabs.includes(tab)) {
        group.state.activeTabIndex = group.state.tabs.indexOf(tab);
        return true;
    }
    const at = atIndex === undefined ? group.state.tabs.length : Math.max(0, Math.min(atIndex, group.state.tabs.length));
    group.state.tabs.splice(at, 0, tab);
    group.state.activeTabIndex = at;
    return true;
}

/** Reorder a tab within its own group. */
export function reorderTab(root: Subdivision, groupId: number, from: number, to: number): boolean {
    const group = findGroup(root, groupId);
    if (!group) return false;
    const tabs = group.state.tabs;
    if (from < 0 || from >= tabs.length) return false;
    const clampedTo = Math.max(0, Math.min(to, tabs.length - 1));
    const [moved] = tabs.splice(from, 1);
    tabs.splice(clampedTo, 0, moved);
    group.state.activeTabIndex = clampedTo;
    return true;
}

// ---------------------------------------------------------------------------
// Split ops (mutate in place)
// ---------------------------------------------------------------------------

/** Split `targetGroupId` by inserting a new group carrying `tabs` on the given
 *  side. `left`/`top` insert before; `left`/`right` want a horizontal axis.
 *  Returns the new group's id, or null if the target wasn't found. */
export function splitPanelGroup(
    root: Subdivision,
    targetGroupId: number,
    direction: DockingSplitDirection,
    tabs: PanelType[],
    activeTabIndex: number,
    newId: number,
    sourceSlotSize?: number,
): number | null {
    const newChild: SplitChild = {
        subdivision: makeGroup(newId, tabs, activeTabIndex),
        size: sourceSlotSize ?? DEFAULT_SPLIT_SHARE,
    };
    const insertBefore = direction === 'left' || direction === 'top';
    const needsHorizontal = direction === 'left' || direction === 'right';
    const ok = insertSplitAdjacent(root, targetGroupId, newChild, insertBefore, needsHorizontal, 0, sourceSlotSize);
    return ok ? newId : null;
}

/** Insert `newChild` adjacent to the group `targetId`, recursing to the
 *  deepest split whose axis matches `needsHorizontal`. If the target is a
 *  direct child of a mismatched-axis split, wrap it in a new sub-split
 *  (flipping axis one level deeper). Ported from Graphite. */
export function insertSplitAdjacent(
    node: Subdivision,
    targetId: number,
    newChild: SplitChild,
    insertBefore: boolean,
    needsHorizontal: boolean,
    depth: number,
    sourceSlotSize?: number,
): boolean {
    if (node.kind !== 'split') return false;
    const children = node.children;

    const isHorizontal = depth % 2 === 0;
    const directionMatches = isHorizontal === needsHorizontal;

    const containingIndex = children.findIndex((c) => containsGroup(c.subdivision, targetId));
    if (containingIndex === -1) return false;

    const containing = children[containingIndex];
    const targetIsDirectChild = containing.subdivision.kind === 'group' && containing.subdivision.id === targetId;

    if (targetIsDirectChild) {
        if (directionMatches) {
            const targetWillBePruned =
                containing.subdivision.kind === 'group' && containing.subdivision.state.tabs.length === 0;
            if (targetWillBePruned) {
                newChild.size = containing.size;
            } else if (sourceSlotSize !== undefined) {
                newChild.size = sourceSlotSize;
            } else {
                const total = containing.size;
                containing.size = total * DEFAULT_SPLIT_SHARE;
                newChild.size = total * (1 - DEFAULT_SPLIT_SHARE);
            }
            const insertIndex = insertBefore ? containingIndex : containingIndex + 1;
            children.splice(insertIndex, 0, newChild);
        } else {
            // Axis mismatch: wrap the target in a fresh sub-split one depth
            // deeper (which flips its axis to the one we need).
            const oldSubdivision = containing.subdivision;
            const oldShare = DEFAULT_SPLIT_SHARE;
            const oldChild: SplitChild = { subdivision: oldSubdivision, size: oldShare };
            newChild.size = 1 - oldShare;
            const subChildren = insertBefore ? [newChild, oldChild] : [oldChild, newChild];
            containing.subdivision = { kind: 'split', children: subChildren };
        }
        return true;
    }

    return insertSplitAdjacent(
        containing.subdivision,
        targetId,
        newChild,
        insertBefore,
        needsHorizontal,
        depth + 1,
        sourceSlotSize,
    );
}

// ---------------------------------------------------------------------------
// Prune (mutate in place)
// ---------------------------------------------------------------------------

/** Normalize invariants after any mutation: drop empty groups and empty
 *  splits, flatten a redundant single-child `split` wrapper, and renormalize
 *  sibling sizes back to ≈1.
 *
 *  **The flatten is axis-preserving by construction.** When a child slot holds
 *  a `split` with exactly one child that is *itself* a `split`, the outer
 *  wrapper is redundant (a one-child split just renders its child full-size).
 *  We splice the *grandchildren* directly into this level — a shift of **two**
 *  depth levels — so their implicit axis (parity of depth) is unchanged and the
 *  panels do not visually re-orient. Promoting the inner split instead (a shift
 *  of one level) would rotate the subtree, which is exactly what we avoid.
 *  For the same reason we do NOT collapse a single *group*-in-`split`: it is not
 *  redundant nesting the way split-in-split is, and touching it buys nothing. */
export function prune(node: Subdivision): void {
    if (node.kind !== 'split') return;
    const children = node.children;

    for (const child of children) prune(child.subdivision);

    // Remove empty panel groups.
    for (let i = children.length - 1; i >= 0; i--) {
        const s = children[i].subdivision;
        if (s.kind === 'group' && s.state.tabs.length === 0) children.splice(i, 1);
    }
    // Remove empty splits.
    for (let i = children.length - 1; i >= 0; i--) {
        const s = children[i].subdivision;
        if (s.kind === 'split' && s.children.length === 0) children.splice(i, 1);
    }

    // Flatten single-`split`-in-`split`, rescaling to preserve proportions.
    let i = 0;
    while (i < children.length) {
        const outer = children[i].subdivision;
        if (outer.kind !== 'split' || outer.children.length !== 1) {
            i++;
            continue;
        }
        const only = outer.children[0].subdivision;
        if (only.kind !== 'split') {
            i++;
            continue;
        }
        const outerSize = children[i].size;
        children.splice(i, 1);
        const innerChildren = only.children;
        const innerTotal = innerChildren.reduce((a, c) => a + c.size, 0);
        innerChildren.forEach((grandchild, offset) => {
            grandchild.size = innerTotal > 0 ? (grandchild.size / innerTotal) * outerSize : outerSize;
            children.splice(i + offset, 0, grandchild);
        });
    }

    renormalize(children);
}

/** Rescale a split's children so their sizes sum to ≈1 (dock/prune cycles
 *  compound shrinkage otherwise). */
export function renormalize(children: SplitChild[]): void {
    const total = children.reduce((a, c) => a + c.size, 0);
    if (total > 0 && Math.abs(total - 1) > 0.001) {
        for (const child of children) child.size /= total;
    }
}

// ---------------------------------------------------------------------------
// Fold + load
// ---------------------------------------------------------------------------

/** Merge orphaned panels (from pop-out windows that can't be restored, or a
 *  closed pop-out) into the main root's first group as tabs. Lossless — no
 *  arrangement is preserved, but every panel returns. Deduped: a panel already
 *  present in main is skipped. */
export function foldPanelsIntoMain(mainRoot: Subdivision, panels: PanelType[]): void {
    const present = new Set(collectPanelTypes(mainRoot));
    let targetGroup = firstGroupId(mainRoot);
    for (const p of panels) {
        if (present.has(p)) continue;
        if (targetGroup === null) break;
        insertTab(mainRoot, targetGroup, p);
        present.add(p);
    }
}

/** Guarantee the singleton `document` panel exists. If absent (a corrupted
 *  save, or a layout persisted before the canvas became a panel), prepend it as
 *  the root row's first child so the canvas sits left of everything else, then
 *  renormalize. Root is always a `split` by contract. */
export function ensureDocument(root: Subdivision, docId: number): void {
    if (root.kind !== 'split') return;
    if (collectPanelTypes(root).includes('document')) return;
    root.children.unshift({ size: DOCUMENT_SHARE, subdivision: makeGroup(docId, ['document']) });
    renormalize(root.children);
}

/** Reassign every group id sequentially from 0 and return the next free id.
 *  Guarantees uniqueness after folding independently-numbered trees. */
export function renumber(node: Subdivision, start = 0): number {
    let next = start;
    const walk = (n: Subdivision) => {
        if (n.kind === 'group') {
            n.id = next++;
        } else {
            for (const c of n.children) walk(c.subdivision);
        }
    };
    walk(node);
    return next;
}

const KNOWN_PANEL_TYPES: readonly PanelType[] = ['document', 'layers', 'properties'];

function stripUnknownTabs(node: Subdivision): void {
    if (node.kind === 'group') {
        node.state.tabs = node.state.tabs.filter((t) => (KNOWN_PANEL_TYPES as readonly string[]).includes(t));
        node.state.activeTabIndex = Math.max(0, Math.min(node.state.activeTabIndex, node.state.tabs.length - 1));
    } else {
        for (const c of node.children) stripUnknownTabs(c.subdivision);
    }
}

interface PersistedShape {
    workspaces: { layout: WorkspaceLayout }[];
}

function isSubdivision(v: unknown): v is Subdivision {
    if (typeof v !== 'object' || v === null) return false;
    const n = v as { kind?: unknown };
    if (n.kind === 'group') {
        const g = v as { state?: { tabs?: unknown; activeTabIndex?: unknown } };
        return Array.isArray(g.state?.tabs) && typeof g.state?.activeTabIndex === 'number';
    }
    if (n.kind === 'split') {
        const s = v as { children?: unknown };
        return Array.isArray(s.children) && s.children.every((c) => isSubdivision((c as { subdivision?: unknown })?.subdivision));
    }
    return false;
}

/**
 * Parse persisted layout JSON into a single, valid main-window root — the whole
 * validation gauntlet in one place.
 *
 * Pop-out windows can't be reopened without a user gesture, so any persisted
 * pop-out trees are **folded back into the main tree** here rather than
 * recreated. Steps: parse → per-workspace strip unknown `PanelType`s → prune
 * (a group emptied by stripping becomes an unhittable zero) → fold non-main
 * workspaces into main → empty ⇒ default → ensure the singleton `document`
 * panel is present (self-heals a layout saved before the canvas was a panel).
 * Malformed JSON ⇒ default.
 */
export function loadOrDefault(raw: string | null): { root: Subdivision; nextGroupId: number } {
    const fallback = () => {
        const layout = defaultMainLayout(0, 1, 2);
        return { root: layout.root, nextGroupId: 3 };
    };
    if (raw === null) return fallback();

    let parsed: unknown;
    try {
        parsed = JSON.parse(raw);
    } catch {
        return fallback();
    }

    const workspaces = (parsed as Partial<PersistedShape>)?.workspaces;
    if (!Array.isArray(workspaces) || workspaces.length === 0) return fallback();

    const roots: Subdivision[] = [];
    for (const w of workspaces) {
        const root = w?.layout?.root;
        if (!isSubdivision(root)) continue;
        // Root must be a split (parity math assumes it); wrap a bare group.
        const normalized: Subdivision = root.kind === 'split' ? root : { kind: 'split', children: [{ size: 1, subdivision: root }] };
        stripUnknownTabs(normalized);
        prune(normalized);
        roots.push(normalized);
    }

    if (roots.length === 0) return fallback();

    const mainRoot = roots[0];
    const orphans = roots.slice(1).flatMap(collectPanelTypes);
    foldPanelsIntoMain(mainRoot, orphans);
    prune(mainRoot);

    // Empty check runs *before* injecting Document: a fully-stripped layout
    // should reset to the full default (Document + Layers + Properties), not a
    // lone canvas.
    if (isEmptyLayout(mainRoot)) return fallback();

    // Renumber first so ensureDocument can hand the injected group a free id.
    const usedIds = renumber(mainRoot, 0);
    ensureDocument(mainRoot, usedIds);
    prune(mainRoot);
    const nextGroupId = renumber(mainRoot, 0);
    return { root: mainRoot, nextGroupId };
}
