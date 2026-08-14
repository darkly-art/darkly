import { parseMenuSegment, type Action } from '../../actions/registry';

/**
 * A menu is a tree of entries rendered by `MenuItems.svelte`. An entry is one
 * of: an action row, a submenu (with its own entries — rendered as a hover
 * flyout), a named widget slot (e.g. the theme switcher, which isn't an
 * action), or a separator.
 *
 * `label` / `icon` on an action entry override the action's own displayName /
 * (no) icon for that placement — used to surface the command palette as a
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
const MENU_ORDER = ['File', 'Edit', 'Select', 'Image', 'Layer', 'Colors', 'View', 'Help'];

function groupByTop(regs: Action[]): Map<string, Action[]> {
    const m = new Map<string, Action[]>();
    for (const reg of regs) {
        const seg = reg.menuPath?.[0];
        if (!seg) continue;
        const { title } = parseMenuSegment(seg);
        let arr = m.get(title);
        if (!arr) {
            arr = [];
            m.set(title, arr);
        }
        arr.push(reg);
    }
    return m;
}

/** An action's position within its top-level menu, parsed from the order
 *  suffix on `menuPath[0]` (e.g. `'Edit:10'` → 10). Unordered actions sort
 *  to the end. */
function menuOrder(reg: Action): number {
    return parseMenuSegment(reg.menuPath?.[0] ?? '').order ?? Infinity;
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
 * Build the ordered top-level menus from the action registry. Action grouping
 * is FLAT (by `menuPath[0]`'s title); within each menu items sort by the
 * order suffix on `menuPath[0]` (e.g. `'Edit:10'`; lower first, unordered
 * actions fall to the end in registration order). The
 * resulting `entries` are all leaf action rows, except the View menu which
 * also carries the theme switcher widget (the theme control isn't an action).
 * Used directly by the pinned MenuBar and composed into the hamburger's root
 * list.
 */
export function buildTopMenus(regs: Action[]): TopMenu[] {
    const grouped = groupByTop(regs);
    const result: TopMenu[] = [];
    for (const title of orderedTitles(grouped)) {
        const entries: MenuEntry[] = grouped
            .get(title)!
            .slice()
            .sort((a, b) => menuOrder(a) - menuOrder(b))
            .map((r): MenuEntry => ({ kind: 'action', actionId: r.id }));
        if (title === 'View') entries.push({ kind: 'widget', widget: 'theme' });
        result.push({ title, entries });
    }
    return result;
}

/**
 * The hamburger's root entry list: a prominent "Find" (command palette) item
 * up top, the top-level menus as submenu flyouts, then a courtesy block that
 * duplicates the globally-useful commands at the root for one-click access
 * (deliberate duplication — those live in their submenus too). The theme
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
