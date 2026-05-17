/**
 * `use:bindingSite={{ name, ctx, mouse? }}` — declares a DOM element as a
 * binding site for both keyboard scope and mouse-click dispatch.
 *
 * What it does:
 *
 *   1. **Keyboard scope.** Sets `tabindex="-1"` so the element can hold focus
 *      programmatically (not via Tab — see `suppressButtonKeyboardFocus.ts`).
 *      Listens for `mousedown` in the bubble phase and calls `.focus()`,
 *      since the capture-phase button-focus suppression would otherwise
 *      block click-to-focus on inner buttons.
 *      The hotkey dispatcher (`hotkeys.svelte.ts`) walks the focus chain
 *      and resolves `<site>:<chord>` bindings against this element.
 *
 *   2. **Mouse-click chord dispatch.** Listens for `click` in capture phase
 *      and routes modifier+click chords to `dispatchClick(name, ...)`. If
 *      a chord matches, propagation is stopped so the element's own
 *      `onclick` (the no-chord fallback) doesn't also fire.
 *
 *   3. **Drag chord dispatch (opt-out).** Same pattern via `pointerdown`,
 *      routing to `dispatchDrag(name, ...)`. Disabled when `mouse: false`,
 *      which is required for sites whose containing element runs its own
 *      pointer pipeline (canvas: navigation → tool-claim → chord → tool
 *      default) where ordering matters.
 */

import { dispatchClick, dispatchDrag } from './triggers';
import { registerSite, unregisterSite, type BindingSiteEntry } from './active_site';

export interface BindingSiteParams {
    name: string;
    ctx?: (e?: Event) => Record<string, unknown>;
    /** Defaults to true. Set false to opt out of click/drag chord dispatch
     *  on this site (used by the canvas, which orders its own pointer
     *  pipeline). Keyboard scope still works. */
    mouse?: boolean;
}

export function bindingSite(node: HTMLElement, params: BindingSiteParams) {
    let current = params;

    const entry: BindingSiteEntry = {
        get name() { return current.name; },
        ctx: (e?: Event) => current.ctx?.(e) ?? {},
    } as BindingSiteEntry;

    node.setAttribute('tabindex', '-1');
    registerSite(node, entry);

    const onMouseDown = (e: MouseEvent) => {
        // The capture-phase preventDefault in suppressButtonKeyboardFocus
        // would otherwise block click-to-focus on inner buttons; restore
        // focus to the site root explicitly here (bubble phase, after the
        // suppression has run).
        //
        // stopPropagation: when binding sites are nested (e.g. a `layerEye`
        // button inside a `layerPanel` panel), the innermost site should
        // own focus — otherwise the outer site's mousedown would re-focus
        // its own root on bubble. The outer site still appears in the
        // activeSiteChain via DOM ancestry, so its hotkeys still fire.
        node.focus();
        e.stopPropagation();
    };

    const mouseEnabled = () => current.mouse !== false;

    const onClickCapture = (e: MouseEvent) => {
        if (!mouseEnabled()) return;
        if (dispatchClick(current.name, e, entry.ctx(e))) {
            e.stopImmediatePropagation();
            e.preventDefault();
        }
    };

    const onPointerDownCapture = (e: PointerEvent) => {
        if (!mouseEnabled()) return;
        if (dispatchDrag(current.name, e, entry.ctx(e))) {
            e.stopImmediatePropagation();
            e.preventDefault();
        }
    };

    node.addEventListener('mousedown', onMouseDown);
    node.addEventListener('click', onClickCapture, true);
    node.addEventListener('pointerdown', onPointerDownCapture, true);

    return {
        update(next: BindingSiteParams) {
            current = next;
        },
        destroy() {
            node.removeEventListener('mousedown', onMouseDown);
            node.removeEventListener('click', onClickCapture, true);
            node.removeEventListener('pointerdown', onPointerDownCapture, true);
            unregisterSite(node);
        },
    };
}
