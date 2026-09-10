import { actions } from './registry';
import { palettePopup } from '../state/palettePopup.svelte';
import { registerColorsSection } from '../ui/palette_popup/sections/colors';
import { registerBrushesSection } from '../ui/palette_popup/sections/brushes';

/** Register the radial palette popup gesture.
 *
 *  The binding comes from the YAML preset layers: defaults ship
 *  `canvas:rightDrag`, so with a pen the barrel button (which reaches the
 *  browser as the right button) held while the pen touches the canvas
 *  summons the wheel at the contact point; the drag threads it and the lift
 *  commits. The whole lifecycle rides the drag dispatcher, so the overlay
 *  itself is display-only. */
export function registerPalettePopupAction(): void {
    registerColorsSection();
    registerBrushesSection();
    actions.register({
        id: 'palettePopup',
        type: 'hold',
        handler: (ctx) => {
            const e = ctx.event as PointerEvent | undefined;
            if (e) palettePopup.open(e);
        },
        onMove: (_ctx, e) => palettePopup.move(e),
        deactivate: (ctx) => palettePopup.release(ctx.upEvent as PointerEvent | undefined),
    });
}
