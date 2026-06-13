/**
 * Svelte action for backdrop-to-dismiss surfaces — a modal `<dialog>` whose
 * `::backdrop` reports `e.target === dialog`, or an overlay `<div>` that is its
 * own backdrop. The surface closes only when a press *originates* on it.
 *
 * The naive approach — close on `click` whose target is the backdrop — fires on
 * `mouseup`. Selecting text inside a modal and dragging the cursor onto the
 * backdrop before releasing then closes the modal, because the release lands on
 * the backdrop even though the press started on inner content. Gating on
 * `pointerdown` makes the *press* location authoritative, the same reason
 * popups dismiss on pointerdown in `dismiss.ts`. We still require the closing
 * `click` to land on the backdrop too, so a press that began on the backdrop
 * but released over content (the reverse drag) doesn't dismiss either.
 *
 * Usage: `<dialog use:backdropDismiss={onClose}>`.
 */
export function backdropDismiss(node: HTMLElement, onDismiss: () => void) {
    let pressedOnSelf = false;

    function onPointerDown(e: PointerEvent) {
        pressedOnSelf = e.target === node;
    }
    function onClick(e: MouseEvent) {
        if (pressedOnSelf && e.target === node) onDismiss();
        pressedOnSelf = false;
    }

    node.addEventListener('pointerdown', onPointerDown);
    node.addEventListener('click', onClick);

    return {
        update(next: () => void) {
            onDismiss = next;
        },
        destroy() {
            node.removeEventListener('pointerdown', onPointerDown);
            node.removeEventListener('click', onClick);
        },
    };
}
