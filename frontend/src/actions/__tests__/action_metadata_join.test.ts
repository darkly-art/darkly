import { describe, it, expect, beforeAll } from 'vitest';
import { registerActions } from '../index';
import { actions } from '../registry';
import { rustActionDocs } from './rust_action_docs';

// An action is two halves joined by id: its documentation is authored in Rust
// (`crates/darkly/src/actions/`) and its handler closes over Svelte runes here.
// Nothing at either end can tell that the other half is missing: a handler with
// no metadata renders as a bare id in the menus, and metadata with no handler is
// a palette row and a hotkey that do nothing. This is the test that notices.
//
// Tool selection and filter application are absent from both sides here: their
// documentation lives in the `tools` / `filters` catalogs (each names the action
// that reaches it in `hotkey_action`), and their handlers register from loops
// that need a live WASM handle. The Rust preset test covers those ids.

describe('Rust action metadata joins to its TypeScript handler', () => {
    let handlers: string[];
    let documented: string[];

    beforeAll(() => {
        registerActions();
        handlers = actions.ids().sort();
        documented = Object.keys(rustActionDocs()).sort();
    });

    it('parses a plausible number of actions out of the Rust tables', () => {
        expect(documented.length).toBeGreaterThan(50);
    });

    it('declares metadata for exactly the actions that have a handler', () => {
        expect(documented).toEqual(handlers);
    });

    it('resolves every registered action to a name, a category and an icon', () => {
        actions.setDocs(rustActionDocs());
        const bare = actions
            .all()
            .filter(a => a.displayName === a.id || !a.category || !a.icon.includes(':'))
            .map(a => a.id);
        expect(bare).toEqual([]);
    });
});
