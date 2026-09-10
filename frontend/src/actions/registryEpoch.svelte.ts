/**
 * Reactive epoch for the action registry. The registry itself lives in a plain
 * `.ts` module (16 import sites; no runes), but the data-driven menu needs to
 * recompute once actions finish registering, which happens asynchronously,
 * after the menu components have already mounted. `register()` bumps this
 * epoch; menu builders read `registryEpoch()` inside a `$derived` so they
 * re-run when the registry is populated.
 */
let epoch = $state(0);

export function bumpRegistryEpoch() {
    epoch++;
}

/** Read inside a reactive context to subscribe to registry mutations. */
export function registryEpoch(): number {
    return epoch;
}
