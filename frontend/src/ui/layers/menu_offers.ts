/**
 * What a layer row's context menu offers, and under which name.
 *
 * Each offer is a function rather than an inline expression because the menu
 * entry and its click handler both need the answer, and when they each carried
 * their own copy of the rule they drifted (see `flattenOffer` below). One
 * source, no drift.
 */

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
 * The drift this guards against: the entry appeared for a smart object while
 * the handler still early-returned unless there was a mask, so clicking
 * Rasterize did nothing at all — no error, no change.
 *
 * Groups are not covered: they always flatten, under that name, and their row
 * is a different component.
 */
export function flattenOffer(node: { paintable: boolean; hasMask: boolean }): string | null {
    if (!node.paintable) return 'Rasterize';
    return node.hasMask ? 'Flatten' : null;
}

/**
 * Whether the layer row offers to turn the layer into a smart object.
 *
 * The rule itself — owns its pixels, editable, no mask — belongs to the
 * operation, so the engine answers it per row on `LayerInfo`
 * (`can_become_smart_object`, from `engine/smart_object.rs`). This reads that
 * answer and nothing else; restating the rule here would put it in two places
 * that can disagree.
 *
 * A multi-row selection is not offered: the conversion consumes one layer and
 * replaces it in its own slot, and "convert 3 layers" has no single meaning —
 * three separate smart objects and one merged object are both defensible, so
 * neither is assumed.
 */
export function smartObjectOffer(node: { canBecomeSmartObject?: boolean }, isMulti: boolean): boolean {
    return !isMulti && node.canBecomeSmartObject === true;
}
