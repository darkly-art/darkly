/**
 * MediaStream lifecycle for the Camera void.
 *
 * Owns one `<video>` element backed by `getUserMedia({ video: true })`, and
 * exposes a `tick()` method that hands the live frame to the WASM bridge.
 * Each tick blits the video into a backing `OffscreenCanvas` via
 * `drawImage`, then passes that canvas to `copy_external_image_to_texture`.
 *
 * Why the canvas hop? The WebGPU spec's `GPUCopyExternalImage.source` lists
 * `HTMLVideoElement`, but Firefox's WebGPU rejects it at runtime (only
 * canvas-family + ImageBitmap + HTMLImageElement sources are accepted), and
 * some Chromium configurations silently no-op the video-direct path
 * (texture stays zero). The canvas route is the cross-browser path used by
 * the official WebGPU samples; the `drawImage` stays GPU-side in modern
 * Chromium (no CPU readback).
 *
 * Permission UX: we delegate entirely to the browser. `getUserMedia`'s
 * native prompt includes a device picker on Chromium when multiple cameras
 * are present, so we don't ship our own picker.
 *
 * Electron: identical code path. The host's main process must grant the
 * `'media'` permission (see the camera-void plan); otherwise
 * `getUserMedia` rejects with `NotAllowedError` and `this.error` surfaces
 * a notice in `VoidProperties.svelte`.
 */

import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';

export class CameraSource {
    readonly layerId: number;
    private readonly handle: DarklyHandle;
    private video: HTMLVideoElement | null = null;
    private stream: MediaStream | null = null;
    private starting = false;
    private stopped = false;
    /** True once `requestVideoFrameCallback` has fired at least once,
     *  confirming a decoded frame is available. Gates the first upload. */
    private hasFrame = false;

    /** How many `tick()` calls to skip between actual uploads. Mirrors the
     *  `frame_divisor` param on the Rust-side `Camera` void; the value is
     *  pushed in by the layer-tree reconciler whenever the user adjusts the
     *  slider. 1 = upload every rAF, 4 = upload every 4th rAF (~15fps at
     *  60Hz; the default), etc. Higher values save the per-frame canvas
     *  blit, GPU copy, void shader pass, and full compositor re-encode.
     *
     *  The gate is `frameCount % frameDivisor === 0`, where `frameCount` is
     *  the canonical master counter from the compositor (`handle.frame_count`).
     *  This is the same counter the Rust-side veil / overlay / void
     *  animation divisors gate against (see
     *  `Compositor::update_animations`), so a camera `divisor=N` fires on
     *  the exact rAF a veil `divisor=N` fires on — the throttled upload
     *  lands on a frame the compositor was going to re-render anyway. */
    private frameDivisor = 4;

    /** Effective visibility (self + every ancestor visible) for the void's
     *  layer. Pushed in by the layer-tree reconciler. When false, `tick()`
     *  short-circuits — no canvas blit, no WASM call, no GPU work. The Rust
     *  side also gates `wants_external_input` on its own visibility flag, so
     *  this is the JS-local optimization (skip the `drawImage` and the
     *  bridge call) and Rust is the canonical correctness guard. */
    private visible = true;

    /** 2D-context-backed canvas we blit the video frame into each tick.
     *  Required because Firefox's WebGPU rejects `HTMLVideoElement` as a
     *  `copyExternalImageToTexture` source, and some Chromium configs
     *  silently no-op the video-direct path. The blit itself stays GPU-side
     *  in modern Chromium. */
    private canvas: OffscreenCanvas | null = null;
    private ctx: OffscreenCanvasRenderingContext2D | null = null;

    /** Human-readable error if start failed (permission denied, no device,
     *  etc.). Reactive Svelte readers in VoidProperties pull this directly. */
    error: string | null = null;

    constructor(layerId: number, handle: DarklyHandle) {
        this.layerId = layerId;
        this.handle = handle;
    }

    /** Begin the MediaStream. Resolves once the first frame is available or
     *  rejects with a user-friendly error string assigned to `this.error`.
     *  Idempotent: calling twice is a no-op. */
    async start(): Promise<void> {
        if (this.starting || this.video || this.stopped) return;
        this.starting = true;
        try {
            // `getUserMedia` triggers the browser's native consent UI on
            // first use. On Chromium the prompt includes the device picker,
            // so multi-camera systems work without our own picker.
            this.stream = await navigator.mediaDevices.getUserMedia({
                video: true,
                audio: false,
            });
            // The browser may have stopped the layer between request and
            // grant — bail without wiring up the video element.
            if (this.stopped) {
                this.stream.getTracks().forEach((t) => t.stop());
                this.stream = null;
                return;
            }
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
            this.error = describeMediaError(err);
            if (this.stream) {
                this.stream.getTracks().forEach((t) => t.stop());
                this.stream = null;
            }
        } finally {
            this.starting = false;
        }
    }

    /** Push the current frame into the void's input texture. Cheap when the
     *  video isn't ready yet (no-op) — safe to call every animation frame.
     *
     *  `frameCount` is the canonical master tick from the compositor (see
     *  `DarklyHandle.frame_count`). Using it directly — rather than a
     *  per-source rolling counter — keeps the gate phase-locked with every
     *  other divisor-throttled system in the engine: a camera with
     *  `divisor=4` will fire on the same rAF as a veil with `divisor=4`,
     *  not one rAF off. */
    tick(frameCount: number): void {
        if (!this.visible) return;
        if (frameCount % this.frameDivisor !== 0) return;
        if (!this.video || !this.canvas || !this.ctx || this.stopped || !this.hasFrame) return;
        const vw = this.video.videoWidth;
        const vh = this.video.videoHeight;
        if (vw === 0 || vh === 0) return;
        if (this.canvas.width !== vw || this.canvas.height !== vh) {
            this.canvas.width = vw;
            this.canvas.height = vh;
        }
        this.ctx.drawImage(this.video, 0, 0, vw, vh);
        this.handle.upload_void_external_image(this.layerId, this.canvas);
    }

    /** Update the upload throttle. Called by the layer-tree reconciler when
     *  the user adjusts the `frame_divisor` param. No counter to reset —
     *  the gate is a pure function of the shared master counter and the
     *  current divisor, so a slider change takes effect on the next rAF. */
    setFrameDivisor(n: number): void {
        this.frameDivisor = Math.max(1, Math.floor(n));
    }

    /** Update the effective-visibility flag. Called by the layer-tree
     *  reconciler whenever any node on the path from root to this camera
     *  void changes its eye state. */
    setVisible(visible: boolean): void {
        this.visible = visible;
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

function describeMediaError(err: unknown): string {
    // DOMException codes from the MediaStream spec.
    const name = (err as { name?: string })?.name;
    switch (name) {
        case 'NotAllowedError':
        case 'PermissionDeniedError':
            return 'Camera access was denied.';
        case 'NotFoundError':
        case 'DevicesNotFoundError':
            return 'No camera was found on this device.';
        case 'NotReadableError':
        case 'TrackStartError':
            return 'The camera is already in use by another application.';
        case 'OverconstrainedError':
            return 'No camera satisfies the requested constraints.';
        case 'SecurityError':
            return 'Camera access blocked by browser security settings.';
        default:
            return `Camera failed to start: ${(err as Error)?.message ?? String(err)}`;
    }
}
