/**
 * HTTP frame source for the Blender void.
 *
 * A [`FrameSource`](./frameSource.ts) whose frames arrive over a long-lived HTTP
 * response instead of a browser `MediaStream`. The companion Blender add-on
 * (`blender-addon/`) renders the active camera to an offscreen buffer, encodes
 * WebP-with-alpha, and serves frames on `GET <url>` as a single chunked
 * response. Each frame is **length-prefixed** on the wire —
 * `[4-byte big-endian length][WebP bytes]` — so this class reassembles whole
 * frames from the byte stream independent of HTTP chunk boundaries, stashing the
 * latest complete frame as a `Blob`.
 *
 * The shared base owns the per-`tick()` gate and the off-thread
 * `createImageBitmap → uploadVoidExternalImage` upload; `presentFrame()` hands it
 * the latest WebP blob with `{ premultiplyAlpha: 'none' }` so the browser keeps
 * the straight alpha Blender's WebP writer emits (see `docs`/plan "Alpha
 * correctness"). No user gesture or permission is needed for a localhost stream,
 * so unlike `MediaStreamSource` this connects immediately.
 *
 * Sink-side dedup: a static camera produces identical frames, so the add-on only
 * pushes bytes on a real change. `hasNewFrame` flips true when a frame is parsed
 * off the stream and clears after upload, so `tick()` does zero decode + zero GPU
 * work while the scene is still.
 */

import type { Engine } from '../engine/protocol';
import { FrameSource, type CaptureKind, type PresentedFrame } from './frameSource';

export class HttpStreamSource extends FrameSource {
    /** URL of the frame stream. Set by `start` / `setUrl`; a change reconnects. */
    private url = '';
    /** Aborts the in-flight `fetch`. Swapped on reconnect and cleared on stop, so
     *  a superseded read loop can tell it was intentionally torn down. */
    private abort: AbortController | null = null;
    /** Bytes received but not yet forming a complete frame — carried across
     *  `fetch` chunk boundaries. */
    private buffer = new Uint8Array(0);
    /** Latest fully-received WebP frame, ready to decode. */
    private latestFrame: Blob | null = null;
    /** True once a new frame has been parsed and not yet uploaded (dedup gate). */
    private hasNewFrame = false;

    constructor(
        layerId: number,
        engine: Engine,
        onEnded: ((layerId: number) => void) | null = null,
    ) {
        super(layerId, engine, 'stream', onEnded);
    }

    /** Connect to `url` and begin reading frames. Idempotent against `stopped`;
     *  re-issuing (via `setUrl`) supersedes any in-flight connection. */
    async start(url: string): Promise<void> {
        if (this.stopped) return;
        this.url = url;
        await this.connect();
    }

    /** Reconnect when the `url` param changes. Called by the layer-tree
     *  reconciler, mirroring how `freeze` / `frame_divisor` are pushed. */
    setUrl(url: string): void {
        if (this.stopped || url === this.url) return;
        void this.start(url);
    }

    /** Open the `fetch` and, once connected, spawn the background read pump.
     *  Resolves as soon as the response headers arrive (or the connection
     *  fails) — it does NOT block for the lifetime of the stream, so callers can
     *  `await` a connect without hanging until the feed ends. */
    private async connect(): Promise<void> {
        // Supersede any in-flight connection; its pump sees `abort !== this`
        // (or an aborted signal) and bows out without firing `onEnded`.
        this.abort?.abort();
        const controller = new AbortController();
        this.abort = controller;
        this.buffer = new Uint8Array(0);
        this.error = null;
        this.ended = false;
        const url = this.url;
        let reader: ReadableStreamDefaultReader<Uint8Array>;
        try {
            const resp = await fetch(url, { signal: controller.signal });
            if (!resp.ok || !resp.body) {
                throw new Error(`HTTP ${resp.status}`);
            }
            reader = resp.body.getReader();
        } catch (err) {
            // Intentional teardown / reconnect — not a real disconnect.
            if (controller.signal.aborted || this.abort !== controller) return;
            this.handleDisconnect(err);
            return;
        }
        // Pump in the background; `connect` (and thus `start`) resolves now.
        void this.pump(reader, controller);
    }

    /** Drain the response body into the frame parser until the stream ends,
     *  errors, or is superseded/stopped. Runs detached from `connect`. */
    private async pump(
        reader: ReadableStreamDefaultReader<Uint8Array>,
        controller: AbortController,
    ): Promise<void> {
        try {
            for (;;) {
                const { done, value } = await reader.read();
                if (done) break;
                // A newer connect() (or stop) took over while we awaited — drop
                // this loop without touching shared state or notifying.
                if (this.stopped || this.abort !== controller) return;
                if (value && value.length) this.ingest(value);
            }
            // Server closed the response cleanly (add-on stopped / client dropped).
            if (this.abort === controller) this.handleDisconnect(null);
        } catch (err) {
            if (controller.signal.aborted || this.abort !== controller) return;
            this.handleDisconnect(err);
        }
    }

    /** Append a received chunk and extract every complete length-prefixed frame,
     *  keeping the trailing partial bytes for the next chunk. */
    private ingest(chunk: Uint8Array): void {
        const merged = new Uint8Array(this.buffer.length + chunk.length);
        merged.set(this.buffer, 0);
        merged.set(chunk, this.buffer.length);
        this.buffer = merged;

        for (;;) {
            if (this.buffer.length < 4) break;
            const len = new DataView(
                this.buffer.buffer,
                this.buffer.byteOffset,
                4,
            ).getUint32(0, false); // big-endian
            if (this.buffer.length < 4 + len) break;
            // `.slice` copies, so the Blob owns its bytes and the DataView above
            // always sees byteOffset 0 on the next iteration.
            const frameBytes = this.buffer.slice(4, 4 + len);
            this.latestFrame = new Blob([frameBytes], { type: 'image/webp' });
            this.hasNewFrame = true;
            this.buffer = this.buffer.slice(4 + len);
        }
    }

    /** Note the disconnect, surface an error, and notify the app so it prunes the
     *  source and re-shows "Connect" — reusing the same machinery MediaStream
     *  voids use on external stop. Idempotent. */
    private handleDisconnect(err: unknown): void {
        if (this.stopped || this.ended) return;
        this.ended = true;
        this.error = describeStreamError(err, this.url);
        this.onEnded?.(this.layerId);
    }

    /** The latest complete WebP frame, decoded with straight alpha preserved. */
    protected presentFrame(): PresentedFrame | null {
        if (!this.latestFrame) return null;
        return { source: this.latestFrame, options: { premultiplyAlpha: 'none' } };
    }

    /** Only decode when a genuinely new frame has arrived — a static scene sends
     *  no bytes, so this stays false and `tick()` does nothing. */
    protected hasFrameReady(): boolean {
        return this.hasNewFrame && this.latestFrame !== null;
    }

    /** Clear the dedup flag so the next tick waits for a fresh frame. */
    protected afterUpload(): void {
        this.hasNewFrame = false;
    }

    /** Abort the stream, drop the buffered frame, and mark this source
     *  permanently dead. Safe to call multiple times. */
    stop(): void {
        this.stopped = true;
        this.abort?.abort();
        this.abort = null;
        this.buffer = new Uint8Array(0);
        this.latestFrame = null;
        this.hasNewFrame = false;
        this.decoding = false;
    }
}

/** Human-readable message for a stream disconnect / connection failure, shown in
 *  the VoidProperties notice. A clean server close (`err == null`) reads as a
 *  plain disconnect; a `fetch` rejection maps its cause. Exported for testing. */
export function describeStreamError(err: unknown, url: string): string {
    if (err == null) {
        return `Blender stream at ${url} disconnected.`;
    }
    const message = (err as Error)?.message ?? String(err);
    // A localhost `fetch` that can't reach the server rejects with a TypeError
    // ("Failed to fetch") — the add-on almost certainly isn't running.
    if (err instanceof TypeError) {
        return `Could not connect to the Blender stream at ${url}. Is the add-on running?`;
    }
    return `Blender stream error: ${message}`;
}

// Re-export for symmetry with `mediaStreamSource`'s `CaptureKind` re-export.
export type { CaptureKind };
