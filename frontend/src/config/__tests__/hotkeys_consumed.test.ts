// @vitest-environment jsdom
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

// A key a component consumed (Escape closing the colour popup, ending a
// drag) is not a hotkey: with Escape bound to Discard, closing a popup must
// not also discard the open edit session.

vi.mock('../store.svelte', () => ({
    effectiveHotkeys: (id: string) => (id === 'discardEditSession' ? ['Escape'] : []),
}));
vi.mock('../../state/app.svelte', () => ({
    app: { activeToolId: 'brush', activeLayerId: null },
}));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: { activeBrush: null } }));
vi.mock('../../tools/registry', () => ({ toolRegistry: { get: () => undefined } }));

import { actions } from '../../actions/registry';
import { registerHotkeys, unregisterHotkeys } from '../hotkeys.svelte';

const discard = vi.fn();

beforeAll(() => {
    actions.register({ id: 'discardEditSession', handler: discard });
    registerHotkeys();
});

afterEach(() => {
    discard.mockClear();
});

function pressEscape() {
    document.body.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', bubbles: true, cancelable: true }),
    );
}

describe('a consumed key', () => {
    it('dispatches an unconsumed Escape', () => {
        pressEscape();
        expect(discard).toHaveBeenCalledOnce();
    });

    it('does not dispatch an Escape a capture-phase listener consumed', () => {
        // Registered after the hotkeys, as a popup that opens later is.
        const consume = (e: KeyboardEvent) => {
            if (e.key === 'Escape') e.preventDefault();
        };
        window.addEventListener('keydown', consume, true);
        try {
            pressEscape();
        } finally {
            window.removeEventListener('keydown', consume, true);
        }
        expect(discard).not.toHaveBeenCalled();
    });
});

afterAll(() => unregisterHotkeys());
