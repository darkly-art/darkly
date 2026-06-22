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
