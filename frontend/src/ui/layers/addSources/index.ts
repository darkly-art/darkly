import type { AddSource } from './types';

export type { AddSource } from './types';

/**
 * Every add source, discovered from this directory.
 *
 * A glob rather than a hand-written list so a new way of adding something is a
 * new file and nothing else — the frontend's equivalent of `build.rs` scanning
 * a module directory. Rail order comes from each source's action, not from the
 * order they land in here.
 */
const modules = import.meta.glob<{ source: AddSource }>('./*.ts', { eager: true });

export const addSources: AddSource[] = Object.entries(modules)
    .filter(([path]) => !path.endsWith('/index.ts') && !path.endsWith('/types.ts'))
    .map(([, mod]) => mod.source)
    .filter((s): s is AddSource => s != null);
