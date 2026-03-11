import { actions, sites, missingContext } from './registry';

export interface BindingConflict {
    key: string;          // The config key or key combo that conflicts
    actions: string[];    // Action IDs bound to the same trigger
}

export interface BindingMismatch {
    configKey: string;    // e.g. "bindings.canvas.alt+click"
    actionId: string;     // e.g. "isolateLayer"
    site: string;         // e.g. "canvas"
    missing: string[];    // e.g. ["layerId"]
}

/** Validate all bindings: check context contracts and detect conflicts.
 *  Call on config load and preset switch. */
export function validateBindings(
    getConfig: (key: string) => any,
): { conflicts: BindingConflict[]; mismatches: BindingMismatch[] } {
    const conflicts = findHotkeyConflicts(getConfig);
    const mismatches = findContextMismatches(getConfig);

    for (const c of conflicts) {
        console.warn(`Hotkey conflict: "${c.key}" bound to [${c.actions.join(', ')}]`);
    }
    for (const m of mismatches) {
        console.warn(
            `Binding mismatch: "${m.configKey}" maps to action "${m.actionId}" ` +
            `which requires [${m.missing.join(', ')}] not provided by site "${m.site}"`
        );
    }

    return { conflicts, mismatches };
}

/** Scan all hotkey bindings for conflicts (two actions sharing a key combo). */
function findHotkeyConflicts(
    getConfig: (key: string) => any,
): BindingConflict[] {
    const keyToActions = new Map<string, string[]>();
    for (const id of actions.ids()) {
        const key = getConfig(`hotkeys.${id}`) as string | undefined;
        if (!key) continue;
        let list = keyToActions.get(key);
        if (!list) { list = []; keyToActions.set(key, list); }
        list.push(id);
    }
    const conflicts: BindingConflict[] = [];
    for (const [key, ids] of keyToActions) {
        if (ids.length > 1) conflicts.push({ key, actions: ids });
    }
    return conflicts;
}

/** Scan all bindings for context contract mismatches.
 *  For keyboard bindings, checks that the keyboard site provides
 *  all keys the bound action requires. */
function findContextMismatches(
    getConfig: (key: string) => any,
): BindingMismatch[] {
    const mismatches: BindingMismatch[] = [];
    const keyboardSite = sites.get('keyboard');

    if (keyboardSite) {
        for (const action of actions.all()) {
            const key = getConfig(`hotkeys.${action.id}`) as string | undefined;
            if (key) {
                const missing = missingContext(action, keyboardSite.provides);
                if (missing.length > 0) {
                    mismatches.push({
                        configKey: `hotkeys.${action.id}`,
                        actionId: action.id,
                        site: 'keyboard',
                        missing,
                    });
                }
            }
        }
    }

    return mismatches;
}
