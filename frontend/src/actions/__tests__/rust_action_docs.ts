// Node builtins; the project intentionally omits @types/node (see vite.config.ts
// and woff2_decode.test.ts). Vitest runs under node, so these resolve at runtime.
// @ts-ignore
import { readFileSync, readdirSync } from 'node:fs';
// @ts-ignore
import { fileURLToPath } from 'node:url';
// @ts-ignore
import { resolve, dirname } from 'node:path';
import type { ActionDoc } from '../registry';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const ACTIONS_DIR = resolve(REPO_ROOT, 'crates/darkly/src/actions');

const field = (block: string, name: string): string | undefined =>
    new RegExp(`\\b${name}:\\s*"((?:[^"\\\\]|\\\\.)*)"`).exec(block)?.[1];

/**
 * The `actions` catalog as the Rust tables declare it: every action id mapped
 * to its documentation, keyed exactly as `actions.setDocs` wants it.
 *
 * The catalog normally reaches the editor over the WASM bridge, which a node
 * test has no handle to. Reading the tables directly is possible because every
 * field in them is a plain string literal in a `const ACTIONS: &[ActionDef]`,
 * and it means these tests assert against the same authority the running editor
 * uses rather than a fixture that could drift from it.
 */
export function rustActionDocs(): Record<string, ActionDoc> {
    const out: Record<string, ActionDoc> = {};
    for (const file of readdirSync(ACTIONS_DIR)) {
        if (!file.endsWith('.rs') || file === 'mod.rs') continue;
        const src = readFileSync(resolve(ACTIONS_DIR, file), 'utf8');
        const category = /ActionCategory\s*\{[\s\S]*?\bid:\s*"([^"]+)"/.exec(src)?.[1];
        if (!category) throw new Error(`${file} declares no ActionCategory id`);
        for (const m of src.matchAll(/ActionDef\s*\{([^}]*)\}/g)) {
            const block = m[1];
            const id = field(block, 'id');
            const displayName = field(block, 'display_name');
            if (!id || !displayName) throw new Error(`${file}: malformed ActionDef ${block}`);
            out[id] = {
                displayName,
                category,
                description: field(block, 'description'),
                icon: field(block, 'icon') ?? '',
            };
        }
    }
    return out;
}
