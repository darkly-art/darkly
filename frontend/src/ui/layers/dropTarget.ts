/**
 * Where a layer-panel drag is asking to drop, as pure arithmetic over the panel's
 * row list.
 *
 * The gesture belongs to the **gap** between two consecutive visible rows, not to
 * a row. Gap `g` sits below `rows[g - 1]` and above `rows[g]`; gap `0` is above
 * the first row and gap `rows.length` is below the last. A row's drag bands map
 * onto the gaps around it (see `bandToGap`), so every drop in the panel — row
 * edges, the viewport divider, the empty area below the list — resolves through
 * one function instead of each site inventing its own.
 *
 * Nothing here touches the DOM or the engine. That is deliberate and load-bearing:
 * Vitest runs in the node environment with no DOM, so logic welded to `DragEvent`
 * cannot be tested at all.
 */

import type { DropRow } from '../../state/layerTree';
import type { MoveTarget } from '../../engine/protocol_gen';

/** Left padding of a depth-0 row, in px. Mirrors the panel's row styles. */
export const ROW_BASE_PAD = 8;
/** Additional left padding per depth level, in px. */
export const ROW_INDENT = 16;

export interface DropResolution {
    /** The tree depth the drop lands at. Drives the indicator's indent. */
    depth: number;
    target: MoveTarget;
}

/**
 * A drag band within a row: which affordance the row draws, which gap the
 * pointer addresses, and the depth that band names.
 */
export interface Band {
    band: 'above' | 'below' | 'into';
    gap: number;
    pin: 'min' | 'max';
}

/**
 * The insertion depths that are structurally legal in gap `gap`.
 *
 * The shallow end is fixed by the row *below* the gap: dropping above `rows[gap]`
 * cannot land deeper-out than that row's own level without reordering it too. The
 * deep end is fixed by the row *above*: a drop can land as a sibling of it, or —
 * when it is a group — one level further in, as its first child.
 *
 * Off either end of the list the missing neighbour imposes no constraint, so the
 * range opens to the root.
 */
export function gapDepthRange(rows: DropRow[], gap: number): { min: number; max: number } {
    const prev = gap > 0 ? rows[gap - 1] : undefined;
    const next = gap < rows.length ? rows[gap] : undefined;
    const min = next ? next.depth : 0;
    const max = prev ? prev.depth + (prev.isGroup ? 1 : 0) : 0;
    // A collapsed or empty group can leave `max` below `min`; the row below the
    // gap always wins, because it is the one whose parentage the drop must not
    // silently change.
    return { min, max: Math.max(min, max) };
}

/**
 * Which gap a pointer in row `rowIndex` is addressing, and at what depth, from
 * how far down the row it sits.
 *
 * The upper band addresses the gap above the row and pins to the shallow end of
 * it; the lower band addresses the gap below and pins deep. Those two pins are
 * what make a drop stay in the parent it was gestured at, so an edge drop reads
 * as "next to this row" rather than as a reparent the pointer never asked for.
 *
 * A group has a third band: 25%–75% means "drop inside me", the only way to
 * reach a *collapsed* group's interior, since an expanded one is reachable
 * through the gap below its header. It resolves to that same gap, pinned deep.
 */
export function bandToGap(rowIndex: number, isGroup: boolean, yRatio: number): Band {
    if (isGroup) {
        if (yRatio < 0.25) return { band: 'above', gap: rowIndex, pin: 'min' };
        if (yRatio > 0.75) return { band: 'below', gap: rowIndex + 1, pin: 'max' };
        return { band: 'into', gap: rowIndex + 1, pin: 'max' };
    }
    return yRatio < 0.5
        ? { band: 'above', gap: rowIndex, pin: 'min' }
        : { band: 'below', gap: rowIndex + 1, pin: 'max' };
}

/**
 * The drop the pointer is asking for, or `null` when there is nothing to drop
 * against.
 *
 * `xOffset` is the pointer's distance from the panel's left edge; it selects a
 * depth within the gap's legal range. `pin` overrides that reading for the
 * gestures that name a depth outright rather than gesturing at one — a group's
 * `into` band pins deep, the empty area below the list pins shallow.
 */
export function resolveGapDrop(
    rows: DropRow[],
    gap: number,
    xOffset: number,
    pin?: 'min' | 'max',
): DropResolution | null {
    if (rows.length === 0) return null;
    const clamped = Math.max(0, Math.min(gap, rows.length));
    const { min, max } = gapDepthRange(rows, clamped);

    let depth: number;
    if (pin === 'min') depth = min;
    else if (pin === 'max') depth = max;
    else {
        const asked = Math.round((xOffset - ROW_BASE_PAD) / ROW_INDENT);
        depth = Math.max(min, Math.min(max, asked));
    }

    const target = targetForDepth(rows, clamped, depth);
    return target ? { depth, target } : null;
}

/**
 * Turn a (gap, depth) pair into the move the engine understands.
 *
 * Panel order is top-first while `MoveTarget` speaks in stack terms, where
 * `Before(x)` means "below x in the panel". So landing in the gap *above* a row
 * is `After` that row, and landing below one is `Before` it.
 */
function targetForDepth(rows: DropRow[], gap: number, depth: number): MoveTarget | null {
    const prev = gap > 0 ? rows[gap - 1] : undefined;
    const next = gap < rows.length ? rows[gap] : undefined;

    if (prev) {
        // One level deeper than the row above, which is only offered for a group:
        // the drop is asking to become its first child.
        if (prev.isGroup && depth === prev.depth + 1) {
            return { target_type: 'into_top', target_id: prev.id };
        }
        if (depth === prev.depth) {
            return { target_type: 'before', target_id: prev.id };
        }
        if (depth < prev.depth) {
            // Escaping outward: land below the nearest enclosing row at the
            // requested depth, which is the ancestor the gesture pointed at.
            for (let i = gap - 1; i >= 0; i--) {
                if (rows[i].depth === depth) {
                    return { target_type: 'before', target_id: rows[i].id };
                }
            }
        }
    }

    // No row above the gap, or no ancestor matched: fall back to the row below,
    // which the depth range guarantees exists whenever `prev` does not.
    return next ? { target_type: 'after', target_id: next.id } : null;
}
