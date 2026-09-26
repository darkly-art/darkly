import { parseMenuSegment, UNORDERED, type Action } from '../../actions/registry';
import { groupByCategory } from '../../lib/groupByCategory';

/**
 * A menu is a tree of entries rendered by `MenuItems.svelte`. An entry is one
 * of: an action row, a submenu (with its own entries, rendered as a hover
 * flyout), a named widget slot (e.g. the theme switcher, which isn't an
 * action), or a separator.
 *
 * `label` / `icon` on an action entry override the action's own displayName /
 * (no) icon for that placement: used to surface the command palette as a
 * prominent "Find" item without renaming the action everywhere else.
 */
export type MenuEntry =
    | { kind: 'action'; actionId: string; label?: string; icon?: string }
    | { kind: 'submenu'; title: string; entries: MenuEntry[] }
    | { kind: 'widget'; widget: 'theme' }
    | { kind: 'separator' };

/** A top-level menu (File, Edit, …, Help) and its entries. */
export interface TopMenu {
    title: string;
    entries: MenuEntry[];
}

/** Fixed ordering for the known top-level menus. Any group not in this list
 *  (forward-compat for a new `menuPath[0]`) is appended after, in first-seen
 *  order. */
const MENU_ORDER = ['File', 'Edit', 'Select', 'Image', 'Layer', 'Filters', 'View', 'Window', 'Help'];

/** One slot in a menu under construction: an action's own row, or a submenu
 *  collecting the actions that descend into it. `order` is the lowest order
 *  any member declared at this depth, so two members disagreeing about where
 *  their shared submenu sits cannot make it jump. */
type Slot = { order: number } & ({ action: Action } | { title: string; children: Action[] });

/**
 * Entries of one menu, built from the actions that live in or below it: every
 * `reg` shares the first `depth` segments of its `menuPath` and has at least
 * one more. An action whose path ends at `depth` is a row here; one with more
 * segments descends into the submenu named by `menuPath[depth + 1]`, whose own
 * entries are this function one level down.
 *
 * Slots accumulate in first-appearance order across both kinds (a submenu
 * takes the position of its first member) and `Array.prototype.sort` is
 * stable, so equal orders keep registration order with no tiebreak field.
 */
function buildEntries(regs: Action[], depth: number): MenuEntry[] {
    const slots: Slot[] = [];
    const submenus = new Map<string, Extract<Slot, { children: Action[] }>>();
    for (const reg of regs) {
        const path = reg.menuPath!;
        const order = parseMenuSegment(path[depth]).order ?? UNORDERED;
        if (path.length === depth + 1) {
            slots.push({ order, action: reg });
            continue;
        }
        const title = parseMenuSegment(path[depth + 1]).title;
        let slot = submenus.get(title);
        if (!slot) {
            slot = { order, title, children: [] };
            submenus.set(title, slot);
            slots.push(slot);
        }
        slot.order = Math.min(slot.order, order);
        slot.children.push(reg);
    }
    return slots
        .slice()
        .sort((a, b) => a.order - b.order)
        .map((slot): MenuEntry =>
            'action' in slot
                ? { kind: 'action', actionId: slot.action.id }
                : { kind: 'submenu', title: slot.title, entries: buildEntries(slot.children, depth + 1) },
        );
}

function orderedTitles(present: Map<string, unknown>): string[] {
    const placed = new Set<string>();
    const out: string[] = [];
    for (const t of MENU_ORDER) {
        if (present.has(t)) {
            out.push(t);
            placed.add(t);
        }
    }
    for (const t of present.keys()) {
        if (!placed.has(t)) out.push(t);
    }
    return out;
}

/**
 * Build the ordered top-level menus from the action registry. Actions group by
 * `menuPath[0]`'s title into the fixed `MENU_ORDER`; everything below that is
 * `buildEntries`, so a multi-segment path renders as a submenu flyout at any
 * depth. The resulting `entries` are action rows and submenus, except the View
 * menu which also carries the theme switcher widget (the theme control isn't
 * an action). Used directly by the pinned MenuBar and composed into the
 * hamburger's root list.
 */
export function buildTopMenus(regs: Action[]): TopMenu[] {
    const grouped = new Map(
        groupByCategory(
            regs.filter(r => r.menuPath?.[0]),
            r => parseMenuSegment(r.menuPath![0]).title,
            '',
        ).map(g => [g.category, g.items] as const),
    );
    const result: TopMenu[] = [];
    for (const title of orderedTitles(grouped)) {
        const entries = buildEntries(grouped.get(title)!, 0);
        if (title === 'View') entries.push({ kind: 'widget', widget: 'theme' });
        result.push({ title, entries });
    }
    return result;
}

/**
 * The hamburger's root entry list: a prominent "Find" (command palette) item
 * up top, the top-level menus as submenu flyouts, then a courtesy block that
 * duplicates the globally-useful commands at the root for one-click access
 * (deliberate duplication; those live in their submenus too). The theme
 * switcher is intentionally NOT duplicated here; it lives in the View menu.
 */
export function buildHamburgerEntries(regs: Action[]): MenuEntry[] {
    const submenus = buildTopMenus(regs).map(
        (t): MenuEntry => ({ kind: 'submenu', title: t.title, entries: t.entries }),
    );
    return [
        { kind: 'action', actionId: 'commandPalette', label: 'Find', icon: 'fa6-solid:magnifying-glass' },
        { kind: 'separator' },
        ...submenus,
        { kind: 'separator' },
        { kind: 'action', actionId: 'openSettings', icon: 'fa6-solid:gear' },
        { kind: 'action', actionId: 'openCheatsheet' },
        { kind: 'action', actionId: 'aboutDarkly' },
    ];
}
