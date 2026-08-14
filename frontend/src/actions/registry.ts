import type { CatalogEntry } from '../engine/protocol_gen';
import { bumpRegistryEpoch } from './registryEpoch.svelte';

export type ActionContext = Record<string, any>;
export type ActionType = 'instant' | 'hold';

/** An action's documentation — the half authored in Rust (`crates/darkly/src/
 *  actions/`) and shipped in the `actions` catalog. */
export interface ActionDoc {
    displayName: string;
    /** Grouping id, e.g. 'edit'. The cheat sheet renders one section per
     *  category and the hotkeys tab groups by it. */
    category: string;
    description?: string;
    /** Base Iconify icon name for this action (e.g. 'fa6-solid:rotate-left'),
     *  rendered in the menu gutter and command-palette row via `<Icon>`. The
     *  dynamic `status()` icon, when active, takes precedence over this base
     *  icon in the gutter. */
    icon: string;
}

/** What a call site hands to `actions.register` — the behavioural half, which
 *  closes over Svelte runes and so cannot leave the browser. */
export interface ActionRegistration {
    id: string;
    /** Documentation for an action whose metadata another catalog owns:
     *  tool selection reads `tools`, filter application reads `filters`, and
     *  each composes a phrasing this side owns ("Switch to Brush tool", the
     *  parametric `…` suffix). Absent for every other action, which resolves
     *  through the `actions` catalog. */
    doc?: ActionDoc;
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
    /** Leading status indicator for menu/palette rows. Returns an Iconify
     *  icon name to display in the row's gutter (e.g. 'fa6-solid:check' for an
     *  active toggle), or undefined for no status. The action owns its own
     *  representation — the renderer just displays whatever name it returns.
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

/** A registration joined with its documentation — what every consumer reads. */
export type Action = Omit<ActionRegistration, 'doc'> & ActionDoc;

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
    action: Action,
): { enabled: boolean; reason?: string } {
    const e = action.enabled?.();
    if (e === undefined || e === true) return { enabled: true };
    if (typeof e === 'string') return { enabled: false, reason: e };
    return { enabled: false };
}

/** Index an `actions` catalog's entries by id, ready for `actions.setDocs`. */
export function actionDocs(entries: CatalogEntry[]): Record<string, ActionDoc> {
    const out: Record<string, ActionDoc> = {};
    for (const e of entries) {
        out[e.type] = {
            displayName: e.displayName,
            category: e.category ?? 'other',
            description: e.description ?? undefined,
            icon: e.icon ?? '',
        };
    }
    return out;
}

class ActionRegistry {
    private actions = new Map<string, ActionRegistration>();

    /** Rust-owned documentation by action id, installed once during editor init
     *  from the `actions` catalog. Fixed for the process — a catalog is
     *  `&'static` data on the other side of the bridge — so the join needs no
     *  reactive tracking. */
    private docs: Record<string, ActionDoc> = {};

    setDocs(docs: Record<string, ActionDoc>) {
        this.docs = docs;
        bumpRegistryEpoch();
    }

    /** Join a registration to its documentation. An id with neither an
     *  `actions` entry nor its own `doc` falls back to showing the id, the same
     *  way `app.displayName` does for an unknown `type_id` — the TypeScript
     *  join test is what fails loudly on it. */
    private resolve(reg: ActionRegistration): Action {
        const { doc, ...behaviour } = reg;
        const resolved = doc ??
            this.docs[reg.id] ?? { displayName: reg.id, category: 'other', icon: '' };
        return { ...behaviour, ...resolved };
    }

    register(reg: ActionRegistration) {
        this.actions.set(reg.id, reg);
        bumpRegistryEpoch();
    }

    get(id: string): Action | undefined {
        const reg = this.actions.get(id);
        return reg && this.resolve(reg);
    }

    /** Run an action's handler. Unknown ids are a no-op — a preset can name
     *  one, and the Rust preset test is what catches it. */
    dispatch(id: string, ctx: ActionContext = {}) {
        this.actions.get(id)?.handler(ctx);
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
    all(): Action[] {
        return [...this.actions.values()].map(reg => this.resolve(reg));
    }

    /** Actions grouped by category (for shortcuts editor UI). */
    byCategory(): Map<string, Action[]> {
        const map = new Map<string, Action[]>();
        for (const action of this.all()) {
            let list = map.get(action.category);
            if (!list) { list = []; map.set(action.category, list); }
            list.push(action);
        }
        return map;
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
