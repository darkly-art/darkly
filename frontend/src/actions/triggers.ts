import { actions } from './registry';

/** Derive a canonical trigger name from a MouseEvent's modifier state.
 *  Format: sorted modifiers joined with '+', then the interaction type.
 *  Examples: "click", "alt+click", "ctrl+shift+doubleClick" */
export function triggerName(e: MouseEvent): string {
    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');

    let interaction: string;
    if (e.button === 1) {
        interaction = 'middleClick';
    } else if (e.detail === 2) {
        interaction = 'doubleClick';
    } else {
        interaction = 'click';
    }

    return mods.length > 0 ? `${mods.join('+')}+${interaction}` : interaction;
}

/** Look up a binding for a site+trigger in config and dispatch if found.
 *  Returns true if a binding existed and was dispatched.
 *
 *  The dispatch call goes through ActionRegistry.dispatch(), which validates
 *  that ctx contains all keys the action requires. So even if a misconfigured
 *  binding slips past config-time validation, the runtime guard catches it. */
export function dispatchBinding(
    site: string,
    e: MouseEvent,
    ctx: Record<string, any>,
    config: { get(key: string): any },
): boolean {
    const trigger = triggerName(e);
    if (trigger === 'click') return false; // plain click = component default
    const actionId = config.get(`bindings.${site}.${trigger}`) as string;
    if (!actionId) return false;
    actions.dispatch(actionId, ctx);
    return true;
}
