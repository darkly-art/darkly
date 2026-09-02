import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { watchDismiss } from '../dismiss';

// jsdom isn't available in this project, so we exercise the helper against a
// stubbed `window` and fake event targets. That's enough to pin the behaviour
// that matters: it dismisses on `pointerdown` (not click; the canvas
// suppresses that) for any target outside the popup's own scope.

let listeners: Record<string, (e: { target: unknown }) => void>;
let removed: string[];

beforeEach(() => {
    listeners = {};
    removed = [];
    vi.stubGlobal('window', {
        addEventListener: (type: string, h: (e: { target: unknown }) => void) => {
            listeners[type] = h;
        },
        removeEventListener: (type: string) => {
            removed.push(type);
        },
    });
});
afterEach(() => vi.unstubAllGlobals());

// A fake event target whose `closest(selector)` resolves only for the given
// scope's keep-open selector.
function target(scope: string | null) {
    return {
        closest: (sel: string) => (scope && sel === `[data-keep-open~="${scope}"]` ? {} : null),
    };
}

describe('watchDismiss', () => {
    it('listens on pointerdown, not click (canvas suppresses click)', () => {
        watchDismiss('menu', () => {});
        expect(Object.keys(listeners)).toEqual(['pointerdown']);
        expect(listeners.click).toBeUndefined();
    });

    it('dismisses for targets outside its scope, keeps open for its own controls', () => {
        const cb = vi.fn();
        watchDismiss('menu', cb);

        listeners.pointerdown({ target: target(null) }); // bar chrome / padding / canvas
        expect(cb).toHaveBeenCalledTimes(1);

        listeners.pointerdown({ target: target('menu') }); // a control in this scope
        expect(cb).toHaveBeenCalledTimes(1);
    });

    it('ignores other popups\' controls: a different scope still dismisses this one', () => {
        const cb = vi.fn();
        watchDismiss('menu', cb);
        listeners.pointerdown({ target: target('brush-picker') });
        expect(cb).toHaveBeenCalledTimes(1);
    });

    it('teardown unbinds the listener', () => {
        const stop = watchDismiss('menu', () => {});
        stop();
        expect(removed).toContain('pointerdown');
    });
});
