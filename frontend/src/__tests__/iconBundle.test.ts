import { describe, it, expect, beforeAll } from 'vitest';
// Side effect: registers the generated offline bundle into Iconify's storage.
import '../icons/bundle.generated';
import { generateIcon } from '@iconify/svelte/dist/offline-functions.js';
import { registerActions } from '../actions/index';
import { actions } from '../actions/registry';
import { toolRegistry } from '../tools/registry';

// Register the menu/palette actions. Tools are imported lazily inside the tool
// test instead — registering tool-switch actions needs app methods that aren't
// stood up in the node test env, exactly as in menu_actions.test.ts.
beforeAll(() => {
    registerActions();
});

/** An icon name resolves iff the offline bundle contains it — generateIcon
 *  returns null for an unregistered name. No network, no API. This is the
 *  guard that a referenced icon was actually discovered + bundled. */
function resolves(name: string | undefined): boolean {
    return !!name && generateIcon({ icon: name }) !== null;
}

// All component source, loaded as raw text via Vite (no node:fs — the frontend
// tsconfig is browser-targeted). Keys are paths relative to this file.
const SVELTE_SOURCES = import.meta.glob('../**/*.svelte', {
    query: '?raw',
    eager: true,
    import: 'default',
}) as Record<string, string>;

/** Pull every literal icon name out of `name=`/`icon=` attributes in component
 *  markup — the direct `"fa6-solid:eye"` form and quoted literals inside a
 *  `{cond ? 'a' : 'b'}` expression. Dynamic, registry-driven values (e.g.
 *  `name={entry.icon}`) have no quoted literal and are covered by the registry
 *  tests above; we only collect icon-shaped (`prefix:name`) literals, so a bare
 *  `name="something"` on some other component is ignored. This is what catches a
 *  mistyped *prefix* in markup (which the generator silently skips) at test time
 *  rather than only at dev runtime. */
function markupIconNames(): { file: string; name: string }[] {
    const attr = /[\s](?:name|icon)=("([^"]*)"|'([^']*)'|\{([^}]*)\})/g;
    const literal = /['"]([a-z][a-z0-9-]*:[a-z0-9-]+)['"]/g;
    const iconShape = /^[a-z][a-z0-9-]*:[a-z0-9-]+$/;
    const found: { file: string; name: string }[] = [];
    for (const [rel, text] of Object.entries(SVELTE_SOURCES)) {
        for (const m of text.matchAll(attr)) {
            const direct = m[2] ?? m[3];
            if (direct !== undefined) {
                if (iconShape.test(direct)) found.push({ file: rel, name: direct });
            } else if (m[4] !== undefined) {
                for (const lit of m[4].matchAll(literal)) found.push({ file: rel, name: lit[1] });
            }
        }
    }
    return found;
}

describe('icon bundle completeness (offline)', () => {
    it('bundles every registered action icon', () => {
        const missing = actions
            .all()
            .filter(a => !resolves(a.icon))
            .map(a => `${a.id} -> ${a.icon}`);
        expect(missing).toEqual([]);
    });

    it('bundles every registered tool icon', async () => {
        await import('../tools/index'); // side effect: populates toolRegistry
        const tools = toolRegistry.all();
        expect(tools.length).toBeGreaterThan(0);
        const missing = tools
            .filter(t => t.icon && !resolves(t.icon))
            .map(t => `${t.id} -> ${t.icon}`);
        expect(missing).toEqual([]);
    });

    it('bundles both dynamic brush-tool states', () => {
        // The brush icon getter only returns its current state; assert both the
        // paint and erase glyphs are present so the toolbar never blanks.
        expect(resolves('fa6-solid:paintbrush')).toBe(true);
        expect(resolves('fa6-solid:eraser')).toBe(true);
    });

    it('bundles the custom local:gradient icon', () => {
        expect(resolves('local:gradient')).toBe(true);
    });

    it('bundles Rust-originated icons (crate scan works)', () => {
        // These names live only in crates/darkly/src (brush nodes); they must be
        // picked up by the cross-boundary scan in gen-icon-bundle.mjs.
        for (const name of ['fa6-solid:droplet', 'fa6-solid:feather', 'fa6-solid:wave-square']) {
            expect(resolves(name), name).toBe(true);
        }
    });

    it('bundles every literal icon name hardcoded in component markup', () => {
        const refs = markupIconNames();
        // Sanity: the scanner is actually finding the <Icon> usages.
        expect(refs.length).toBeGreaterThan(20);
        const missing = refs
            .filter(r => !resolves(r.name))
            .map(r => `${r.file} -> ${r.name}`);
        expect(missing).toEqual([]);
    });
});
