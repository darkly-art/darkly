import { describe, it, expect, beforeAll, vi } from 'vitest';
import { Catalogs } from '../catalogs.svelte';
import { withApi } from '../../engine/testApi';
import { toolRegistry } from '../../tools/registry';
import { brushSession } from '../../tools/brush.svelte';
import type { Catalog, CatalogEntry } from '../../engine/protocol_gen';

/** A catalog entry with every field the protocol declares, so a test can name
 *  only the two or three it cares about. */
function makeEntry(type: string, over: Partial<CatalogEntry> = {}): CatalogEntry {
    return {
        type,
        displayName: type,
        icon: null,
        description: null,
        category: null,
        hotkeyAction: null,
        params: [],
        supportsPreview: false,
        source: null,
        ...over,
    } as CatalogEntry;
}

function makeCatalog(id: string, entries: CatalogEntry[]): Catalog {
    return { id, title: id, description: null, icon: null, order: null, entries } as Catalog;
}

/** A fake handle whose `catalogs` request answers with `payload`, with a spy on
 *  the transport so a test can count how many requests actually went out. */
function fakeEngine(payload: Catalog[], settle: () => Promise<void> = async () => {}) {
    const send = vi.fn(async (kind: string) => {
        if (kind !== 'catalogs') return null;
        await settle();
        return payload;
    });
    return withApi({ send });
}

describe('catalog lookups', () => {
    it('falls back to the type id when the entry is unknown', async () => {
        const catalogs = new Catalogs();
        await catalogs.load(fakeEngine([makeCatalog('effects', [makeEntry('curves', { displayName: 'Curves' })])]));

        expect(catalogs.displayName('effects', 'curves')).toBe('Curves');
        // Unknown entry, and unknown catalog: both answer with the id rather
        // than an empty label, so a panel never renders a blank.
        expect(catalogs.displayName('effects', 'vhs')).toBe('vhs');
        expect(catalogs.displayName('nosuch', 'vhs')).toBe('vhs');
        expect(catalogs.entries('nosuch')).toEqual([]);
        expect(catalogs.catalog('nosuch')).toBeUndefined();
    });

    it('maps only capture-backed voids into voidCaptureKind', async () => {
        const catalogs = new Catalogs();
        await catalogs.load(
            fakeEngine([
                makeCatalog('voids', [
                    makeEntry('camera', { source: { kind: 'capture', capture: 'camera' } }),
                    makeEntry('screen', { source: { kind: 'capture', capture: 'display' } }),
                    // Procedural and image-sourced voids drive no MediaStream,
                    // so they must not appear: the map is what the reconciler
                    // asks "is this stream-backed?".
                    makeEntry('noise', { source: { kind: 'procedural' } }),
                    makeEntry('plain'),
                ]),
            ]),
        );

        expect([...catalogs.voidCaptureKind.entries()]).toEqual([
            ['camera', 'camera'],
            ['screen', 'display'],
        ]);
    });
});

describe('catalogs load once per process', () => {
    it('a second load does not re-request', async () => {
        const catalogs = new Catalogs();
        const engine = fakeEngine([makeCatalog('tools', [makeEntry('fill')])]);

        await catalogs.load(engine);
        await catalogs.load(engine);

        expect(engine.send).toHaveBeenCalledTimes(1);
    });

    // Regression shape: crash recovery opens one tab per snapshot, each
    // bootstrapping concurrently and each feeding `actions.setDocs` from the
    // `actions` catalog right after awaiting the load. A boolean "already
    // loading" guard would return early to the second caller while the first
    // request was still out, handing it an empty catalog and losing every
    // action's documentation for the whole session.
    it('concurrent loads share one in-flight request and both see the result', async () => {
        const catalogs = new Catalogs();
        let release: () => void = () => {};
        const inFlight = new Promise<void>((r) => (release = r));
        const engine = fakeEngine([makeCatalog('actions', [makeEntry('copy')])], () => inFlight);

        const first = catalogs.load(engine);
        const second = catalogs.load(engine);
        // The second caller must not be allowed to proceed on an empty catalog
        // while the first request is still out.
        expect(catalogs.entries('actions')).toEqual([]);

        release();
        await Promise.all([first, second]);

        expect(engine.send).toHaveBeenCalledTimes(1);
        expect(catalogs.entries('actions').map((e) => e.type)).toEqual(['copy']);
    });
});

// A tool's glyph is registry metadata and arrives in the `tools` catalog from
// Rust. A descriptor may override it only when the glyph tracks live session
// state: the brush swaps to the eraser icon while erase mode is on, which a
// static registration cannot express. `toolGlyph` is the single place that
// precedence is decided, so this pins both directions of it.
describe('toolGlyph', () => {
    beforeAll(async () => {
        await import('../../tools/index'); // side effect: populates toolRegistry
    });

    async function withTools(entries: Array<[string, string | null]>) {
        const catalogs = new Catalogs();
        await catalogs.load(
            fakeEngine([makeCatalog('tools', entries.map(([type, icon]) => makeEntry(type, { icon })))]),
        );
        return catalogs;
    }

    it('uses the registry icon for a tool that declares no override', async () => {
        const catalogs = await withTools([['fill', 'fa6-solid:fill-drip']]);
        // The fill descriptor carries no `icon`; its glyph is Rust's.
        expect(toolRegistry.get('fill')?.icon).toBeUndefined();
        expect(catalogs.toolGlyph('fill')).toBe('fa6-solid:fill-drip');
    });

    it("prefers the brush's session-dependent override over the registry icon", async () => {
        const catalogs = await withTools([['brush', 'fa6-solid:paintbrush']]);

        const wasErasing = brushSession.eraseMode;
        try {
            brushSession.eraseMode = false;
            expect(catalogs.toolGlyph('brush')).toBe('fa6-solid:paintbrush');

            // The override is what makes the toolbar button a mode indicator;
            // the registry icon must not win here.
            brushSession.eraseMode = true;
            expect(catalogs.toolGlyph('brush')).toBe('fa6-solid:eraser');
        } finally {
            brushSession.eraseMode = wasErasing;
        }
    });

    it('falls back to a generic glyph before the catalog has loaded', () => {
        expect(new Catalogs().toolGlyph('fill')).toBe('fa6-solid:wrench');
    });
});
