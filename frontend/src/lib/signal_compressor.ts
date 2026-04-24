/**
 * Leading-edge debouncer — fire immediately, then suppress further fires
 * for `delayMs`. If any requests arrive during the suppression window, one
 * final fire is scheduled at the end of it so the caller never misses the
 * latest state.
 *
 * Mirrors Krita's `KisSignalCompressor` in FIRST_ACTIVE mode
 * (`krita/libs/global/kis_signal_compressor.h`) — the same pattern that
 * drives its live brush preview at a ~100ms cadence.
 *
 * Typical use: slider drags fire dozens of times per second; the WASM
 * preview render takes ~10-30ms each. Without throttling, updates pile up
 * and fall behind the user's input. With leading-edge throttling, the user
 * sees an immediate response and the final value always wins.
 */
export class SignalCompressor {
    private readonly delayMs: number;
    private readonly callback: () => void;
    private timer: ReturnType<typeof setTimeout> | null = null;
    private trailingPending = false;

    constructor(delayMs: number, callback: () => void) {
        this.delayMs = delayMs;
        this.callback = callback;
    }

    /** Request a fire. Fires immediately if not in the suppression window. */
    request() {
        if (this.timer !== null) {
            // Already firing / within the suppression window — mark a trailing fire.
            this.trailingPending = true;
            return;
        }
        // Fire now, then start the suppression window.
        this.callback();
        this.trailingPending = false;
        this.timer = setTimeout(() => this.onTimeout(), this.delayMs);
    }

    /** Cancel any pending trailing fire. Useful on unmount. */
    cancel() {
        if (this.timer !== null) {
            clearTimeout(this.timer);
            this.timer = null;
        }
        this.trailingPending = false;
    }

    private onTimeout() {
        this.timer = null;
        if (this.trailingPending) {
            this.trailingPending = false;
            // Trailing fire, then re-enter the suppression window so a
            // burst of requests during one window still coalesces into
            // one more fire.
            this.callback();
            this.timer = setTimeout(() => this.onTimeout(), this.delayMs);
        }
    }
}
