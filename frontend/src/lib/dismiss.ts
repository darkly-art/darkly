/**
 * Dismiss an open popup on any pointerdown that doesn't land on one of its
 * keep-open controls. A popup tags its trigger and its panel (wherever they
 * live, even in different components) with `data-keep-open="<scope>"`; a
 * pointerdown whose target isn't inside an element of that scope closes the
 * popup. One rule covers everything non-interactive uniformly — surrounding
 * chrome, the panel's own padding/separators, and the canvas.
 *
 * The `scope` keeps independent popups from holding each other open: each only
 * treats *its own* scope's controls as keep-open, so clicking into one popup
 * dismisses the others. The attribute is matched with `~=` (space-separated
 * tokens), so an element can belong to multiple scopes if ever needed.
 *
 * Two reasons it's `pointerdown`, not `click`:
 *  - The canvas calls `preventDefault()` in its pointerdown handler
 *    (`CanvasView.svelte`, to suppress pen fling-scroll) and takes pointer
 *    capture, which suppresses the synthetic `click`. A click-based dismiss
 *    never fires for clicks on the canvas.
 *  - Marked controls are skipped, so pressing an action row doesn't unmount
 *    the popup before that row's own `click` runs.
 *
 * Returns a teardown function; call it from a Svelte `$effect`.
 */
export function watchDismiss(scope: string, onDismiss: () => void): () => void {
    const selector = `[data-keep-open~="${scope}"]`;
    function handle(e: PointerEvent) {
        if (!(e.target as HTMLElement).closest(selector)) onDismiss();
    }
    window.addEventListener('pointerdown', handle);
    return () => window.removeEventListener('pointerdown', handle);
}
