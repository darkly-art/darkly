/**
 * Whether the layer row offers to bake a node into plain pixels, and under
 * which name.
 *
 * One action (`flatten`) with two names, because it answers two questions: a
 * layer that owns its pixels and carries a mask gets **Flatten** (bake the mask
 * in), and a layer whose pixels are generated — a smart object, a camera void,
 * a text layer — gets **Rasterize**, the verb every editor uses for it and the
 * answer to "why can't I paint on this?".
 *
 * A function rather than an inline expression because the menu entry and its
 * click handler both need the answer, and when they each carried their own copy
 * of the rule they drifted: the entry appeared for a smart object while the
 * handler still early-returned unless there was a mask, so clicking Rasterize
 * did nothing at all — no error, no change. One source, no drift.
 *
 * Groups are not covered: they always flatten, under that name, and their row
 * is a different component.
 */
export function flattenOffer(node: { paintable: boolean; hasMask: boolean }): string | null {
    if (!node.paintable) return 'Rasterize';
    return node.hasMask ? 'Flatten' : null;
}
