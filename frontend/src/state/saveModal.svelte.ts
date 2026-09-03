/**
 * Fallback Save dialog state, used only in browsers without the File System
 * Access API (Firefox, Safari) where there is no native save picker.
 *
 * The unified save flow (`saveDocument`) awaits `request()`, which resolves
 * once the artist confirms or cancels, so `closeGuard.save()` can await the
 * whole save before deciding whether to close the tab. The modal itself drives
 * produce + download via the shared `saveViaDownload`.
 */
class SaveModalState {
    open = $state(false);
    /** Filename (no extension) to prefill, seeded from the document name. */
    suggestedName = $state('');
    private resolver: (() => void) | null = null;

    /** Open the modal for `suggestedName` and resolve when the artist confirms
     *  or cancels (or dismisses via Escape / backdrop). */
    request(suggestedName: string): Promise<void> {
        this.suggestedName = suggestedName;
        this.open = true;
        return new Promise((resolve) => {
            this.resolver = resolve;
        });
    }

    /** Close the modal and resolve the pending `request()`. Idempotent, so it's
     *  safe to call from both the buttons and the dismiss-on-close guard. */
    finish(): void {
        this.open = false;
        const resolve = this.resolver;
        this.resolver = null;
        resolve?.();
    }
}

export const saveModal = new SaveModalState();
