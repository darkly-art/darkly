/**
 * A baked-PNG poller: fetch bytes from the engine, poll on rAF while a
 * cold-cache GPU bake completes (the engine returns empty bytes until the
 * bake lands on a render frame), and expose the result as a reactive object
 * URL.
 *
 * Object URLs are cheaper than data URLs across remounts (the browser gets
 * the bytes once, not per render) and are revoked when bytes change or the
 * owner destroys the poller. Byte length keys the churn guard: identical
 * payload length is treated as a cache hit.
 *
 * Extracted from `BrushPreviewStrip.svelte`; the palette popup's brush
 * leaves are the second consumer.
 */
import { app } from '../state/app.svelte';
import { SignalCompressor } from './signal_compressor';

/** Same throttle cadence as the dab and editor previews. */
const REFRESH_MS = 100;

/** rAF poll budget per request: a bake fits comfortably inside 30 frames. */
const POLL_FRAMES_PER_REQUEST = 30;

export class BakedThumbnail {
    /** Object URL of the latest non-empty payload, '' until one arrives. */
    url = $state('');
    #len = 0;
    #frames = 0;
    #raf = 0;
    readonly #fetch: () => Promise<Uint8Array | undefined>;
    readonly #compressor: SignalCompressor;

    constructor(fetch: () => Promise<Uint8Array | undefined>) {
        this.#fetch = fetch;
        this.#compressor = new SignalCompressor(REFRESH_MS, () => {
            void this.#refresh();
            this.#frames = POLL_FRAMES_PER_REQUEST;
            this.#schedule();
        });
    }

    /** Request a (re)fetch. Coalesced; polls until bytes arrive or the
     *  frame budget runs out. */
    request(): void {
        this.#compressor.request();
    }

    /** Stop polling and release the URL. Call on unmount. */
    destroy(): void {
        this.#compressor.cancel();
        if (this.#raf) cancelAnimationFrame(this.#raf);
        this.#raf = 0;
        if (this.url) URL.revokeObjectURL(this.url);
        this.url = '';
        this.#len = 0;
    }

    async #refresh(): Promise<void> {
        const bytes = await this.#fetch();
        if (!bytes || bytes.length === 0) return;
        if (bytes.length === this.#len && this.url) return;
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (this.url) URL.revokeObjectURL(this.url);
        this.url = next;
        this.#len = bytes.length;
    }

    #schedule(): void {
        if (this.#raf) return;
        this.#raf = requestAnimationFrame(() => this.#onFrame());
    }

    #onFrame(): void {
        this.#raf = 0;
        if (this.#frames <= 0) return;
        this.#frames--;
        // The engine completes bakes on render frames, so polling must keep
        // the render loop ticking.
        app.requestFrame();
        void this.#refresh();
        this.#schedule();
    }
}
