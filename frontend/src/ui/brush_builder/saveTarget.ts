/**
 * What "save" means for the brush the builder currently has open.
 *
 * Pure, so it is testable without a DOM: the same reason `grouping.ts` and
 * `placement.ts` sit beside their components rather than inside them.
 */
import type { BrushInfo } from '../../engine/protocol_gen';

/**
 * The brush the active graph may be saved back over, or `null` when the only
 * option is saving as new.
 *
 * Two cases yield `null`, and they are different in the UI only in that both
 * offer one button rather than two:
 *
 * - Nothing recognizable is loaded (a scratch graph, or a name that no longer
 *   resolves).
 * - The loaded brush ships with the app. Saving over one would shadow it until
 *   the next boot rebuilt it from YAML, so a modified builtin is saved as new;
 *   `can_edit` is the engine's own answer to who owns it.
 *
 * Looked up by name because `activeBrush` is a name: `brush_load` is the one
 * name-keyed call in the library API. Display names are not guaranteed unique,
 * so a collision resolves to whichever the engine listed first; that ambiguity
 * is the API asymmetry, not something this function can fix.
 */
export function updateTarget(
    activeBrush: string | null,
    brushes: BrushInfo[],
): BrushInfo | null {
    if (!activeBrush) return null;
    const found = brushes.find(b => b.name === activeBrush);
    return found?.can_edit ? found : null;
}
