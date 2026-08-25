/**
 * Panel-type registry — the type-owned dispatch that keeps the docking system
 * ignorant of *which* panel it's rendering. Each panel registers its component
 * and meta once; consumers (`PanelGroupView`, the pop-out gate) call
 * `resolvePanel(type)` and read meta fields, never `switch`ing on the type.
 *
 * Adding a future top-level panel (overview, history, …) is one `registerPanel`
 * call in `registerPanels.ts` plus one entry in `PanelType` — no consumer edits.
 */

import type { Component } from 'svelte';
import type { PanelType } from './tree';

export interface PanelMeta {
    /** Title shown on the tab. */
    title: string;
    /** The Svelte component rendered in the group body. */
    component: Component<Record<string, never>>;
    /** Whether the tab shows a close affordance and may be removed to nothing.
     *  Layers/Properties are permanent (`false`). */
    closable: boolean;
    /** Whether this panel may be popped out into its own OS window. */
    poppable: boolean;
    /** Whether this panel can be dragged/tiled at all. A non-movable panel (the
     *  canvas) is a fixed *anchor*: it renders with no tab bar, can't be grabbed,
     *  and can't be tabbed into — other panels dock *around* its edges only. */
    movable: boolean;
}

const registry = new Map<PanelType, PanelMeta>();

export function registerPanel(type: PanelType, meta: PanelMeta): void {
    registry.set(type, meta);
}

export function resolvePanel(type: PanelType): PanelMeta {
    const meta = registry.get(type);
    if (!meta) throw new Error(`No panel registered for type "${type}"`);
    return meta;
}

export function isPanelRegistered(type: PanelType): boolean {
    return registry.has(type);
}

