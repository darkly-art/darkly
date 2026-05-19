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
    requires?: string[];
    accepts?: string[];
    type?: ActionType;
    /** Default keyboard trigger(s) in tinykeys notation, optionally prefixed
     *  by a binding site: `"$mod+KeyZ"` (global) or `"layerPanel:Delete"`
     *  (fires only when a `layerPanel` binding site is the active site).
     *  Used when no `hotkeys.<id>` setting is present. Pass an array when
     *  one action wants to be bound to multiple chords. Empty/undefined =
     *  no keyboard trigger by default. */
    defaultHotkey?: string | string[];
    /** Default mouse trigger(s) ("<site>:<chord>", e.g. "layerEye:alt+click").
     *  Used when no `mouseclicks.<id>` setting is present. Pass an array
     *  when one action wants the same chord to fire from multiple sites
     *  (e.g. alt+click on either the layer thumb or the mask thumb both
     *  isolating). User overrides remain a single string and fully replace
     *  the defaults. Empty/undefined = no mouse trigger by default. */
    defaultMouseClick?: string | string[];
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
