import { app } from '../../../state/app.svelte';
import type { AddSource } from './types';

/**
 * A non-destructive filter layer. The `effects` catalog is the same list that
 * drives the destructive Colors menu, so a new effect in the Rust core surfaces
 * here with no frontend edit.
 */
export const source: AddSource = {
    action: 'newFilterLayer',
    catalog: 'effects',
    category: 'Filters',
    async spawn(entry) {
        if (!app.engine) return;
        const id = await app.engine.api.addFilter({
            pipeline: entry.type,
            params: {},
            anchor: app.activeLayerId,
        });
        if (id != null) app.selectLayer(id);
        app.requestFrame();
    },
};
