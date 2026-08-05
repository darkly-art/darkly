import { describe, it, expect, beforeAll } from 'vitest';
import { DarklyInstance } from '../app.svelte';
import { toolRegistry } from '../../tools/registry';
import { brushSession } from '../../tools/brush.svelte';

// A tool's glyph is registry metadata and arrives in the `tools` catalog from
// Rust. A descriptor may override it only when the glyph tracks live session
// state — the brush swaps to the eraser icon while erase mode is on, which a
// static registration cannot express. `toolGlyph` is the single place that
// precedence is decided, so this pins both directions of it.

/** Stand-in for the catalog the engine would deliver at startup. */
function withToolsCatalog(inst: DarklyInstance, entries: Array<[string, string | null]>) {
    inst.catalogs = {
        tools: {
            id: 'tools',
            title: 'Tools',
            description: null,
            icon: null,
            order: null,
            entries: entries.map(([type, icon]) => ({
                type,
                displayName: type,
                icon,
                description: null,
                category: null,
                hotkeyAction: null,
                params: [],
                supportsPreview: null,
                captureKind: null,
            })),
        },
    } as never;
}

describe('toolGlyph', () => {
    beforeAll(async () => {
        await import('../../tools/index'); // side effect: populates toolRegistry
    });

    it('uses the registry icon for a tool that declares no override', () => {
        const inst = new DarklyInstance();
        withToolsCatalog(inst, [['fill', 'fa6-solid:fill-drip']]);
        // The fill descriptor carries no `icon` — its glyph is Rust's.
        expect(toolRegistry.get('fill')?.icon).toBeUndefined();
        expect(inst.toolGlyph('fill')).toBe('fa6-solid:fill-drip');
    });

    it("prefers the brush's session-dependent override over the registry icon", () => {
        const inst = new DarklyInstance();
        withToolsCatalog(inst, [['brush', 'fa6-solid:paintbrush']]);

        const wasErasing = brushSession.eraseMode;
        try {
            brushSession.eraseMode = false;
            expect(inst.toolGlyph('brush')).toBe('fa6-solid:paintbrush');

            // The override is what makes the toolbar button a mode indicator;
            // the registry icon must not win here.
            brushSession.eraseMode = true;
            expect(inst.toolGlyph('brush')).toBe('fa6-solid:eraser');
        } finally {
            brushSession.eraseMode = wasErasing;
        }
    });

    it('falls back to a generic glyph before the catalog has loaded', () => {
        const inst = new DarklyInstance();
        expect(inst.toolGlyph('fill')).toBe('fa6-solid:wrench');
    });
});
