import { bumpRegistryEpoch } from './registryEpoch.svelte';

export type ActionContext = Record<string, any>;
export type ActionType = 'instant' | 'hold';

export type ActionCategory =
    | 'edit' | 'tools' | 'selection' | 'brush'
    | 'layers' | 'view' | 'colors' | 'transform' | 'file';

export interface ActionRegistration {
    id: string;
    displayName: string;
    category: ActionCategory;
    description?: string;
    /** Base Font Awesome glyph class for this action (e.g. 'fa-rotate-left'),
     *  rendered in the menu gutter and command-palette row. Bare glyph only —
     *  the renderer supplies the `fa-solid` style prefix (matching `status`
     *  and `MenuEntry.icon`). The dynamic `status()` icon, when active, takes
     *  precedence over this base icon in the gutter. */
    icon: string;
    requires?: string[];
    accepts?: string[];
    type?: ActionType;
    /** Top-level menu this action appears under, e.g. ['Select']. Absent →
     *  not in the click-through menu (still available via hotkey + palette).
     *  v1 is FLAT: only the first segment is used; the renderer is
     *  non-recursive (no submenu flyouts). The array shape is kept for
     *  forward-compat, but multi-segment nesting is deferred.
     *
     *  Each segment may carry a position suffix `'Title:order'` (e.g.
     *  `['Select:10']`): lower `order` sorts first within the menu, and
     *  actions whose segment has no suffix fall to the end in registration
     *  order. Parse with `parseMenuSegment`. */
    menuPath?: string[];
    /** Menu/palette enablement. Absent or `true` → enabled. Return `false` to
     *  disable with no explanation, or a string to disable *and* use that
     *  string as the row's tooltip (the disabled-reason). Resolve via
     *  `actionEnablement` rather than calling this directly. */
    enabled?: () => boolean | string;
    /** Leading status indicator for menu/palette rows. Returns a FontAwesome
     *  icon class to display in the row's gutter (e.g. 'fa-check' for an active
     *  toggle), or undefined for no status. The action owns its own
     *  representation — the renderer just displays whatever class it returns.
     *  An action that defines `status` always reserves gutter space, so the
     *  label doesn't shift when the indicator toggles on/off. */
    status?: () => string | undefined;
    handler: (ctx: ActionContext) => void;
    /** For drag-bound actions: receives the live pointer event plus the
     *  total displacement from the pointerdown position (client pixels)
     *  on each pointermove. Only invoked when the action is triggered via
     *  `dispatchDrag`. */
    onMove?: (ctx: ActionContext, e: PointerEvent, dx: number, dy: number) => void;
    deactivate?: (ctx: ActionContext) => void;
}

export interface BindingSiteRegistration {
    name: string;
    provides: string[];
    /** Human-readable label shown in the cheatsheet scope chip and the
     *  settings UI's site dropdown. Defaults to a title-cased `name`. */
    displayName?: string;
}

/** Parse a `menuPath` segment into its title and optional position. A bare
 *  `'Title'` has no order; `'Title:10'` places the action at order 10 within
 *  its menu (lower sorts first). A non-numeric or absent suffix yields no
 *  order, so the action falls to the end in registration order. */
export function parseMenuSegment(segment: string): { title: string; order?: number } {
    const i = segment.lastIndexOf(':');
    if (i < 0) return { title: segment };
    const order = Number(segment.slice(i + 1));
    if (!Number.isFinite(order)) return { title: segment };
    return { title: segment.slice(0, i), order };
}

/** Resolve an action's menu/palette enablement into a flag plus optional
 *  tooltip reason. `enabled` absent or returning `true` → enabled; a string →
 *  disabled with that string as the reason; `false` → disabled, no reason. */
export function actionEnablement(
    action: ActionRegistration,
): { enabled: boolean; reason?: string } {
    const e = action.enabled?.();
    if (e === undefined || e === true) return { enabled: true };
    if (typeof e === 'string') return { enabled: false, reason: e };
    return { enabled: false };
}

/** Check if an action's hard requirements are satisfied by a set of provided keys. */
export function contextSatisfied(
    action: ActionRegistration,
    provides: string[],
): boolean {
    const req = action.requires;
    if (!req || req.length === 0) return true;
    return req.every(k => provides.includes(k));
}

/** Return the missing required keys, or [] if satisfied. */
export function missingContext(
    action: ActionRegistration,
    provides: string[],
): string[] {
    const req = action.requires;
    if (!req || req.length === 0) return [];
    return req.filter(k => !provides.includes(k));
}

class ActionRegistry {
    private actions = new Map<string, ActionRegistration>();

    register(reg: ActionRegistration) {
        this.actions.set(reg.id, reg);
        bumpRegistryEpoch();
    }

    get(id: string): ActionRegistration | undefined {
        return this.actions.get(id);
    }

    /** Dispatch an action with runtime context validation.
     *  Checks that all required keys are present and non-nullish in ctx. */
    dispatch(id: string, ctx: ActionContext = {}) {
        const action = this.actions.get(id);
        if (!action) return;
        const req = action.requires;
        if (req && req.length > 0) {
            const missing = req.filter(k => ctx[k] == null);
            if (missing.length > 0) {
                console.warn(
                    `Action "${id}" requires [${req.join(', ')}] but context is missing [${missing.join(', ')}]. Skipping.`
                );
                return;
            }
        }
        action.handler(ctx);
    }

    /** For 'hold' actions — called on trigger release. */
    release(id: string, ctx: ActionContext = {}) {
        const action = this.actions.get(id);
        if (action?.type === 'hold') action.deactivate?.(ctx);
    }

    /** All registered action IDs (for hotkey binding enumeration). */
    ids(): string[] {
        return [...this.actions.keys()];
    }

    /** All registrations (for shortcuts editor UI). */
    all(): ActionRegistration[] {
        return [...this.actions.values()];
    }

    /** Actions grouped by category (for shortcuts editor UI). */
    byCategory(): Map<ActionCategory, ActionRegistration[]> {
        const map = new Map<ActionCategory, ActionRegistration[]>();
        for (const reg of this.actions.values()) {
            let list = map.get(reg.category);
            if (!list) { list = []; map.set(reg.category, list); }
            list.push(reg);
        }
        return map;
    }

    /** Actions compatible with a given binding site (for shortcuts editor UI). */
    compatibleWith(site: BindingSiteRegistration): ActionRegistration[] {
        return this.all().filter(a => contextSatisfied(a, site.provides));
    }
}

class BindingSiteRegistry {
    private sites = new Map<string, BindingSiteRegistration>();

    register(reg: BindingSiteRegistration) {
        this.sites.set(reg.name, reg);
    }

    get(name: string): BindingSiteRegistration | undefined {
        return this.sites.get(name);
    }

    all(): BindingSiteRegistration[] {
        return [...this.sites.values()];
    }
}

export const actions = new ActionRegistry();
export const sites = new BindingSiteRegistry();
