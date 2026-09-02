/**
 * MediaStream frame source for video-stream voids (camera + screenshare).
 *
 * A [`FrameSource`](./frameSource.ts) whose frames come from a `<video>` element
 * backed by a `MediaStream` the caller has already acquired (`getUserMedia` for
 * the camera, `getDisplayMedia` for screenshare, see `app.acquireMediaStream`).
 * The shared base owns the per-`tick()` gate, the off-thread
 * `createImageBitmap → uploadVoidExternalImage` upload, the resolution cap, and
 * the freeze / visibility / divisor setters; this subclass only supplies the
 * live `<video>` frame and the MediaStream lifecycle.
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
import { FrameSource, type CaptureKind, type PresentedFrame } from './frameSource';

// Re-export so existing importers (`app.svelte.ts`, `VoidPickerModal.svelte`)
// keep resolving `CaptureKind` here; the type's home is `frameSource.ts`.
export type { CaptureKind };

export class MediaStreamSource extends FrameSource {
    private video: HTMLVideoElement | null = null;
    private stream: MediaStream | null = null;
    private starting = false;
    /** True once `requestVideoFrameCallback` has fired at least once,
     *  confirming a decoded frame is available. Gates the first upload. */
    private hasFrame = false;

    constructor(
        layerId: number,
        engine: Engine,
        captureKind: CaptureKind,
        onEnded: ((layerId: number) => void) | null = null,
        onStatusChange: ((layerId: number) => void) | null = null,
    ) {
        super(layerId, engine, captureKind, onEnded, onStatusChange);
    }

    /** Wire up an already-acquired `MediaStream`: attach a `<video>` element,
     *  gate the first upload on a presented frame, and listen for external
     *  track-end. Resolves once the element is playing. Idempotent: calling
     *  twice (or after stop) is a no-op. */
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
            // Fire `onEnded` when the capture stops outside our control: the
            // common path for screenshare (the artist clicks the browser's "Stop
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
            // *presented*, see the requestVideoFrameCallback gate below.
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
            // between attach and play: bail without leaving a live element.
            if (this.stopped) {
                video.pause();
                video.srcObject = null;
                video.remove();
                return;
            }
            this.video = video;
            this.setStatus('connected');
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
            this.markFailed(describeMediaError(err, this.captureKind));
            if (this.stream) {
                this.stream.getTracks().forEach((t) => t.stop());
                this.stream = null;
            }
        } finally {
            this.starting = false;
        }
    }

    /** External track-end handler, also the unit-test seam, since a live
     *  `MediaStreamTrack` can't be faked in the node vitest env. Tears the
     *  source down, flips `ended`, and notifies the app so it can prune the
     *  source and re-show Resume. Idempotent. */
    handleTrackEnded(): void {
        if (this.ended) return;
        this.ended = true;
        this.setStatus('disconnected');
        this.stop();
        this.onEnded?.(this.layerId);
    }

    /** The video's current presented frame. `null` until a frame has been
     *  presented (`hasFrame`) or if the element reports zero dimensions (not
     *  yet ready / ended). */
    protected presentFrame(): PresentedFrame | null {
        if (!this.video || this.stopped || !this.hasFrame) return null;
        const vw = this.video.videoWidth;
        const vh = this.video.videoHeight;
        if (vw === 0 || vh === 0) return null;
        return { source: this.video, sourceWidth: vw, sourceHeight: vh };
    }

    /** A live camera/screenshare is always "new" once its first frame has been
     *  presented; every tick decodes whatever the video currently shows. */
    protected hasFrameReady(): boolean {
        return this.hasFrame;
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
        this.decoding = false;
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
            // getDisplayMedia rejects with NotAllowedError both when the artist
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
