/**
 * The anchor a drag-reorder sends: the key of the item that sits directly
 * after `key` in the final `order`, or `null` when `key` ends the list.
 *
 * A reorder names its destination by neighbour rather than by index because
 * the list a user sees may be a filtered view of the list being reordered
 * (the brush bar hides wire-driven entries). A neighbour's key means the same
 * thing in both; a position does not.
 */
export function reorderAnchor(order: readonly string[], key: string): string | null {
    const idx = order.indexOf(key);
    if (idx === -1) throw new Error(`reorderAnchor: '${key}' is not in the order`);
    return order[idx + 1] ?? null;
}
