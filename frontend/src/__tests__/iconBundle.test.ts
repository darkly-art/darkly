import { describe, it, expect, beforeAll } from 'vitest';
// Side effect: registers the generated offline bundle into Iconify's storage.
import '../icons/bundle.generated';
import { generateIcon } from '@iconify/svelte/dist/offline-functions.js';
import { registerActions } from '../actions/index';
import { actions } from '../actions/registry';
import { rustActionDocs } from '../actions/__tests__/rust_action_docs';
import { toolRegistry } from '../tools/registry';

// Register the menu/palette actions. Tools are imported lazily inside the tool
// test instead — registering tool-switch actions needs app methods that aren't
// stood up in the node test env, exactly as in menu_actions.test.ts.
beforeAll(() => {
    actions.setDocs(rustActionDocs());
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
    // Action glyphs live in `crates/darkly/src/actions/`, which the generator
    // scans along with the rest of the crate — this is what proves that scan
    // reaches them.
    it('bundles every registered action icon', () => {
        const missing = actions
            .all()
            .filter(a => !resolves(a.icon))
            .map(a => `${a.id} -> ${a.icon}`);
        expect(missing).toEqual([]);
    });

    it('bundles every session-dependent tool icon override', async () => {
        await import('../tools/index'); // side effect: populates toolRegistry
        const tools = toolRegistry.all();
        expect(tools.length).toBeGreaterThan(0);
        // A tool's own glyph lives on its Rust registration, and
        // `gen-icon-bundle.mjs` scans `crates/darkly/src` for those. What is
        // still declared here is the session-dependent override (the brush's
        // erase-mode getter), so that is what this asserts.
        const missing = tools
            .map(t => ({ id: t.id, icon: typeof t.icon === 'function' ? t.icon() : t.icon }))
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

/** Parse the `left top width height` viewBox that the generator baked into an
 *  icon, as Iconify exposes it on the rendered SVG's attributes. */
function viewBox(name: string): [number, number, number, number] {
    const g = generateIcon({ icon: name });
    const vb = (g?.attributes as { viewBox?: string } | undefined)?.viewBox;
    if (!vb) throw new Error(`no viewBox for ${name}`);
    return vb.split(/\s+/).map(Number) as [number, number, number, number];
}

// Every icon renders into the same 1em box (Icon.svelte + `.tool svg`), so its
// on-screen size is how much of its viewBox the artwork fills. gen-icons
// shrink-wraps each viewBox to the inked bounds at build time so all icons —
// regardless of source set's built-in margins — render at a uniform optical
// size. These guard that the tightening actually ran and didn't over-crop.
describe('icon viewBox tightening (offline)', () => {
    it('crops the canonical padded icon to its inked bounds', () => {
        // boxicons:square-dashed ships on a 24×24 grid with the marquee inset
        // ~3 units on every side. Untightened it would read "0 0 24 24"; the
        // baked box must hug the ~18×18 artwork instead.
        const [l, t, w, h] = viewBox('boxicons:square-dashed');
        expect(l).toBeCloseTo(3, 0);
        expect(t).toBeCloseTo(3, 0);
        expect(w).toBeCloseTo(18, 0);
        expect(h).toBeCloseTo(18, 0);
    });

    it('tightens the outline select-tool icons off their full grid', () => {
        // The dashed/lasso marquees are the icons that motivated this: each is
        // drawn inside a padded 24-grid. After tightening every one must be
        // cropped below the full 24-unit canvas, yet not collapsed to nothing.
        for (const name of [
            'boxicons:square-dashed',
            'lucide:circle-dashed',
            'lucide:triangle-dashed',
            'tabler:lasso',
        ]) {
            const [, , w, h] = viewBox(name);
            const span = Math.max(w, h);
            expect(span, `${name} not cropped`).toBeLessThan(24);
            expect(span, `${name} over-cropped`).toBeGreaterThan(10);
        }
    });
});
