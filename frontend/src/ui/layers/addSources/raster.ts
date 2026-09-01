import type { AddSource } from './types';

/**
 * A plain raster layer — the `raster` layer kind, whose `display_name` is
 * "Raster Layer". The tab says "Normal", which is what the add-layer UI has
 * always called it.
 *
 * No `spawn`: `newLayer` already places a raster by the active layer, and
 * routing through the action keeps that one implementation.
 */
export const source: AddSource = {
    action: 'newLayer',
    catalog: '',
    title: 'Normal',
};
