import { app } from '../../../state/app.svelte';
import type { AddSource } from './types';

/**
 * Both effect categories spawn the same thing: an effect layer in the tree.
 * `category` splits one catalog into two tabs for reading, and the viewport
 * divider — not the category — decides which space a layer renders in, so the
 * two sources differ only in which tab they title.
 */
export function effectLayerSource(action: string, category: string): AddSource {
    return {
        action,
        catalog: 'effects',
        category,
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
}
