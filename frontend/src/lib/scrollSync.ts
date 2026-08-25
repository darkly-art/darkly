/**
 * Ownership arbitration for two scrollports that drive each other.
 *
 * Two-way scroll sync oscillates the obvious way: A's `scroll` handler writes
 * `B.scrollTop`, which fires B's handler, which writes back to A, and sub-pixel
 * rounding keeps them ping-ponging. The fix is an ownership token, not an
 * epsilon comparison: whichever side the painter is actually driving owns the
 * pair, and a `scroll` event from the other side is discarded as an echo.
 *
 * Generic by name and by shape — nothing here knows about brushes or wheels.
 */

/**
 * Which of two coupled scrollports is currently driving.
 *
 * Ownership is claimed on the first `scroll` event from a side and released
 * `holdMs` after that side's last one, so a fling's whole momentum tail stays
 * owned. A trailing timer rather than one animation frame because a
 * programmatic `scrollTop` write fires its `scroll` event before the next
 * frame — a one-frame token suppresses the immediate echo, but native momentum
 * runs for hundreds of milliseconds, and writing back mid-momentum fights the
 * browser's own scrolling. `scrollend` would be tidier but Safari lacks it, and
 * pen tablets are the point.
 *
 * The clock is injected, so this is testable in the node environment.
 */
export class ScrollSyncToken<Side extends string> {
    #owner: Side | null = null;
    #lastActivity = 0;
    readonly #holdMs: number;

    constructor(holdMs = 120) {
        this.#holdMs = holdMs;
    }

    /**
     * Whether `side` may write the other pane right now.
     *
     * An unowned pair is claimed by whoever asks first. The owner keeps it, and
     * each call refreshes the hold, so a continuous stream of scroll events
     * never lets go mid-gesture.
     */
    claim(side: Side, now: number): boolean {
        if (this.#owner !== null && this.#owner !== side && now - this.#lastActivity < this.#holdMs) {
            return false;
        }
        this.#owner = side;
        this.#lastActivity = now;
        return true;
    }

    /**
     * Take ownership for `side` regardless of who holds it.
     *
     * For a `pointerdown`: a deliberate touch is never an echo, where a
     * `scroll` event is ambiguous. Without this, a pen landing on one pane
     * during the other's momentum tail is refused for the whole hold window and
     * the pane snaps back under the painter's finger — the token would convert
     * oscillation into eaten input.
     */
    preempt(side: Side, now: number): void {
        this.#owner = side;
        this.#lastActivity = now;
    }

    /** Who holds the pair, if anyone. */
    get owner(): Side | null {
        return this.#owner;
    }

    /** Drop ownership immediately. For teardown, and for tests. */
    release(): void {
        this.#owner = null;
        this.#lastActivity = 0;
    }
}
