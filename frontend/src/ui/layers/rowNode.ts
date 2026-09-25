/**
 * What the layer panel's row component receives.
 *
 * `LayerInfo` is generated from the Rust `LayerInfo` enum
 * (`engine/protocol_gen.ts`), and the panel draws a row for every variant of it
 * except the viewport divider, which is a separator rather than a node. So the
 * row's prop type is that union minus the divider, derived rather than
 * restated: a new layer kind or a new capability flag reaches the row on the
 * next `DARKLY_REGEN_TS` and needs no edit here.
 *
 * Writing this type out by hand is what the previous hand-written param union
 * did in `ui/filters/filterParams.ts`, and it had already drifted.
 */
import type { LayerInfo, ModifierInfo } from '../../engine/protocol_gen';

/** Any node the panel draws a row for. Excludes the viewport divider. */
export type RowNode = Exclude<LayerInfo, { type: 'divider' }>;

export type { ModifierInfo };

/**
 * The host's mask modifier, or null.
 *
 * The model permits N modifiers; the UI exposes the mask. Both row kinds asked
 * this question and both answered it the same way, so it lives here.
 */
export function maskOf(node: RowNode): ModifierInfo | null {
    return node.modifiers?.find((m) => m.kind === 'mask') ?? null;
}
