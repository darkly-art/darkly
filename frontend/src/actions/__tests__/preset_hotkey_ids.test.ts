import { describe, it, expect, beforeAll } from 'vitest';
// Node builtins; the project intentionally omits @types/node (see vite.config.ts
// and woff2_decode.test.ts). Vitest runs under node, so these resolve at runtime.
// @ts-ignore
import { readFileSync, readdirSync } from 'node:fs';
// @ts-ignore
import { fileURLToPath } from 'node:url';
// @ts-ignore
import { resolve, dirname } from 'node:path';
import { registerActions } from '../index';
import { actions } from '../registry';

// Regression guard for the "adjustInvert" bug. A preset can bind a chord to
// any action-id string; the hotkey dispatcher only indexes bindings under
// *registered* action ids (config/hotkeys.svelte.ts), so a binding whose id
// matches no action is silently dropped — the key does nothing and the action
// shows an empty hotkey. Krita/Photoshop bound Ctrl+I to `adjustInvert`, but
// the invert filter registers as `filterInvert`, so Ctrl+I was dead.
//
// This test enforces the general invariant: *every* action id referenced by a
// preset must correspond to a real, registerable action — not just invert.

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const PRESETS_DIR = resolve(REPO_ROOT, 'crates/darkly/presets');
const FILTERS_DIR = resolve(REPO_ROOT, 'crates/darkly/src/gpu/filters');
const TOOLS_DIR = resolve(REPO_ROOT, 'frontend/src/tools');
const PRESETS = ['defaults', 'krita', 'photoshop', 'gimp'];

/** Collect the action ids referenced by a preset's `hotkeys:` and
 *  `mouse_clicks:` blocks. Both are a single level of `  id: …` keys (the
 *  value may be an inline chord or a `-` list on following lines — we only
 *  care about the id). We read a block until the next top-level key. */
function presetActionIds(preset: string): string[] {
    const text = readFileSync(resolve(PRESETS_DIR, `${preset}.yaml`), 'utf8');
    const ids: string[] = [];
    let inBlock = false;
    for (const line of text.split('\n')) {
        if (/^\S/.test(line)) inBlock = /^(hotkeys|mouse_clicks):/.test(line.trimEnd());
        if (!inBlock) continue;
        const m = /^ {2}([A-Za-z0-9_]+):/.exec(line);
        if (m) ids.push(m[1]);
    }
    return ids;
}

/** The action id the dynamic filter registration produces for a filter type —
 *  mirrors the `filter${Titlecase(type)}` expression in actions/index.ts. */
const filterActionId = (type: string) =>
    `filter${type.charAt(0).toUpperCase()}${type.slice(1)}`;

/** Filter action ids, sourced from the Rust filter registry (the same source
 *  of truth the presets live beside). These register dynamically at runtime
 *  from `filter_types()`, so they can't come from the live frontend registry
 *  in a headless test — but the invariant they satisfy is identical. */
function dynamicFilterActionIds(): string[] {
    const ids: string[] = [];
    for (const file of readdirSync(FILTERS_DIR)) {
        if (!file.endsWith('.rs') || file === 'mod.rs') continue;
        const src = readFileSync(resolve(FILTERS_DIR, file), 'utf8');
        const m = /type_id:\s*"([a-z_]+)"/.exec(src);
        if (m) ids.push(filterActionId(m[1]));
    }
    return ids;
}

/** Tool-switch action ids, sourced from each tool's `hotkeyAction` literal.
 *  The per-tool loop in actions/index.ts registers exactly these ids, but the
 *  registration touches `app` methods that aren't wired up in a headless test,
 *  so we read the ids from the tool definitions instead. */
function toolActionIds(): string[] {
    const ids: string[] = [];
    for (const file of readdirSync(TOOLS_DIR)) {
        if (!file.endsWith('.ts')) continue;
        const src = readFileSync(resolve(TOOLS_DIR, file), 'utf8');
        for (const m of src.matchAll(/hotkeyAction:\s*'([A-Za-z0-9_]+)'/g)) {
            ids.push(m[1]);
        }
    }
    return ids;
}

describe('preset hotkey ids resolve to real actions', () => {
    let validIds: Set<string>;

    beforeAll(() => {
        registerActions(); // static + brush-param + sample-color + clipboard actions
        validIds = new Set([
            ...actions.all().map(a => a.id),
            ...toolActionIds(),
            ...dynamicFilterActionIds(),
        ]);
    });

    it('every preset binding targets a registerable action id', () => {
        const orphans: string[] = [];
        for (const preset of PRESETS) {
            for (const id of presetActionIds(preset)) {
                if (!validIds.has(id)) orphans.push(`${preset}: ${id}`);
            }
        }
        expect(orphans).toEqual([]);
    });

    // Belt-and-suspenders for the specific bug: assert the invert filter's id
    // is what the presets bind, and that the dead `adjustInvert` id is gone.
    it('binds Ctrl+I to filterInvert in Krita/Photoshop, with no adjustInvert left', () => {
        expect(filterActionId('invert')).toBe('filterInvert');
        expect(validIds.has('filterInvert')).toBe(true);
        for (const preset of ['krita', 'photoshop']) {
            expect(presetActionIds(preset), preset).toContain('filterInvert');
        }
        for (const preset of PRESETS) {
            expect(presetActionIds(preset), preset).not.toContain('adjustInvert');
        }
    });
});
