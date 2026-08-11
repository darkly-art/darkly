import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// jsdom isn't available in this project, so we stub the few browser globals the
// save flow touches (`showSaveFilePicker`) and mock the modules that reach for
// the GPU / OPFS (`exportComposite`, `downloadBlob`, recovery). That's enough
// to pin the behaviour that matters — including the regression that Firefox
// (no File System Access API) can now save at all.

vi.mock('../exportComposite', () => ({
    exportComposite: vi.fn(),
    rgbaToBlob: vi.fn(),
}));
vi.mock('../recovery', () => ({
    removeSnapshot: vi.fn(async () => {}),
}));
vi.mock('../index', async (importOriginal) => ({
    ...(await importOriginal<typeof import('../index')>()),
    downloadBlob: vi.fn(),
}));

import {
    formatFromName,
    saveViaDownload,
    saveDocument,
    type Format,
} from '../saveDocument';
import { pickFileHandle, writeToHandle, hasFilePicker, type SaveAccept } from '../fileHandle';
import { exportComposite } from '../exportComposite';
import { downloadBlob } from '../index';
import { removeSnapshot } from '../recovery';
import { saveModal } from '../../state/saveModal.svelte';
import { setActiveInstance } from '../../state/app.svelte';

afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    setActiveInstance(null);
});

describe('formatFromName', () => {
    it('maps known extensions to formats, unknown/missing → darkly', () => {
        expect(formatFromName('a.darkly')).toBe('darkly');
        expect(formatFromName('a.png')).toBe('png');
        expect(formatFromName('photo.JPG')).toBe('jpeg');
        expect(formatFromName('photo.jpeg')).toBe('jpeg');
        expect(formatFromName('a.webp')).toBe('webp');
        // Unknown or extensionless names fall back to a true `.darkly` save.
        expect(formatFromName('a.weird')).toBe('darkly');
        expect(formatFromName('noext')).toBe('darkly');
    });
});

describe('pickFileHandle', () => {
    const accept: SaveAccept = {
        description: 'Darkly Document',
        accept: { 'application/x-darkly': ['.darkly'] },
    };

    it('passes suggestedName / types / id to showSaveFilePicker and returns the handle', async () => {
        const handle = { name: 'doc.darkly' };
        const picker = vi.fn(async () => handle);
        vi.stubGlobal('showSaveFilePicker', picker);

        const result = await pickFileHandle('doc.darkly', [accept], 'darkly-file');

        expect(result).toBe(handle);
        expect(picker).toHaveBeenCalledWith({
            suggestedName: 'doc.darkly',
            types: [accept],
            id: 'darkly-file',
        });
    });

    it('returns null when the user cancels (AbortError)', async () => {
        vi.stubGlobal(
            'showSaveFilePicker',
            vi.fn(async () => {
                throw Object.assign(new Error('cancelled'), { name: 'AbortError' });
            }),
        );
        expect(await pickFileHandle('doc.darkly', [accept], 'darkly-file')).toBeNull();
    });

    it('rethrows non-abort errors', async () => {
        vi.stubGlobal(
            'showSaveFilePicker',
            vi.fn(async () => {
                throw new Error('permission denied');
            }),
        );
        await expect(pickFileHandle('doc.darkly', [accept], 'darkly-file')).rejects.toThrow(
            'permission denied',
        );
    });
});

describe('writeToHandle', () => {
    function fakeHandle() {
        const write = vi.fn(async () => {});
        const close = vi.fn(async () => {});
        return {
            handle: { createWritable: vi.fn(async () => ({ write, close })) } as unknown as FileSystemFileHandle,
            write,
            close,
        };
    }

    it('writes Uint8Array bytes then closes', async () => {
        const { handle, write, close } = fakeHandle();
        const bytes = new Uint8Array([1, 2, 3]);
        await writeToHandle(handle, bytes);
        expect(write).toHaveBeenCalledWith(bytes);
        expect(close).toHaveBeenCalledOnce();
    });

    it('writes a Blob unchanged then closes', async () => {
        const { handle, write, close } = fakeHandle();
        const blob = new Blob([new Uint8Array([9])]);
        await writeToHandle(handle, blob);
        expect(write).toHaveBeenCalledWith(blob);
        expect(close).toHaveBeenCalledOnce();
    });
});

describe('saveViaDownload (Firefox/Safari download path)', () => {
    it('produces bytes, hands a correctly-named Blob to downloadBlob', async () => {
        const fakeBlob = new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' });
        vi.mocked(exportComposite).mockResolvedValue(fakeBlob);

        await saveViaDownload({} as never, 'png', 'My Pic');

        expect(downloadBlob).toHaveBeenCalledTimes(1);
        const [blob, name] = vi.mocked(downloadBlob).mock.calls[0];
        expect(name).toBe('My Pic.png');
        expect(blob).toBe(fakeBlob);
    });

    it('an image export does NOT drop the recovery snapshot (doc stays dirty)', async () => {
        vi.mocked(exportComposite).mockResolvedValue(new Blob([new Uint8Array([1])]));
        await saveViaDownload({} as never, 'webp', 'pic');
        expect(removeSnapshot).not.toHaveBeenCalled();
    });
});

describe('regression: save works without the File System Access API', () => {
    it('hasFilePicker is false in this (Firefox-like) environment', () => {
        // The node test env has no `showSaveFilePicker` — exactly Firefox/Safari.
        expect(hasFilePicker).toBe(false);
    });

    it('Ctrl+S routes to the download Save modal instead of dead-ending', async () => {
        // Pre-fix, `saveDocument` hit `if (!canSave) { toast error; return }` and
        // produced nothing. Now it must open the download-backed Save modal.
        const instance = {
            engine: { api: { documentName: async () => 'MyDoc' } },
            fileHandle: null,
        };
        setActiveInstance(instance as never);
        expect(saveModal.open).toBe(false);

        const pending = saveDocument({ forceAs: false });
        await vi.waitFor(() => expect(saveModal.open).toBe(true));
        expect(saveModal.suggestedName).toBe('MyDoc');

        // Resolve the awaited modal so the flow (and this test) completes.
        saveModal.finish();
        await pending;
        expect(saveModal.open).toBe(false);
    });
});
