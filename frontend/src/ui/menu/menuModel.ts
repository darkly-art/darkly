import type { ActionRegistration } from '../../actions/registry';

/** One leaf row in a menu. Hotkey, `enabled`, and `checked` are resolved
 *  reactively at render time (the renderer looks the action up by id) rather
 *  than baked in here, so the model stays a pure function of the registry. */
export interface MenuItem {
    actionId: string;
    label: string;
    description?: string;
}

/** A top-level menu (File, Edit, …) and its items. */
export interface MenuGroup {
    title: string;
    items: MenuItem[];
}

/** Fixed ordering for the known top-level menus. Any group not in this list
 *  (forward-compat for a new `menuPath[0]`) is appended after, in first-seen
 *  order. */
const MENU_ORDER = ['File', 'Edit', 'Select', 'Layer', 'Colors', 'View'];

/**
 * Group menu-eligible actions by `menuPath[0]` into ordered top-level menus.
 *
 * FLAT (v1): only the first path segment is used; there is no nesting. Actions
 * without a `menuPath` are excluded (they remain hotkey- and palette-only).
 * Item order within a group follows registration order.
 */
export function buildMenu(regs: ActionRegistration[]): MenuGroup[] {
    const byTitle = new Map<string, MenuItem[]>();
    for (const reg of regs) {
        const title = reg.menuPath?.[0];
        if (!title) continue;
        let items = byTitle.get(title);
        if (!items) {
            items = [];
            byTitle.set(title, items);
        }
        items.push({ actionId: reg.id, label: reg.displayName, description: reg.description });
    }

    const groups: MenuGroup[] = [];
    const placed = new Set<string>();
    for (const title of MENU_ORDER) {
        const items = byTitle.get(title);
        if (items) {
            groups.push({ title, items });
            placed.add(title);
        }
    }
    for (const [title, items] of byTitle) {
        if (!placed.has(title)) groups.push({ title, items });
    }
    return groups;
}
