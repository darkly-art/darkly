/**
 * Structural queries over a serialized layer tree, and the rule for picking a
 * replacement row when the active one disappears.
 *
 * The reselection rule follows GIMP and Krita, which agree on it exactly:
 * nearest surviving sibling **below** → nearest surviving sibling **above** →
 * the **parent** → nothing, scoped to the dead row's own sibling list and
 * computed against the tree as it was *before* the removal.
 *
 * - GIMP: `gimp_item_tree_remove_item`, `app/core/gimpitemtree.c`: captures the
 *   index before removal, re-reads that index in the shrunk container (= the
 *   sibling below), `CLAMP`s to the last child (= the sibling above, when the
 *   removed row was bottom-most), else selects the parent.
 * - Krita: `LayerBox::slotAboutToRemoveRows`,
 *   `plugins/dockers/layerdocker/LayerBox.cpp`: row `end + 1`, else row
 *   `start - 1`, else an invalid index, which `KisNodeModel::setData` resolves
 *   to the captured parent.
 *
 * Making the reselected row visible also follows GIMP, whose tree view expands
 * the parent of every newly selected item
 * (`gimp_container_tree_view_selection_changed`, `app/widgets/gimpcontainertreeview.c`).
 *
 * Direction convention: index 0 of any `children` array is the **top** of the
 * stack, so a higher index is lower in the panel and "sibling below" is the next
 * higher index. See `docs/coordinate-systems.md` and the double `.rev()` in
 * `crates/darkly/src/engine/veils.rs` + `engine/types.rs`.
 */

/**
 * A row's position: its parent and that parent's ordered child list. Modifiers
 * fold in as children of their host, which is what makes the reselection rule
 * uniform across layers, groups and masks: a mask's siblings are the host's
 * other modifiers, and its parent is the host.
 */
interface Slot {
    parent: number | null;
    siblings: number[];
}

export interface LayerTreeIndex {
    /** Every selectable id: nodes at any depth plus their modifiers. */
    ids: Set<number>;
    /**
     * Panel order, top to bottom, each host immediately followed by its
     * modifiers. Descends into collapsed groups: a row the user can't currently
     * see is still a live, selectable node.
     */
    order: number[];
    /** `order` minus everything inside a collapsed group. */
    visibleOrder: number[];
    /** Ids of groups whose children are hidden. */
    collapsed: Set<number>;
    slots: Map<number, Slot>;
}

/**
 * The single walk over a layer tree. Every structural question (liveness,
 * panel order, visibility, parentage) is answered from the one traversal, so
 * callers never hand-roll another.
 */
export function indexLayerTree(tree: any[]): LayerTreeIndex {
    const ids = new Set<number>();
    const order: number[] = [];
    const visibleOrder: number[] = [];
    const collapsed = new Set<number>();
    const slots = new Map<number, Slot>();

    const walk = (nodes: any[], parent: number | null, visible: boolean) => {
        const siblings = nodes.filter((n) => n?.id !== undefined).map((n) => n.id as number);
        for (const n of nodes) {
            if (n?.id === undefined) continue;
            const id: number = n.id;
            ids.add(id);
            order.push(id);
            if (visible) visibleOrder.push(id);
            slots.set(id, { parent, siblings });

            if (Array.isArray(n.modifiers) && n.modifiers.length > 0) {
                const mods = n.modifiers
                    .filter((m: any) => m?.id !== undefined)
                    .map((m: any) => m.id as number);
                for (const m of n.modifiers) {
                    if (m?.id === undefined) continue;
                    ids.add(m.id);
                    order.push(m.id);
                    if (visible) visibleOrder.push(m.id);
                    slots.set(m.id, { parent: id, siblings: mods });
                }
            }

            if (n.type === 'group') {
                if (n.collapsed) collapsed.add(id);
                if (Array.isArray(n.children)) {
                    walk(n.children, id, visible && !n.collapsed);
                }
            }
        }
    };
    walk(Array.isArray(tree) ? tree : [], null, true);

    return { ids, order, visibleOrder, collapsed, slots };
}

/**
 * The row that takes `deadId`'s place: nearest surviving sibling below, else
 * nearest surviving sibling above, else the parent (the enclosing group for a
 * node, the host for a modifier), escalating to the parent's own sibling level
 * when the parent died in the same batch. `null` when nothing qualifies.
 *
 * `prev` describes the tree as it was before the removal; `alive` is the set of
 * ids that remain.
 */
export function nextActiveAfterRemoval(
    prev: LayerTreeIndex,
    alive: Set<number>,
    deadId: number,
): number | null {
    let id = deadId;
    const seen = new Set<number>();
    for (;;) {
        if (seen.has(id)) return null;
        seen.add(id);

        const slot = prev.slots.get(id);
        if (!slot) return null;
        const i = slot.siblings.indexOf(id);
        for (let k = i + 1; k < slot.siblings.length; k++) {
            if (alive.has(slot.siblings[k])) return slot.siblings[k];
        }
        for (let k = i - 1; k >= 0; k--) {
            if (alive.has(slot.siblings[k])) return slot.siblings[k];
        }
        if (slot.parent === null) return null;
        if (alive.has(slot.parent)) return slot.parent;
        id = slot.parent;
    }
}

/**
 * The collapsed groups between `id` and the root, outermost first: the set that
 * must be expanded for `id` to be a row the user can see. Empty when `id` is
 * already visible, absent, or hidden by nothing.
 */
export function collapsedAncestorsOf(index: LayerTreeIndex, id: number): number[] {
    if (!index.ids.has(id)) return [];
    const out: number[] = [];
    let cursor = index.slots.get(id)?.parent ?? null;
    const seen = new Set<number>();
    while (cursor !== null && !seen.has(cursor)) {
        seen.add(cursor);
        if (index.collapsed.has(cursor)) out.push(cursor);
        cursor = index.slots.get(cursor)?.parent ?? null;
    }
    return out.reverse();
}

/**
 * Ids present in `next` but not in `prev`, keeping only the **topmost** of each
 * restored subtree. Undo of a layer removal reattaches the subtree root, but the
 * tree re-serializes every descendant and modifier under it, so the raw
 * difference would select a group *and* everything inside it, a selection the
 * rest of the codebase treats as malformed (batch ops drop any id whose ancestor
 * is also selected). Both reference editors produce single-scope selections
 * here: GIMP a single item, Krita the source layers without their children.
 */
export function appearedRoots(prev: LayerTreeIndex, next: LayerTreeIndex): number[] {
    const fresh = next.order.filter((id) => !prev.ids.has(id));
    if (fresh.length <= 1) return fresh;
    const freshSet = new Set(fresh);
    return fresh.filter((id) => {
        const parent = next.slots.get(id)?.parent ?? null;
        return parent === null || !freshSet.has(parent);
    });
}
