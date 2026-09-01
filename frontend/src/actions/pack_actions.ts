/**
 * Importing and exporting brush packs.
 *
 * Without pack-management UI these two actions are how a pack is reached at
 * all. They ship together deliberately: import with no export is a one-way
 * door, and the asymmetry would read as a half-built feature.
 *
 * A `.darkly-brush` is a zip, so it would be indistinguishable from a `.darkly`
 * document to `detectKind`, which is magic-byte-only by design. It never has
 * to be: the unified Open flow only sees what its picker accepts, and
 * `.darkly-brush` is in neither `OPEN_TYPES` nor `OPEN_ACCEPT`. Pack import has
 * its own affordance with its own `accept`, so the two flows never meet.
 */
import { actions } from './registry';
import { app } from '../state/app.svelte';
import { toast } from '../state/toast.svelte';
import { brushLibrary } from '../state/brush_library.svelte';
import { packExport } from '../state/packExport.svelte';
import { downloadBlob, sanitizeFilename } from '../storage';
import { newId } from '../lib/id';

export const PACK_EXTENSION = '.darkly-brush';

/** Prompt for a `.darkly-brush` file and import it as a new pack. */
export async function importPackFromFile(file: File): Promise<void> {
    if (!app.engine) return;
    const bytes = new Uint8Array(await file.arrayBuffer());
    const id = newId('pack');
    try {
        await app.engine.api.packImport({ id }, bytes);
    } catch (e) {
        toast.show('error', `Could not import brush pack: ${e instanceof Error ? e.message : e}`);
        return;
    }
    await brushLibrary.refresh();
    // Persist the imported pack and every brush that arrived with it, so the
    // import survives a reload.
    await brushLibrary.persistImported(id);
    const pack = brushLibrary.pack(id);
    toast.show('success', `Imported brush pack “${pack?.name ?? 'Untitled'}”.`);
}

/** Write a pack out as a `.darkly-brush` file. */
export async function exportPack(id: string): Promise<void> {
    if (!app.engine) return;
    const pack = brushLibrary.pack(id);
    try {
        const { bytes } = await app.engine.api.packExport({ id });
        const blob = new Blob([bytes as Uint8Array<ArrayBuffer>], {
            type: 'application/zip',
        });
        downloadBlob(blob, `${sanitizeFilename(pack?.name ?? 'brush-pack')}${PACK_EXTENSION}`);
    } catch (e) {
        toast.show('error', `Could not export brush pack: ${e instanceof Error ? e.message : e}`);
    }
}

/** Open a one-shot file input for a pack. The input is never mounted: the
 *  same shape the font browser's upload affordance uses, minus the markup. */
function promptForPack() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = PACK_EXTENSION;
    input.onchange = () => {
        const file = input.files?.[0];
        if (file) void importPackFromFile(file);
    };
    input.click();
}

export function registerPackActions() {
    actions.register({
        id: 'importBrushPack',
        menuPath: ['File:60'],
        handler: promptForPack,
    });
    actions.register({
        id: 'exportBrushPack',
        menuPath: ['File:61'],
        handler: () => {
            packExport.open = true;
        },
    });
}
