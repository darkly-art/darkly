import { describe, it, expect, beforeEach, vi } from 'vitest';

// Capture the callbacks/spy the SW registration is wired with. `vi.hoisted`
// lets the (hoisted) `vi.mock` factory below reference these safely.
const mocks = vi.hoisted(() => ({
    updateSW: vi.fn(),
    captured: { onNeedRefresh: undefined as undefined | (() => void) },
}));

vi.mock('virtual:pwa-register', () => ({
    registerSW: (opts: { onNeedRefresh?: () => void }) => {
        mocks.captured.onNeedRefresh = opts.onNeedRefresh;
        return mocks.updateSW;
    },
}));

import { registerPwa } from '../pwa';
import { toast } from '../state/toast.svelte';

describe('pwa update prompt', () => {
    beforeEach(() => {
        toast.toasts = [];
        mocks.updateSW.mockClear();
        mocks.captured.onNeedRefresh = undefined;
    });

    it('stays quiet until the service worker reports a waiting update', () => {
        registerPwa();
        expect(mocks.captured.onNeedRefresh).toBeTypeOf('function');
        expect(toast.toasts).toHaveLength(0);
    });

    it('shows exactly one sticky "Reload" toast when a new version is waiting', () => {
        registerPwa();
        mocks.captured.onNeedRefresh!();

        expect(toast.toasts).toHaveLength(1);
        const t = toast.toasts[0];
        expect(t.message).toBe('New version available');
        expect(t.action?.label).toBe('Reload');
    });

    it('activates the waiting service worker and reloads when the action is taken', () => {
        registerPwa();
        mocks.captured.onNeedRefresh!();
        toast.toasts[0].action!.onClick();

        // updateSW(true) immediately activates the new SW and reloads the page.
        expect(mocks.updateSW).toHaveBeenCalledWith(true);
    });
});
