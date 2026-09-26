/**
 * The desktop host's external-link policy. Without it Electron answers a
 * `target="_blank"` link by opening a second BrowserWindow: the linked site
 * inside a chromeless frame with no address bar, no back button and the app's
 * own preload attached. Every external link in Darkly (the announcement
 * banner's bug-report links, the About modal's Website and GitHub rows) goes
 * through here to reach the reader's real browser instead.
 */
import { describe, it, expect, vi } from 'vitest';
import { externalLinkHandler } from '../external-links';

describe('external link policy', () => {
    it('hands an https link to the system browser and opens no window', () => {
        const openExternal = vi.fn();
        const handler = externalLinkHandler(openExternal);

        const result = handler({ url: 'https://discord.gg/kFz2FGhbpu' });

        expect(openExternal).toHaveBeenCalledWith('https://discord.gg/kFz2FGhbpu');
        // Denying is the half that stops the chromeless second window.
        expect(result).toEqual({ action: 'deny' });
    });

    it('takes plain http too', () => {
        const openExternal = vi.fn();

        externalLinkHandler(openExternal)({ url: 'http://example.com/' });

        expect(openExternal).toHaveBeenCalledWith('http://example.com/');
    });

    it('refuses to hand the shell anything but http', () => {
        // `shell.openExternal` asks the OS to launch whatever the scheme is
        // registered to, so a file:// or custom-scheme URL reaching it is a
        // way to start a local program. Deny the window either way, but only
        // ever launch the browser for web links.
        const openExternal = vi.fn();
        const handler = externalLinkHandler(openExternal);

        for (const url of [
            'file:///etc/passwd',
            'smb://host/share',
            'javascript:alert(1)',
            'darkly://whatever',
        ]) {
            expect(handler({ url })).toEqual({ action: 'deny' });
        }

        expect(openExternal).not.toHaveBeenCalled();
    });

    it('denies a url it cannot even parse rather than passing it on', () => {
        const openExternal = vi.fn();

        expect(externalLinkHandler(openExternal)({ url: 'not a url' })).toEqual({
            action: 'deny',
        });
        expect(openExternal).not.toHaveBeenCalled();
    });
});
