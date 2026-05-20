/**
 * MediaStream lifecycle for the Camera void.
 *
 * Owns one `<video>` element backed by `getUserMedia({ video: true })`, and
 * exposes a `tick()` method that hands the live frame to the WASM bridge.
 * The WebGPU backend's `copy_external_image_to_texture` consumes an
 * `HTMLVideoElement` directly, so there's no per-frame ImageBitmap or
 * canvas allocation in the hot path — once the stream is running, each
 * `tick()` is one `queue.copy_external_image_to_texture` and that's it.
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

    /** 2D-context-backed canvas we blit the video frame into each tick.
     *  We could pass `<video>` directly to `copyExternalImageToTexture`, but
     *  some Chromium configurations silently no-op that path (texture stays
     *  black). Blitting through a canvas first is the reliably-working route
     *  for video → WebGPU and is what the official samples do. The blit
     *  itself stays GPU-side in modern Chromium (no CPU readback). */
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
     *  video isn't ready yet (no-op) — safe to call every animation frame. */
    tick(): void {
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
