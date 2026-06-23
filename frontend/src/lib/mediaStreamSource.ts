/**
 * MediaStream lifecycle for video-stream voids (camera + screenshare).
 *
 * Owns one `<video>` element backed by a `MediaStream` the caller has already
 * acquired (`getUserMedia` for the camera, `getDisplayMedia` for screenshare —
 * see `app.acquireMediaStream`). Exposes a `tick()` method that hands the live
 * frame to the WASM bridge. Each tick blits the video into a backing
 * `OffscreenCanvas` via `drawImage`, then passes that canvas to
 * `copy_external_image_to_texture`.
 *
 * Why the canvas hop? The WebGPU spec's `GPUCopyExternalImage.source` lists
 * `HTMLVideoElement`, but Firefox's WebGPU rejects it at runtime (only
 * canvas-family + ImageBitmap + HTMLImageElement sources are accepted), and
 * some Chromium configurations silently no-op the video-direct path (texture
 * stays zero). The canvas route is the cross-browser path used by the official
 * WebGPU samples; the `drawImage` stays GPU-side in modern Chromium (no CPU
 * readback).
 *
 * Acquisition lives in the gesture (`app.acquireMediaStream`), NOT here:
 * `getDisplayMedia` requires transient user activation, which expires across the
 * picker → add_void → reconcile round-trip. By acquiring the stream up front and
 * handing it to `start(stream)`, this class never sits between the click and the
 * capture API.
 *
 * Electron: identical code path. The host's main process must grant the
 * `'media'` (and, for screenshare, the display-capture) permission; otherwise
 * acquisition rejects with `NotAllowedError` and `describeMediaError` surfaces a
 * notice in `VoidProperties.svelte`.
 */

import type { Engine } from '../engine/protocol';

/** Browser capture API a void's frames come from. Mirrors the Rust
 *  `CaptureKind` serialization (`engine/void.rs`). */
export type CaptureKind = 'camera' | 'display';

export class MediaStreamSource {
    readonly layerId: number;
    readonly captureKind: CaptureKind;
    private readonly engine: Engine;
    /** Invoked when the capture track ends *externally* (the user clicks the
     *  browser's "Stop sharing" bar, or unplugs the webcam). The app uses it to
     *  prune the source and re-show the "Resume" affordance. */
    private readonly onEnded: ((layerId: number) => void) | null;
    private video: HTMLVideoElement | null = null;
    private stream: MediaStream | null = null;
    private starting = false;
    private stopped = false;
    /** True once the capture track has ended externally. Observable so the
     *  properties panel can distinguish "never started" from "stopped by the
     *  OS" and offer Resume in both cases. */
    ended = false;
    /** True once `requestVideoFrameCallback` has fired at least once,
     *  confirming a decoded frame is available. Gates the first upload. */
    private hasFrame = false;

    /** How many `tick()` calls to skip between actual uploads. Mirrors the
     *  `frame_divisor` param on the Rust-side void; the value is pushed in by
     *  the layer-tree reconciler whenever the user adjusts the slider. 1 =
     *  upload every rAF, 4 = upload every 4th rAF (~15fps at 60Hz; the
     *  default), etc. Higher values save the per-frame canvas blit, GPU copy,
     *  void shader pass, and full compositor re-encode.
     *
     *  The gate is `frameCount % frameDivisor === 0`, where `frameCount` is the
     *  canonical master counter from the compositor (`handle.frame_count`).
     *  This is the same counter the Rust-side veil / overlay / void animation
     *  divisors gate against (see `Compositor::update_animations`), so a
     *  `divisor=N` source fires on the exact rAF a veil `divisor=N` fires on —
     *  the throttled upload lands on a frame the compositor was going to
     *  re-render anyway. */
    private frameDivisor = 4;

    /** Effective visibility (self + every ancestor visible) for the void's
     *  layer. Pushed in by the layer-tree reconciler. When false, `tick()`
     *  short-circuits — no canvas blit, no WASM call, no GPU work. The Rust
     *  side also gates `wants_external_input` on its own visibility flag, so
     *  this is the JS-local optimization (skip the `drawImage` and the bridge
     *  call) and Rust is the canonical correctness guard. */
    private visible = true;

    /** Longest edge (px) the blit canvas — and therefore the GPU upload — is
     *  allowed to reach; the source is downscaled to fit, preserving aspect.
     *  Pushed in by the reconciler as the document-canvas long edge. The
     *  compositor renders this void cover-fit into a canvas-resolution target,
     *  so source detail finer than the canvas can never be displayed — uploading
     *  it just burns `copyExternalImageToTexture` CPU time every frame. This
     *  matters for screenshare, where the source can be a full 4K monitor
     *  (~33 MB/frame) feeding a far smaller document. `0` means "no cap" (the
     *  safe fallback before a value is pushed). */
    private maxSourceDimension = 0;

    /** Whether the void's `freeze` param is on. Pushed in by the reconciler.
     *  When frozen, `tick()` stops uploading so the GPU holds the last received
     *  frame — but the `MediaStream` stays **open**, so unfreezing resumes
     *  instantly without re-prompting. This matters most for screenshare:
     *  stopping a `getDisplayMedia` track ends the share permanently (there's
     *  no silent re-acquire), so freeze must suppress, not tear down. The Rust
     *  side independently drops frames via `wants_external_input` while frozen;
     *  this is the JS-local skip of the wasted blit + bridge call. */
    private frozen = false;

    /** 2D-context-backed canvas we blit the video frame into each tick.
     *  Required because Firefox's WebGPU rejects `HTMLVideoElement` as a
     *  `copyExternalImageToTexture` source, and some Chromium configs silently
     *  no-op the video-direct path. The blit itself stays GPU-side in modern
     *  Chromium. */
    private canvas: OffscreenCanvas | null = null;
    private ctx: OffscreenCanvasRenderingContext2D | null = null;

    /** Human-readable error if start failed (permission denied, no device,
     *  etc.). Reactive Svelte readers in VoidProperties pull this directly. */
    error: string | null = null;

    constructor(
        layerId: number,
        engine: Engine,
        captureKind: CaptureKind,
        onEnded: ((layerId: number) => void) | null = null,
    ) {
        this.layerId = layerId;
        this.engine = engine;
        this.captureKind = captureKind;
        this.onEnded = onEnded;
    }

    /** Wire up an already-acquired `MediaStream`: attach a `<video>` element,
     *  allocate the blit canvas, gate the first upload on a presented frame,
     *  and listen for external track-end. Resolves once the element is playing.
     *  Idempotent: calling twice (or after stop) is a no-op. */
    async start(stream: MediaStream): Promise<void> {
        if (this.starting || this.video || this.stopped) {
            // We took ownership of the stream; if we're not going to use it,
            // stop its tracks so the OS capture indicator doesn't linger.
            if (this.stopped) stream.getTracks().forEach((t) => t.stop());
            return;
        }
        this.starting = true;
        this.stream = stream;
        try {
            // Fire `onEnded` when the capture stops outside our control — the
            // common path for screenshare (the user clicks the browser's "Stop
            // sharing" bar), and possible for the camera (device unplugged).
            const track = stream.getVideoTracks()[0];
            track?.addEventListener('ended', () => this.handleTrackEnded());

            const video = document.createElement('video');
            video.autoplay = true;
            video.playsInline = true;
            // Required by Safari + some Chromium configurations to start
            // playback without a user gesture on each tab.
            video.muted = true;
            // Off-screen but in the DOM. Chromium will decode frames into a
            // backing texture even without DOM attachment, but in practice
            // `copyExternalImageToTexture` reliably reads real pixels only
            // after the element has been attached and a frame has been
            // *presented* — see the requestVideoFrameCallback gate below.
            video.style.position = 'fixed';
            video.style.left = '-9999px';
            video.style.top = '0';
            video.style.width = '1px';
            video.style.height = '1px';
            video.style.pointerEvents = 'none';
            video.setAttribute('aria-hidden', 'true');
            document.body.appendChild(video);
            video.srcObject = this.stream;
            await video.play();
            // The stream may have been torn down (track ended, layer deleted)
            // between attach and play — bail without leaving a live element.
            if (this.stopped) {
                video.pause();
                video.srcObject = null;
                video.remove();
                return;
            }
            this.video = video;
            // Allocate the blit canvas at the video's current dimensions.
            // We resize lazily in `tick()` if the video reports new dims
            // later (rare but possible for some sources).
            this.canvas = new OffscreenCanvas(video.videoWidth, video.videoHeight);
            this.ctx = this.canvas.getContext('2d');
            // Gate uploads on a real presented frame, not just readyState.
            // `requestVideoFrameCallback` fires per presented frame; we just
            // need the first one to flip the flag and then leave it alone
            // (subsequent ticks pull whatever frame the video is currently
            // presenting).
            const rvfc = (
                video as HTMLVideoElement & {
                    requestVideoFrameCallback?: (cb: () => void) => number;
                }
            ).requestVideoFrameCallback?.bind(video);
            if (rvfc) {
                rvfc(() => {
                    this.hasFrame = true;
                });
            } else {
                // Fallback (older browsers / no rVFC support): assume the
                // frame is ready once readyState says so. Less reliable but
                // still better than nothing.
                this.hasFrame = video.readyState >= 2;
            }
        } catch (err: any) {
            // Translate the cryptic DOMException names into something the
            // VoidProperties notice can show without a switch on the JS side.
            this.error = describeMediaError(err, this.captureKind);
            if (this.stream) {
                this.stream.getTracks().forEach((t) => t.stop());
                this.stream = null;
            }
        } finally {
            this.starting = false;
        }
    }

    /** External track-end handler — also the unit-test seam, since a live
     *  `MediaStreamTrack` can't be faked in the node vitest env. Tears the
     *  source down, flips `ended`, and notifies the app so it can prune the
     *  source and re-show Resume. Idempotent. */
    handleTrackEnded(): void {
        if (this.ended) return;
        this.ended = true;
        this.stop();
        this.onEnded?.(this.layerId);
    }

    /** Push the current frame into the void's input texture. Cheap when the
     *  video isn't ready yet (no-op) — safe to call every animation frame.
     *
     *  `frameCount` is the canonical master tick from the compositor (see
     *  `DarklyHandle.frame_count`). Using it directly — rather than a
     *  per-source rolling counter — keeps the gate phase-locked with every
     *  other divisor-throttled system in the engine: a source with `divisor=4`
     *  fires on the same rAF as a veil with `divisor=4`, not one rAF off. */
    tick(frameCount: number): void {
        if (!this.visible || this.frozen) return;
        if (frameCount % this.frameDivisor !== 0) return;
        if (!this.video || !this.canvas || !this.ctx || this.stopped || !this.hasFrame) return;
        const vw = this.video.videoWidth;
        const vh = this.video.videoHeight;
        if (vw === 0 || vh === 0) return;
        // Downscale to the display cap before the blit, so both `drawImage` and
        // the GPU upload operate on canvas-resolution pixels rather than the
        // source's native (potentially 4K) frame. `drawImage`'s dest-rect form
        // does the scale GPU-side; the smaller canvas is what actually slashes
        // the per-frame `copyExternalImageToTexture` cost.
        const [tw, th] = this.cappedDimensions(vw, vh);
        if (this.canvas.width !== tw || this.canvas.height !== th) {
            this.canvas.width = tw;
            this.canvas.height = th;
        }
        this.ctx.drawImage(this.video, 0, 0, tw, th);
        this.engine.uploadVoidExternalImage(this.layerId, this.canvas);
    }

    /** Source dimensions clamped so the longest edge is at most
     *  `maxSourceDimension`, preserving aspect ratio (so the void's cover-fit
     *  math, which keys off the uploaded texture's aspect, is unaffected).
     *  Returns the input unchanged when uncapped or already within bounds. */
    private cappedDimensions(w: number, h: number): [number, number] {
        const longEdge = Math.max(w, h);
        if (this.maxSourceDimension <= 0 || longEdge <= this.maxSourceDimension) {
            return [w, h];
        }
        const scale = this.maxSourceDimension / longEdge;
        return [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
    }

    /** Update the upload throttle. Called by the layer-tree reconciler when
     *  the user adjusts the `frame_divisor` param. No counter to reset —
     *  the gate is a pure function of the shared master counter and the
     *  current divisor, so a slider change takes effect on the next rAF. */
    setFrameDivisor(n: number): void {
        this.frameDivisor = Math.max(1, Math.floor(n));
    }

    /** Update the upload resolution cap (document-canvas long edge). Called by
     *  the layer-tree reconciler and at start; takes effect on the next blit. */
    setMaxSourceDimension(n: number): void {
        this.maxSourceDimension = Math.max(0, Math.floor(n));
    }

    /** Update the effective-visibility flag. Called by the layer-tree
     *  reconciler whenever any node on the path from root to this void
     *  changes its eye state. */
    setVisible(visible: boolean): void {
        this.visible = visible;
    }

    /** Update the freeze (pause-uploads) flag. Called by the layer-tree
     *  reconciler when the user toggles the void's `freeze` param. Keeps the
     *  stream open — see the `frozen` field. */
    setFrozen(frozen: boolean): void {
        this.frozen = frozen;
    }

    /** Stop the MediaStream, free the video element, and mark this source
     *  permanently dead. Safe to call multiple times. */
    stop(): void {
        this.stopped = true;
        if (this.stream) {
            this.stream.getTracks().forEach((t) => t.stop());
            this.stream = null;
        }
        if (this.video) {
            this.video.pause();
            this.video.srcObject = null;
            this.video.remove();
            this.video = null;
        }
        this.canvas = null;
        this.ctx = null;
        this.hasFrame = false;
    }
}

/** Translate a `MediaDevices` rejection into a human-readable, capture-kind-aware
 *  message for the VoidProperties notice. Exported for unit testing. */
export function describeMediaError(err: unknown, captureKind: CaptureKind): string {
    const name = (err as { name?: string })?.name;
    const subject = captureKind === 'display' ? 'Screen share' : 'Camera';
    switch (name) {
        case 'NotAllowedError':
        case 'PermissionDeniedError':
            // getDisplayMedia rejects with NotAllowedError both when the user
            // cancels the OS picker and when sharing is policy-blocked.
            return captureKind === 'display'
                ? 'Screen share was denied or cancelled.'
                : 'Camera access was denied.';
        case 'NotFoundError':
        case 'DevicesNotFoundError':
            return captureKind === 'display'
                ? 'No screen or window was available to share.'
                : 'No camera was found on this device.';
        case 'NotReadableError':
        case 'TrackStartError':
            return `${subject} is already in use by another application.`;
        case 'OverconstrainedError':
            return `No ${captureKind === 'display' ? 'display' : 'camera'} satisfies the requested constraints.`;
        case 'SecurityError':
            return `${subject} blocked by browser security settings.`;
        case 'AbortError':
            return `${subject} could not start (aborted).`;
        default:
            return `${subject} failed to start: ${(err as Error)?.message ?? String(err)}`;
    }
}
