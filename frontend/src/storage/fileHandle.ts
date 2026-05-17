/**
 * User-facing file save/open via the File System Access API, with a
 * hidden-input fallback for browsers that don't ship it.
 *
 * Distinct from `./index.ts`'s `DarklyStorage` — that's the *internal*
 * config directory (presets, settings, brushes). This module is the
 * *user-facing* file picker for `.darkly` documents the user opens or
 * saves through Ctrl+S, Ctrl+O, the hamburger menu, etc.
 *
 * Three backends, picked per-call:
 *  - **FS Access API** (Chromium, Safari 16.4+): real handle, persists
 *    for the session — Ctrl+S after a Save As writes back to the same
 *    file with no prompt.
 *  - **Electron**: deferred — when the desktop bundle needs file
 *    save/open, extend `ElectronStorageBridge` with `saveAs/open`
 *    methods returning paths-as-handles and add a branch here.
 *  - **Hidden-input fallback** (Firefox, older browsers): bytes only,
 *    no handle returned, so subsequent Save has nowhere to write.
 *    Phase 5 surfaces that by disabling Save/Save As at the UI level.
 */

/** Whether this browser can persist file save (FS Access API present).
 *  Save / Save As actions are disabled at the UI level when this is
 *  `false` — matches draw.io's posture for Firefox. */
export const canSave: boolean =
    typeof globalThis !== 'undefined' &&
    typeof (globalThis as { showSaveFilePicker?: unknown }).showSaveFilePicker === 'function';

/** File picker filter — `application/x-darkly` is the MIME we register
 *  with the OS so the .darkly extension lights up in the picker's
 *  "Darkly Document" filter. */
const DARKLY_TYPES = [
    {
        description: 'Darkly Document',
        accept: { 'application/x-darkly': ['.darkly'] as readonly string[] },
    },
];

/** Result of a successful open — bytes always; handle only when the
 *  browser supports the FS Access API (so subsequent Ctrl+S can write
 *  back to the same file). */
export interface OpenedFile {
    bytes: Uint8Array;
    name: string;
    handle: FileSystemFileHandle | null;
}

/** Show the Save As picker. Returns the chosen handle, or `null` if
 *  the user cancelled. Throws on permission denial / API errors so the
 *  caller can surface a toast.
 *
 *  Must be called from a user-activation context (click / keydown);
 *  Chrome / Safari throw `SecurityError` otherwise. The Ctrl+S
 *  keydown qualifies.
 */
export async function pickSaveFile(
    suggestedName: string,
): Promise<FileSystemFileHandle | null> {
    if (!canSave) return null;
    try {
        const api = (
            globalThis as {
                showSaveFilePicker: (opts: {
                    suggestedName?: string;
                    types?: typeof DARKLY_TYPES;
                }) => Promise<FileSystemFileHandle>;
            }
        ).showSaveFilePicker;
        return await api({ suggestedName, types: DARKLY_TYPES });
    } catch (e) {
        if ((e as { name?: string })?.name === 'AbortError') return null;
        throw e;
    }
}

/** Write bytes to a previously-acquired handle. The writable is
 *  truncated on open so partial writes can't leave stale tail bytes. */
export async function writeToHandle(
    handle: FileSystemFileHandle,
    bytes: Uint8Array,
): Promise<void> {
    const writable = await handle.createWritable();
    await writable.write(bytes);
    await writable.close();
}

/** Show the Open picker. Tries the FS Access API first so the
 *  returned handle can be cached for subsequent saves; falls back to
 *  a transient hidden `<input type="file">` for browsers without it.
 *  Returns `null` if the user cancelled or no file was chosen.
 *
 *  Must be called from a user-activation context (same as Save). */
export async function pickOpenFile(): Promise<OpenedFile | null> {
    const fsApi = (
        globalThis as {
            showOpenFilePicker?: (opts: {
                types?: typeof DARKLY_TYPES;
                multiple?: boolean;
            }) => Promise<FileSystemFileHandle[]>;
        }
    ).showOpenFilePicker;

    if (typeof fsApi === 'function') {
        try {
            const [handle] = await fsApi({ types: DARKLY_TYPES, multiple: false });
            const file = await handle.getFile();
            const bytes = new Uint8Array(await file.arrayBuffer());
            return { bytes, name: file.name, handle };
        } catch (e) {
            if ((e as { name?: string })?.name === 'AbortError') return null;
            throw e;
        }
    }
    return await pickViaHiddenInput();
}

/** Fallback: build a transient `<input type="file">` on demand, click
 *  it, and resolve with the chosen file. No handle is returned because
 *  Firefox doesn't expose one — Save / Save As stay disabled in that
 *  session (the UI consults `canSave`). */
async function pickViaHiddenInput(): Promise<OpenedFile | null> {
    return await new Promise(resolve => {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = '.darkly,application/x-darkly';
        input.style.position = 'absolute';
        input.style.width = '1px';
        input.style.height = '1px';
        input.style.opacity = '0';
        input.style.pointerEvents = 'none';
        document.body.appendChild(input);

        const cleanup = () => {
            if (input.parentNode) input.parentNode.removeChild(input);
        };
        input.onchange = async () => {
            const file = input.files?.[0];
            cleanup();
            if (!file) {
                resolve(null);
                return;
            }
            const bytes = new Uint8Array(await file.arrayBuffer());
            resolve({ bytes, name: file.name, handle: null });
        };
        // Modern browsers (Chrome 113+, Firefox 91+, Safari 16.4+) fire
        // `cancel` when the picker is dismissed; older ones leak the
        // input until GC. Both paths are exception-flow only — the
        // primary path is FS Access API which has its own cancel
        // semantics.
        input.oncancel = () => {
            cleanup();
            resolve(null);
        };
        input.click();
    });
}
