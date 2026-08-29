import type { AddSource } from './types';

/**
 * A group. No `spawn`: `newGroup` wraps the selection when there is one and
 * adds an empty group otherwise, and that branch belongs in the action rather
 * than copied here.
 *
 * `title` because the rail names a kind ("Group") while the action names a
 * command ("New Group"); a catalog-bearing source gets that from its catalog's
 * own title, and a catalog-less one has to say it.
 */
export const source: AddSource = {
    action: 'newGroup',
    catalog: '',
    title: 'Group',
};
