import type { Component } from 'svelte';

/** Extra controls an edit-session kind shows in the canvas frame's tab,
 *  keyed by the kind's id (`EditSessionKind.id`). A kind registers its own
 *  controls here, so the frame stays ignorant of which kinds exist. */
const controls = new Map<string, Component>();

export function registerEditSessionControls(kind: string, component: Component): void {
    controls.set(kind, component);
}

export function editSessionControls(kind: string): Component | undefined {
    return controls.get(kind);
}
