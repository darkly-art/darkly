import { describe, it, expect } from 'vitest';
import { describeMediaError, MediaStreamSource } from '../mediaStreamSource';
import type { Engine } from '../../engine/protocol';

// No DOM / live MediaStream in the node test env, so we exercise the pure error
// mapper directly and drive the external-stop path through the `handleTrackEnded`
// test seam — the DOM-heavy `start()`/`tick()` paths aren't unit-testable here.

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

        // Idempotent — a second end (or a `stop()` racing the listener) must
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

describe('MediaStreamSource freeze suppresses uploads without closing', () => {
    // Regression: freezing a screenshare used to tear down the source (the
    // reconciler called stop()), which ends a getDisplayMedia track for good —
    // unfreeze then showed nothing until the user re-picked. Freeze must only
    // gate `tick()`; the stream stays open so unfreeze resumes instantly.
    //
    // The DOM-heavy `start()` isn't runnable in node vitest, so we inject the
    // fields `tick()` reads and assert it uploads only when not frozen.
    function primedSource() {
        const uploads: number[] = [];
        const engine = {
            uploadVoidExternalImage: (layerId: number) => uploads.push(layerId),
        } as unknown as Engine;
        const src = new MediaStreamSource(5, engine, 'display');
        // Stand in for the wiring `start()` would have done.
        const fields = src as unknown as {
            video: unknown;
            canvas: unknown;
            ctx: unknown;
            hasFrame: boolean;
        };
        fields.video = { videoWidth: 4, videoHeight: 4 };
        fields.canvas = { width: 4, height: 4 };
        fields.ctx = { drawImage: () => {} };
        fields.hasFrame = true;
        return { src, uploads };
    }

    it('uploads when live, skips when frozen, resumes when unfrozen', () => {
        const { src, uploads } = primedSource();

        // frameCount divisible by the default divisor (4) so the throttle gate
        // passes and the only thing under test is the freeze gate.
        src.tick(4);
        expect(uploads).toEqual([5]);

        src.setFrozen(true);
        src.tick(8);
        expect(uploads).toEqual([5]); // suppressed — no new upload

        // Still alive (not torn down): unfreeze resumes uploads without any
        // re-acquire.
        expect(src.ended).toBe(false);
        src.setFrozen(false);
        src.tick(12);
        expect(uploads).toEqual([5, 5]);
    });
});

describe('MediaStreamSource caps upload resolution to the display target', () => {
    // Regression: a 4K screenshare uploaded its native 3840×2160 frame every
    // tick, and the synchronous `copyExternalImageToTexture` of ~33 MB stalled
    // the render loop (~26 ms drains). The compositor only samples the void at
    // canvas resolution, so the blit canvas must be downscaled to the cap
    // before the upload — preserving aspect so cover-fit is unaffected.
    function primedSource(videoWidth: number, videoHeight: number) {
        // Record the blit-canvas dimensions at upload time (the upload happens
        // after tick() has sized the canvas and drawn into it).
        const uploads: Array<{ w: number; h: number }> = [];
        const canvas = { width: 0, height: 0 };
        const engine = {
            uploadVoidExternalImage: () => uploads.push({ w: canvas.width, h: canvas.height }),
        } as unknown as Engine;
        const src = new MediaStreamSource(7, engine, 'display');
        const draws: Array<{ w: number; h: number }> = [];
        const fields = src as unknown as {
            video: unknown;
            canvas: unknown;
            ctx: unknown;
            hasFrame: boolean;
        };
        fields.video = { videoWidth, videoHeight };
        fields.canvas = canvas;
        fields.ctx = {
            drawImage: (_img: unknown, _x: number, _y: number, w: number, h: number) =>
                draws.push({ w, h }),
        };
        fields.hasFrame = true;
        return { src, uploads, draws };
    }

    it('downscales an oversized source to the cap, preserving aspect', () => {
        const { src, uploads, draws } = primedSource(3840, 2160);
        src.setMaxSourceDimension(1000);
        src.tick(4);
        // Long edge clamped to 1000; 2160 * (1000/3840) ≈ 563.
        expect(uploads).toEqual([{ w: 1000, h: 563 }]);
        // The blit drew into the capped dest rect (GPU-side downscale).
        expect(draws).toEqual([{ w: 1000, h: 563 }]);
    });

    it('leaves a source already within the cap untouched', () => {
        const { src, uploads } = primedSource(640, 480);
        src.setMaxSourceDimension(1000);
        src.tick(4);
        expect(uploads).toEqual([{ w: 640, h: 480 }]);
    });

    it('treats a zero/unset cap as no cap', () => {
        const { src, uploads } = primedSource(3840, 2160);
        // Never called setMaxSourceDimension — default 0 means upload native.
        src.tick(4);
        expect(uploads).toEqual([{ w: 3840, h: 2160 }]);
    });
});
