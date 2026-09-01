/**
 * Shared base for a void's per-frame "external image → GPU texture" lifecycle.
 *
 * A void whose frames come from outside the engine, such as a webcam /
 * screenshare (`MediaStreamSource`) or a Blender HTTP feed (`HttpStreamSource`),
 * needs the same per-`tick()` machinery: honor the visible / frozen / frame-divisor /
 * has-new-frame gate, ask for the current decodable frame, decode it off-thread
 * into a finalized `ImageBitmap` (downscaling to the display cap), and hand it to
 * `copy_external_image_to_texture` via the WASM bridge. Only *where the frame
 * comes from* differs, so that's all a subclass supplies.
 *
 * Why an `ImageBitmap` and not a 2D canvas? `copyExternalImageToTexture` from a
 * 2D `OffscreenCanvas` forces a cross-context GPU fence (the canvas lives in the
 * browser's compositor/GL context, the texture in the WebGPU device), which
 * stalls the render loop ~one frame per upload, independent of resolution. A
 * finalized `ImageBitmap` copies without that fence, `createImageBitmap` decodes
 * off the main thread (moving that work off the critical path), and downscales to
 * the display cap in the same pass. It's also the most cross-browser-robust
 * source accepted by every WebGPU implementation.
 *
 * Subclasses implement:
 *  - `presentFrame()` - the current decodable frame (+ its source dims, if known,
 *    for the resolution cap, + `createImageBitmap` options), or `null` if none.
 *  - `hasFrameReady()` - the sink-side gate: is there a frame worth decoding this
 *    tick? A live camera is always "ready"; a static HTTP stream only when a new
 *    frame has actually arrived (so a still scene drives zero decode + GPU work).
 *  - `afterUpload()` - post-upload hook (e.g. clear the "new frame" flag).
 *  - `stop()` - tear down and mark permanently dead.
 */

import type { Engine } from '../engine/protocol';

/** How the frontend acquires a void's frames. Mirrors the Rust `CaptureKind`
 *  serialization (`engine/void.rs`): `getUserMedia`, `getDisplayMedia`, or an
 *  HTTP frame stream (the Blender void). */
export type CaptureKind = 'camera' | 'display' | 'stream';

/** Connection state of a frame source, for positive UI feedback (a
 *  change-driven stream sends no bytes while idle, so "no frames" alone can't
 *  distinguish connected from dead). `connecting` covers both the initial
 *  handshake and a pending permission prompt; `disconnected` is terminal for
 *  the source instance (reconnecting builds a new one). */
export type StreamStatus = 'connecting' | 'connected' | 'disconnected';

/** A frame a subclass offers up for decoding this tick. `source` is anything
 *  `createImageBitmap` accepts (a `<video>` element, a WebP `Blob`, …). When
 *  `sourceWidth`/`sourceHeight` are known the base applies the resolution cap;
 *  when they're 0/undefined (e.g. an undecoded blob) it decodes at native size.
 *  `options` are merged into the `createImageBitmap` call (e.g.
 *  `{ premultiplyAlpha: 'premultiply' }` to match the premultiplied convention
 *  of the void's frame texture). */
export interface PresentedFrame {
    source: CanvasImageSource | Blob;
    sourceWidth?: number;
    sourceHeight?: number;
    options?: ImageBitmapOptions;
}

export abstract class FrameSource {
    readonly layerId: number;
    readonly captureKind: CaptureKind;
    protected readonly engine: Engine;
    /** Invoked when the feed ends *externally* (the user clicks the browser's
     *  "Stop sharing" bar, unplugs the webcam, or the HTTP stream closes). The
     *  app uses it to stop the source (kept in the map so its error/status
     *  stay visible) and re-show the "Connect"/"Resume" affordance. */
    protected readonly onEnded: ((layerId: number) => void) | null;

    /** Invoked on every `status` transition. Class-instance field mutation is
     *  invisible to Svelte's `$state` Map, so the app uses this to reassign
     *  the map and trigger a re-render, mirroring `onEnded`. */
    protected readonly onStatusChange: ((layerId: number) => void) | null;

    /** True once the feed has ended externally. Observable so the properties
     *  panel can distinguish "never started" from "stopped/disconnected" and
     *  offer Connect/Resume in both cases. */
    ended = false;

    /** Human-readable error if the feed failed (permission denied, connection
     *  refused, …). Reactive Svelte readers in VoidProperties pull this. */
    error: string | null = null;

    /** Connection state, driven by subclasses via `setStatus`. Starts
     *  `connecting`: a source is only constructed to be started immediately,
     *  and for camera/screenshare the permission prompt is part of
     *  connecting. */
    status: StreamStatus = 'connecting';

    /** Transition `status` and notify (deduped: repeat sets are silent). */
    protected setStatus(status: StreamStatus): void {
        if (status === this.status) return;
        this.status = status;
        // Each fresh connection opens a new frame-delivery epoch, so the next
        // upload is again a "first frame" worth a timelapse capture. Covers
        // both a brand-new source and an in-place reconnect (`setUrl`).
        if (status === 'connected') this.firstFrameUploaded = false;
        this.onStatusChange?.(this.layerId);
    }

    /** Record a failure that happened outside the source's own machinery
     *  (media acquisition is app-owned: the permission prompt runs before
     *  `start` is ever called). Sets the user-facing error and lands the
     *  status, without flipping `ended`; the feed never began. */
    markFailed(message: string): void {
        this.error = message;
        this.setStatus('disconnected');
    }

    /** Marked permanently dead by `stop()`. Gates `tick()` and the decode
     *  callback so a late-resolving bitmap from a torn-down source is dropped. */
    protected stopped = false;

    /** How many `tick()` calls to skip between uploads. Mirrors the void's
     *  `frame_divisor` param; pushed in by the layer-tree reconciler. The gate is
     *  `frameCount % frameDivisor === 0` against the compositor's master counter,
     *  so a `divisor=N` source fires on the exact rAF a veil `divisor=N` does. */
    protected frameDivisor = 4;

    /** Effective visibility (self + every ancestor). When false, `tick()`
     *  short-circuits: no decode, no bridge call, no GPU work. Rust independently
     *  gates `wants_external_input`; this is the JS-local skip. */
    protected visible = true;

    /** Whether the void's `freeze` param is on. When frozen, uploads stop so the
     *  GPU holds the last frame, but the underlying feed stays open so unfreezing
     *  resumes instantly. Rust independently drops frames while frozen. */
    protected frozen = false;

    /** True while a `createImageBitmap` decode is in flight, so a slow decode
     *  can't pile up a backlog faster than the GPU drains it; the loop
     *  self-throttles to the decode rate. */
    protected decoding = false;

    /** Cleared on every `connected` transition, set after the first upload of
     *  that connection. Gates the timelapse "start" milestone: a live void's
     *  streamed frames never bump the document revision, so its first real
     *  frame would otherwise be absent from the recording. */
    private firstFrameUploaded = false;

    /** Longest edge (px) the decoded bitmap (and the GPU upload) may reach; the
     *  source is downscaled to fit, preserving aspect. Pushed in as the
     *  document-canvas long edge: the compositor renders the void cover-fit into a
     *  canvas-resolution target, so finer source detail is wasted bandwidth. `0`
     *  means "no cap". Only applied when `presentFrame` reports source dims. */
    protected maxSourceDimension = 0;

    constructor(
        layerId: number,
        engine: Engine,
        captureKind: CaptureKind,
        onEnded: ((layerId: number) => void) | null = null,
        onStatusChange: ((layerId: number) => void) | null = null,
    ) {
        this.layerId = layerId;
        this.engine = engine;
        this.captureKind = captureKind;
        this.onEnded = onEnded;
        this.onStatusChange = onStatusChange;
    }

    /** Push the current frame into the void's input texture. Cheap when nothing
     *  is ready (no-op), safe to call every animation frame.
     *
     *  `frameCount` is the compositor's canonical master tick (see
     *  `DarklyHandle.frame_count`). Using it directly keeps the divisor gate
     *  phase-locked with every other divisor-throttled system in the engine. */
    tick(frameCount: number): void {
        if (!this.visible || this.frozen) return;
        if (frameCount % this.frameDivisor !== 0) return;
        if (this.stopped || this.decoding) return;
        if (!this.hasFrameReady()) return;
        const frame = this.presentFrame();
        if (!frame) return;

        // Decode the current frame off-thread into a finalized `ImageBitmap`,
        // downscaling to the display cap in the same pass when the source
        // overruns it. The bridge uploads (and closes) the bitmap at the next
        // drain. The `decoding` guard keeps at most one decode outstanding.
        let opts: ImageBitmapOptions = { ...(frame.options ?? {}) };
        const sw = frame.sourceWidth ?? 0;
        const sh = frame.sourceHeight ?? 0;
        if (sw > 0 && sh > 0) {
            const [tw, th] = this.cappedDimensions(sw, sh);
            if (tw !== sw || th !== sh) {
                opts = { ...opts, resizeWidth: tw, resizeHeight: th, resizeQuality: 'medium' };
            }
        }

        this.decoding = true;
        createImageBitmap(frame.source, opts)
            .then((bitmap) => {
                // The source may have been torn down or frozen mid-decode; drop
                // the now-orphaned bitmap rather than uploading a stale frame.
                if (this.stopped || this.frozen) {
                    bitmap.close();
                    return;
                }
                this.engine.uploadVoidExternalImage(this.layerId, bitmap);
                this.afterUpload();
                // Timelapse "start" milestone: the first frame of a connection
                // is now in the GPU, so ask the recorder to capture it. A no-op
                // when recording is off. Once per connection (reset on reconnect).
                if (!this.firstFrameUploaded) {
                    this.firstFrameUploaded = true;
                    this.engine.api.requestRecordingCapture();
                }
            })
            .catch(() => {
                // Decode can fail if the frame became unavailable (track ended
                // mid-flight, malformed WebP); ignore and retry next eligible tick.
            })
            .finally(() => {
                this.decoding = false;
            });
    }

    /** The current decodable frame, or `null` if none is available this tick. */
    protected abstract presentFrame(): PresentedFrame | null;

    /** Sink-side gate: is a frame worth decoding available this tick? Defaults to
     *  always true (a live feed is always "new"); a stream that only pushes on
     *  actual change overrides this so a static scene does zero work. */
    protected hasFrameReady(): boolean {
        return true;
    }

    /** Post-upload hook, run after a successful `uploadVoidExternalImage`.
     *  Default no-op; overridden to clear a "new frame" flag for change-driven
     *  sources. */
    protected afterUpload(): void {}

    /** Source dimensions clamped so the longest edge is at most
     *  `maxSourceDimension`, preserving aspect ratio. Returns the input unchanged
     *  when uncapped or already within bounds. */
    protected cappedDimensions(w: number, h: number): [number, number] {
        const longEdge = Math.max(w, h);
        if (this.maxSourceDimension <= 0 || longEdge <= this.maxSourceDimension) {
            return [w, h];
        }
        const scale = this.maxSourceDimension / longEdge;
        return [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
    }

    /** Update the upload throttle. Called by the reconciler on `frame_divisor`
     *  change. No counter to reset; the gate is a pure function of the shared
     *  master counter and the current divisor. */
    setFrameDivisor(n: number): void {
        this.frameDivisor = Math.max(1, Math.floor(n));
    }

    /** Update the upload resolution cap (document-canvas long edge). */
    setMaxSourceDimension(n: number): void {
        this.maxSourceDimension = Math.max(0, Math.floor(n));
    }

    /** Update the effective-visibility flag. */
    setVisible(visible: boolean): void {
        this.visible = visible;
    }

    /** Update the freeze (pause-uploads) flag. Keeps the feed open. */
    setFrozen(frozen: boolean): void {
        this.frozen = frozen;
    }

    /** Tear down the feed, free resources, and mark this source permanently
     *  dead. Safe to call multiple times. */
    abstract stop(): void;
}
