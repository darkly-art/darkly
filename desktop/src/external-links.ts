/**
 * Where a link that leaves Darkly goes.
 *
 * Electron's default answer to `target="_blank"` is to open a second
 * BrowserWindow: the linked site in a chromeless frame with no address bar, no
 * back button, and this app's preload attached. For the announcement banner's
 * bug-report links and the About modal's Website and GitHub rows, the right
 * answer is the reader's own browser, so the handler hands the URL to the OS
 * and denies the window.
 *
 * Kept apart from main.ts, which touches Electron's `app` singleton at import
 * time and so cannot be loaded by a test.
 */

/** The shape `webContents.setWindowOpenHandler` passes and expects back. */
type WindowOpenDetails = { url: string };
type WindowOpenResponse = { action: 'deny' };

/** Build the handler, taking the opener as an argument so the policy can be
 *  exercised without an Electron process. In `main.ts` that is
 *  `shell.openExternal`. */
export function externalLinkHandler(
    openExternal: (url: string) => void,
): (details: WindowOpenDetails) => WindowOpenResponse {
    return ({ url }) => {
        // `shell.openExternal` asks the OS to launch whatever the scheme is
        // registered to, which for a file:// or custom-scheme URL means
        // starting a local program. Only the web schemes are ever handed over;
        // everything else is denied without being opened at all.
        let protocol: string;
        try {
            protocol = new URL(url).protocol;
        } catch {
            return { action: 'deny' };
        }

        if (protocol === 'http:' || protocol === 'https:') openExternal(url);
        return { action: 'deny' };
    };
}
