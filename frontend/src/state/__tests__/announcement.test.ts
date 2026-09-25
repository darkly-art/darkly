import { afterEach, describe, expect, it, vi } from 'vitest';
import { Announcement } from '../announcement.svelte';

/** In-memory stand-in for `localStorage`, which node does not provide.
 *  `persistedState` reaches for the bare global, so stubbing it globally is
 *  what reaches the code under test. (`recoverySession` takes an injected
 *  `KeyValueStore` instead; this module does not.) */
function fakeStorage() {
    const map = new Map<string, string>();
    return {
        store: map,
        getItem: (k: string) => map.get(k) ?? null,
        setItem: (k: string, v: string) => void map.set(k, v),
        removeItem: (k: string) => void map.delete(k),
    };
}

function withStorage() {
    const fake = fakeStorage();
    vi.stubGlobal('localStorage', fake);
    return fake;
}

afterEach(() => {
    vi.unstubAllGlobals();
});

describe('the build announcement', () => {
    it('shows a configured message that has not been dismissed', () => {
        withStorage();

        expect(new Announcement('<b>beta</b>').visible).toBe(true);
    });

    it('does not exist when the build configured no message', () => {
        const fake = withStorage();

        expect(new Announcement('').visible).toBe(false);
        // Whitespace is not content: deploy pipelines hand over trailing
        // newlines and those must not count as a message.
        expect(new Announcement('  \n ').visible).toBe(false);
        expect(fake.store.size).toBe(0);
    });

    it('stays dismissed across a reload', () => {
        const fake = withStorage();

        new Announcement('<b>beta</b>').dismiss();

        expect(new Announcement('<b>beta</b>').visible).toBe(false);
        expect(fake.store.get('darkly.announcementDismissed')).toBe(
            JSON.stringify('<b>beta</b>'),
        );
    });

    it('re-announces itself when the build ships different copy', () => {
        const fake = withStorage();
        new Announcement('<b>beta</b>').dismiss();

        const next = new Announcement('<b>thanks patrons</b>');
        expect(next.visible).toBe(true);

        // Dismissing the new one replaces the old record rather than
        // accumulating a key per message ever shipped.
        next.dismiss();
        expect(fake.store.size).toBe(1);
        expect(fake.store.get('darkly.announcementDismissed')).toBe(
            JSON.stringify('<b>thanks patrons</b>'),
        );
    });

    it('stays dismissed when the same copy is redeployed, trailing newline and all', () => {
        withStorage();
        new Announcement('<b>beta</b>').dismiss();

        expect(new Announcement('<b>beta</b>\n').visible).toBe(false);
    });

    it('still shows when the browser denies storage', () => {
        // No stub: `localStorage` is not a binding at all here, which is what
        // a privacy-locked browser amounts to for this code.
        const a = new Announcement('<b>beta</b>');

        expect(a.visible).toBe(true);
        expect(() => a.dismiss()).not.toThrow();
        // Nothing persisted, so it comes back next load. For an announcement
        // that is the right direction to fail in.
        expect(new Announcement('<b>beta</b>').visible).toBe(true);
    });
});
