/**
 * Global toggle for the "Canvas Size" (resize) modal. The `resizeCanvas`
 * action dispatches into this; the modal reads it.
 */
class ResizeCanvasState {
    open = $state(false);
}

export const resizeCanvas = new ResizeCanvasState();
