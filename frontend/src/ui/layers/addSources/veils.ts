import { app } from '../../../state/app.svelte';
import type { AddSource } from './types';

/**
 * A veil — a post-process effect over the whole view. Veils live in their own
 * chain rather than the layer tree, which is why this spawns through `addVeil`
 * and selects out of `veilList` rather than by layer id.
 */
export const source: AddSource = {
    action: 'newVeil',
    catalog: 'effects',
    category: 'Veils',
    async spawn(entry) {
        if (!app.engine) return;
        const defaults: Record<string, any> = {};
        for (const p of entry.params) {
            defaults[p.name] = p.default;
        }
        await app.addVeil(entry.type, defaults);
        // Select the newly added veil (added at end of list).
        app.selectVeil(app.veilList.length - 1);
    },
};
