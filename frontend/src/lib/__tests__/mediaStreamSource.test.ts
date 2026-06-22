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
