import { describe, it, expect, vi, afterEach } from 'vitest';
import { describeMediaError, MediaStreamSource } from '../mediaStreamSource';
import type { Engine } from '../../engine/protocol';

// No DOM / live MediaStream in the node test env, so we exercise the pure error
// mapper directly and drive the external-stop path through the `handleTrackEnded`
// test seam: the DOM-heavy `start()`/`tick()` paths aren't unit-testable here.

const engineStub = {} as Engine;

describe('describeMediaError', () => {
    const denied = { name: 'NotAllowedError' };

    it('words the permission-denied message per capture kind', () => {
        expect(describeMediaError(denied, 'camera')).toBe('Camera access was denied.');
        expect(describeMediaError(denied, 'display')).toBe('Screen share was denied or cancelled.');
    });

    it('words the not-found message per capture kind', () => {
        expect(describeMediaError({ name: 'NotFoundError' }, 'camera')).toBe(
            'No camera was found on this device.',
        );
        expect(describeMediaError({ name: 'NotFoundError' }, 'display')).toBe(
            'No screen or window was available to share.',
        );
    });

    it('falls back to a kind-prefixed message for unknown errors', () => {
        const msg = describeMediaError(new Error('boom'), 'display');
        expect(msg).toBe('Screen share failed to start: boom');
    });
});

describe('MediaStreamSource external stop (track ended)', () => {
    it('flips ended, stops, and fires onEnded once', () => {
        const ids: number[] = [];
        const src = new MediaStreamSource(42, engineStub, 'display', (id) => ids.push(id));

        expect(src.ended).toBe(false);
        src.handleTrackEnded();
        expect(src.ended).toBe(true);
        expect(ids).toEqual([42]);

        // Idempotent: a second end (or a `stop()` racing the listener) must
        // not re-notify, or the app would prune an already-pruned source.
        src.handleTrackEnded();
        expect(ids).toEqual([42]);
    });

    it('does not fire onEnded when there is no callback', () => {
        const src = new MediaStreamSource(7, engineStub, 'camera');
        expect(() => src.handleTrackEnded()).not.toThrow();
        expect(src.ended).toBe(true);
    });
});

// `tick()` decodes the frame off-thread via the global `createImageBitmap`, then
// uploads on the promise resolution. Node vitest has neither, so we stub
// `createImageBitmap` (honoring the resize opts so the fake bitmap reports the
// dimensions the upload will see) and flush microtasks between ticks.
afterEach(() => vi.unstubAllGlobals());

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function primedSource(videoWidth = 4, videoHeight = 4) {
    const uploads: Array<{ width: number; height: number }> = [];
    const optsSeen: ImageBitmapOptions[] = [];
    let closes = 0;
    vi.stubGlobal('createImageBitmap', (_src: unknown, opts?: ImageBitmapOptions) => {
        optsSeen.push(opts ?? {});
        return Promise.resolve({
            width: opts?.resizeWidth ?? videoWidth,
            height: opts?.resizeHeight ?? videoHeight,
            close: () => {
                closes++;
            },
        });
    });
    const engine = {
        uploadVoidExternalImage: (_layerId: number, bmp: ImageBitmap) =>
            uploads.push({ width: bmp.width, height: bmp.height }),
    } as unknown as Engine;
    const src = new MediaStreamSource(5, engine, 'display');
    // Stand in for the wiring `start()` would have done. The video stub carries
    // the no-op teardown methods `stop()` calls so tests can exercise it.
    const fields = src as unknown as { video: unknown; hasFrame: boolean };
    fields.video = {
        videoWidth,
        videoHeight,
        srcObject: null,
        pause: () => {},
        remove: () => {},
    };
    fields.hasFrame = true;
    return { src, uploads, optsSeen, closes: () => closes };
}

describe('MediaStreamSource freeze suppresses uploads without closing', () => {
    // Regression: freezing a screenshare used to tear down the source (the
    // reconciler called stop()), which ends a getDisplayMedia track for good;
    // unfreeze then showed nothing until the user re-picked. Freeze must only
    // gate `tick()`; the stream stays open so unfreeze resumes instantly.
    it('uploads when live, skips when frozen, resumes when unfrozen', async () => {
        const { src, uploads } = primedSource();

        // frameCount divisible by the default divisor (4) so the throttle gate
        // passes and the only thing under test is the freeze gate.
        src.tick(4);
        await flush();
        expect(uploads.length).toBe(1);

        src.setFrozen(true);
        src.tick(8);
        await flush();
        expect(uploads.length).toBe(1); // suppressed: no new upload

        // Still alive (not torn down): unfreeze resumes uploads without any
        // re-acquire.
        expect(src.ended).toBe(false);
        src.setFrozen(false);
        src.tick(12);
        await flush();
        expect(uploads.length).toBe(2);
    });

    it('drops a bitmap that resolves after the source stopped', async () => {
        const { src, uploads, closes } = primedSource();
        src.tick(4);
        src.stop(); // tear down before the decode resolves
        await flush();
        expect(uploads).toEqual([]); // never uploaded
        expect(closes()).toBe(1); // the orphaned bitmap was released
    });
});

describe('MediaStreamSource caps upload resolution to the display target', () => {
    // Regression: a screenshare uploaded its native frame, and the
    // `copyExternalImageToTexture` cross-context fence stalled the render loop
    // (~26ms drains). The compositor only samples the void at canvas resolution,
    // so the frame is downscaled to the cap during the off-thread decode,
    // preserving aspect so cover-fit is unaffected.
    it('downscales an oversized source to the cap, preserving aspect', async () => {
        const { src, uploads, optsSeen } = primedSource(3840, 2160);
        src.setMaxSourceDimension(1000);
        src.tick(4);
        await flush();
        // Long edge clamped to 1000; 2160 * (1000/3840) ≈ 563.
        expect(optsSeen[0]).toMatchObject({ resizeWidth: 1000, resizeHeight: 563 });
        expect(uploads).toEqual([{ width: 1000, height: 563 }]);
    });

    it('decodes at native size when already within the cap', async () => {
        const { src, uploads, optsSeen } = primedSource(640, 480);
        src.setMaxSourceDimension(1000);
        src.tick(4);
        await flush();
        expect(optsSeen[0]).toEqual({}); // no resize requested
        expect(uploads).toEqual([{ width: 640, height: 480 }]);
    });

    it('treats a zero/unset cap as native', async () => {
        const { src, uploads } = primedSource(3840, 2160);
        // Never called setMaxSourceDimension: default 0 means decode native.
        src.tick(4);
        await flush();
        expect(uploads).toEqual([{ width: 3840, height: 2160 }]);
    });
});

describe('FrameSource timelapse start milestone (base-class behavior)', () => {
    // A live void's streamed frames never bump the document revision, so the
    // recorder would miss its first frame. The base fires `requestRecordingCapture`
    // on the first upload of each connection so the recording captures the void
    // appearing. Exercised through MediaStreamSource (the base is abstract).
    function sourceWithCaptureSpy() {
        vi.stubGlobal('createImageBitmap', (_s: unknown, opts?: ImageBitmapOptions) =>
            Promise.resolve({
                width: opts?.resizeWidth ?? 4,
                height: opts?.resizeHeight ?? 4,
                close: () => {},
            }),
        );
        let captures = 0;
        const engine = {
            uploadVoidExternalImage: () => {},
            api: {
                requestRecordingCapture: () => {
                    captures++;
                },
            },
        } as unknown as Engine;
        const src = new MediaStreamSource(5, engine, 'display');
        const fields = src as unknown as { video: unknown; hasFrame: boolean };
        fields.video = {
            videoWidth: 4,
            videoHeight: 4,
            srcObject: null,
            pause: () => {},
            remove: () => {},
        };
        fields.hasFrame = true;
        return { src, captures: () => captures };
    }

    it('requests one capture on the first frame, then not again on the same connection', async () => {
        const { src, captures } = sourceWithCaptureSpy();

        src.tick(4);
        await flush();
        expect(captures()).toBe(1);

        // Later frames of the same connection must not re-fire the milestone.
        src.tick(8);
        await flush();
        expect(captures()).toBe(1);
    });

    it('re-arms on a fresh connection (connected transition)', async () => {
        const { src, captures } = sourceWithCaptureSpy();

        src.tick(4);
        await flush();
        expect(captures()).toBe(1);

        // A reconnect lands on `connected`, opening a new frame epoch whose
        // first upload is again a start worth capturing.
        (src as unknown as { setStatus(s: string): void }).setStatus('connected');
        src.tick(8);
        await flush();
        expect(captures()).toBe(2);
    });
});
