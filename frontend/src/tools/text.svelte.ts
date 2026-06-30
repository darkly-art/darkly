import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { flushTextContent } from './text_editor';
import type { Tool } from './registry';

/** Session state for the text tool. The tool's only job is to choose what the
 *  properties panel edits; all text controls live in `TextProperties.svelte`.
 *
 *  - The style fields are placement *defaults*: written by panel style edits,
 *    read by the deferred `add_text` for the next new block.
 *  - `focusObject` asks the panel to focus a specific object's editor next
 *    render (set when a text-tool click hits an existing object).
 *  - `placement` is a click on empty canvas that hasn't been committed to a
 *    layer yet — the layer is born on the first typed character, so an
 *    abandoned placement never creates one. */
class TextSession {
    /** Font size in canvas pixels. */
    size = $state(48);
    /** Family name resolved against the engine font collection. */
    fontFamily = $state('Noto Sans');
    /** start | center | end | justified */
    align = $state('start');
    italic = $state(false);
    /** CSS weight, 100–900. */
    weight = $state(400);
    /** Object the panel should focus next render, or null. */
    focusObject = $state<number | null>(null);
    /** Where a new text layer will be born on the first typed character, or
     *  null when there's no pending placement. */
    placement = $state<{ x: number; y: number; anchorLayerId: number | null } | null>(null);
}

export const textSession = new TextSession();

export const textTool: Tool = {
    id: 'text',
    icon: 'fa6-solid:font',
    group: 'paint',
    hotkeyAction: 'textTool',

    onActivate() {
        // Pick up the persisted default size if the user configured one.
        const cfgSize = config.get('tools.textSize');
        if (typeof cfgSize === 'number') textSession.size = cfgSize;
    },

    onDeactivate() {
        // Flush any coalesced keystroke before leaving so the last character
        // isn't dropped, then drop a never-typed placement (nothing to discard
        // — no layer was created).
        flushTextContent();
        textSession.placement = null;
    },

    // The tool never owns the canvas (no overlay) — let global drag chords run.
    claimsPointer() {
        return false;
    },

    async onPointerDown(_ctx, _e, cx, cy) {
        const engine = app.engine;
        const layerId = app.activeLayerId ?? null;

        // A click on an existing text object of the active vector layer focuses
        // its editor in the panel rather than placing a new block.
        if (engine && layerId !== null) {
            const hit = await engine.send<{ object: number }>('hit_test_vector_object', {
                id: layerId,
                x: cx,
                y: cy,
            });
            if (hit && hit.object >= 0) {
                app.selectLayer(layerId);
                textSession.focusObject = hit.object;
                textSession.placement = null;
                return;
            }
        }

        // Miss (or no vector layer) → defer creation. The layer is born on the
        // first keystroke in the panel; until then this is a pending placement.
        textSession.placement = { x: cx, y: cy, anchorLayerId: layerId };
        app.selectLayer(null);
    },

    onPointerMove() {},
    onPointerUp() {},

    dismissOverlay() {
        flushTextContent();
        textSession.placement = null;
    },
};
