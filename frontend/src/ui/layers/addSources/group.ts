import type { AddSource } from './types';

/**
 * A group. No `spawn`: `newGroup` wraps the selection when there is one and
 * adds an empty group otherwise, and that branch belongs in the action rather
 * than copied here.
 *
 * Shares the `Normal` tab with the raster source — a group is plain document
 * structure rather than a kind of effect, so it belongs beside the plain layer
 * instead of alone in a rail entry of its own. Sources naming the same title
 * merge into one group, in rail order, so this is the whole declaration.
 */
export const source: AddSource = {
    action: 'newGroup',
    catalog: '',
    title: 'Normal',
};
