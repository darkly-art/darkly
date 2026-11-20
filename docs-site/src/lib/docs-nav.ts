/*
 * The docs sidebar: what it contains, in what order, and with which icon.
 *
 * A section is one object: label, icon, contents. Keeping the icons in a
 * separate map keyed by label, the shape Starlight's sidebar schema forces
 * (`attrs: z.never()` on a group), would let a renamed label silently fall
 * back to a dot.
 *
 * Contents are still *not* enumerated. A section names either one page or one
 * directory, and a directory takes whatever the generator put in it, so a
 * catalog gaining an entry, a new veil or a new brush, shows up here with no
 * change on this side.
 */
import type { CollectionEntry } from 'astro:content';

type Entry = CollectionEntry<'docs'>;

export interface NavLink {
    type: 'link';
    label: string;
    href: string;
    id: string;
    /** Only top-level links carry one; children of a group sit under its icon. */
    icon?: Icon;
}

export interface NavGroup {
    type: 'group';
    label: string;
    icon: Icon;
    entries: NavItem[];
}

export type NavItem = NavLink | NavGroup;

/** A top-level section: an icon, and either one page or one directory. */
interface Section {
    label: string;
    icon: Icon;
    /** Exactly one of these. */
    page?: string;
    directory?: string;
}

export interface Icon {
    box: string;
    body: string;
}

/** Font Awesome 6 solid, the set the editor itself draws from. */
const ICON = {
    book: {
        box: '0 0 448 512',
        body: '<path fill="currentColor" d="M96 0C43 0 0 43 0 96v320c0 53 43 96 96 96h320c17.7 0 32-14.3 32-32s-14.3-32-32-32v-64c17.7 0 32-14.3 32-32V32c0-17.7-14.3-32-32-32zm0 384h256v64H96c-17.7 0-32-14.3-32-32s14.3-32 32-32m32-240c0-8.8 7.2-16 16-16h192c8.8 0 16 7.2 16 16s-7.2 16-16 16H144c-8.8 0-16-7.2-16-16m16 48h192c8.8 0 16 7.2 16 16s-7.2 16-16 16H144c-8.8 0-16-7.2-16-16s7.2-16 16-16"/>',
    },
    keyboard: {
        box: '0 0 576 512',
        body: '<path fill="currentColor" d="M64 64C28.7 64 0 92.7 0 128v256c0 35.3 28.7 64 64 64h448c35.3 0 64-28.7 64-64V128c0-35.3-28.7-64-64-64zm16 64h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16H80c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16M64 240c0-8.8 7.2-16 16-16h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16H80c-8.8 0-16-7.2-16-16zm16 80h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16H80c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16m80-176c0-8.8 7.2-16 16-16h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16zm16 80h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16m-16 112c0-8.8 7.2-16 16-16h224c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16H176c-8.8 0-16-7.2-16-16zm112-208h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16m-16 112c0-8.8 7.2-16 16-16h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16zm112-112h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16m-16 112c0-8.8 7.2-16 16-16h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16zm112-112h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16m-16 112c0-8.8 7.2-16 16-16h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16zm16 80h32c8.8 0 16 7.2 16 16v32c0 8.8-7.2 16-16 16h-32c-8.8 0-16-7.2-16-16v-32c0-8.8 7.2-16 16-16"/>',
    },
    tools: {
        box: '0 0 512 512',
        body: '<path fill="currentColor" d="M78.6 5c-9.5-7.4-23-6.5-31.6 2L7 47c-8.5 8.5-9.4 22-2.1 31.6l80 104c4.5 5.9 11.6 9.4 19 9.4H158l109 109c-14.7 29-10 65.4 14.3 89.6l112 112c12.5 12.5 32.8 12.5 45.3 0l64-64c12.5-12.5 12.5-32.8 0-45.3l-112-112c-24.2-24.2-60.6-29-89.6-14.3L192 158v-54.1c0-7.5-3.5-14.5-9.4-19zM19.9 396.1C7.2 408.8 0 426.1 0 444.1C0 481.6 30.4 512 67.9 512c18 0 35.3-7.2 48-19.9l117.8-117.8c-7.8-20.9-9-43.6-3.6-65.1l-61.7-61.7zM512 144c0-10.5-1.1-20.7-3.2-30.5c-2.4-11.2-16.1-14.1-24.2-6l-63.9 63.9c-3 3-7.1 4.7-11.3 4.7L352 176c-8.8 0-16-7.2-16-16v-57.4c0-4.2 1.7-8.3 4.7-11.3l63.9-63.9c8.1-8.1 5.2-21.8-6-24.2C388.7 1.1 378.5 0 368 0c-79.5 0-144 64.5-144 144v.8l85.3 85.3c36-9.1 75.8.5 104 28.7l15.7 15.7c49-23 83-72.8 83-130.5M56 432a24 24 0 1 1 48 0a24 24 0 1 1-48 0"/>',
    },
    layers: {
        box: '0 0 576 512',
        body: '<path fill="currentColor" d="M264.5 5.2c14.9-6.9 32.1-6.9 47 0l218.6 101c8.5 3.9 13.9 12.4 13.9 21.8s-5.4 17.9-13.9 21.8l-218.6 101c-14.9 6.9-32.1 6.9-47 0l-218.6-101c-8.5-4-13.9-12.5-13.9-21.8s5.4-17.9 13.9-21.8zm212.4 204.4l53.2 24.6c8.5 3.9 13.9 12.4 13.9 21.8s-5.4 17.9-13.9 21.8l-218.6 101c-14.9 6.9-32.1 6.9-47 0l-218.6-101c-8.5-4-13.9-12.5-13.9-21.8s5.4-17.9 13.9-21.8l53.2-24.6l152 70.2a88.1 88.1 0 0 0 73.8 0zm-152 198.2l152-70.2l53.2 24.6c8.5 3.9 13.9 12.4 13.9 21.8s-5.4 17.9-13.9 21.8l-218.6 101c-14.9 6.9-32.1 6.9-47 0l-218.6-101c-8.5-4-13.9-12.5-13.9-21.8s5.4-17.9 13.9-21.8l53.2-24.6l152 70.2a88.1 88.1 0 0 0 73.8 0"/>',
    },
    // The one the brush tool itself registers, `fa6-solid:paintbrush`, so the
    // section is marked with the same glyph the editor puts on the tool.
    paintbrush: {
        box: '0 0 576 512',
        body: '<path fill="currentColor" d="M339.3 367.1c27.3-3.9 51.9-19.4 67.2-42.9L568.2 74.1c12.6-19.5 9.4-45.3-7.6-61.2s-42.9-17.3-61.5-3.3L262.4 187.2c-24 18-38.2 46.1-38.4 76.1zm-19.6 25.4l-116-104.4C143.9 290.3 96 339.6 96 400c0 3.9.2 7.8.6 11.6c1.8 17.5-10.2 36.4-27.8 36.4H64c-17.7 0-32 14.3-32 32s14.3 32 32 32h144c61.9 0 112-50.1 112-112c0-2.5-.1-5-.2-7.5z"/>',
    },
    // The manual's brush page, distinct from the reference section's
    // paintbrush so the two read as different places in the sidebar.
    brush: {
        box: '0 0 384 512',
        body: '<path fill="currentColor" d="M162.4 6c-1.5-3.6-5-6-8.9-6h-19c-3.9 0-7.5 2.4-8.9 6l-20.7 51.7c-3.2 8-14.6 8-17.8 0L66.4 6c-1.5-3.6-5-6-8.9-6H48C21.5 0 0 21.5 0 48v208h384V48c0-26.5-21.5-48-48-48H230.5c-3.9 0-7.5 2.4-8.9 6l-20.7 51.7c-3.2 8-14.6 8-17.8 0zM0 288v32c0 35.3 28.7 64 64 64h64v64c0 35.3 28.7 64 64 64s64-28.7 64-64v-64h64c35.3 0 64-28.7 64-64v-32zm192 144a16 16 0 1 1 0 32a16 16 0 1 1 0-32"/>',
    },
    compass: {
        box: '0 0 512 512',
        body: '<path fill="currentColor" d="M256 512a256 256 0 1 0 0-512a256 256 0 1 0 0 512m117-353.3L317.7 306c-3.6 9.7-11.2 17.3-20.9 20.9l-147.3 55.3c-10 3.7-19.5-6.2-15.8-16.2l55.3-147.3c3.6-9.7 11.2-17.3 20.9-20.9l147.3-55.3c10-3.7 19.5 6.2 15.8 16.2zM256 288a32 32 0 1 0 0-64a32 32 0 1 0 0 64"/>',
    },
} satisfies Record<string, Icon>;

/*
 * The specified layout. Labels are pinned; contents are not.
 *
 * Brushes is its own section and leads the reference: "beginning with paint
 * brush front and center". It was the first page of Tools, which is not the same
 * thing: front and center is not the top row of somebody else's table. Tools
 * keeps the twelve toolbar tools and, holding one page, is pinned as a page
 * rather than a directory; `gen-docs.mjs` writes a single-catalog section flat
 * for exactly this reason, so no group here has one child that repeats its name.
 */
const SECTIONS: Section[] = [
    { label: 'Basics', icon: ICON.book, page: 'index' },
    { label: 'Brush basics', icon: ICON.brush, page: 'brushes' },
    { label: 'Actions & hotkeys', icon: ICON.keyboard, page: 'reference/actions' },
    { label: 'Brushes', icon: ICON.paintbrush, directory: 'reference/brushes' },
    { label: 'Layer types', icon: ICON.layers, directory: 'reference/layer-types' },
    { label: 'Tools', icon: ICON.tools, page: 'reference/tools' },
];

/**
 * Where prose pages the sections don't claim end up.
 *
 * The manual grows without asking. Dropping an unclaimed page would build it
 * as a route nothing links to, reachable only by search, or by knowing the
 * URL. Collecting it here is loud instead: a new
 * guide appears in the sidebar under a heading that says it wasn't placed, and
 * whoever notices can pin it into a section above.
 */
const UNPINNED_LABEL = 'Guides';

const BASE = import.meta.env.BASE_URL;
const href = (id: string) => (id === 'index' ? BASE : `${BASE}${id}/`);

const label = (entry: Entry) => entry.data.title;

/** Reference pages carry an explicit position; prose falls back to its title. */
function byOrderThenTitle(a: Entry, b: Entry) {
    const ao = a.data.order ?? Number.MAX_SAFE_INTEGER;
    const bo = b.data.order ?? Number.MAX_SAFE_INTEGER;
    return ao - bo || label(a).localeCompare(label(b));
}

function toLink(entry: Entry): NavLink {
    return { type: 'link', label: label(entry), href: href(entry.id), id: entry.id };
}

/**
 * The sidebar tree for a set of collection entries.
 *
 * Order is the order of SECTIONS, then whatever `order` the generator wrote
 * inside a directory. A page section whose page does not exist is dropped, so
 * a section can be pinned here before the manual page it names is written.
 */
export function buildNav(entries: Entry[]): NavItem[] {
    const claimed = new Set<string>();
    const items: NavItem[] = [];

    for (const section of SECTIONS) {
        if (section.page) {
            const entry = entries.find((e) => e.id === section.page);
            if (!entry) continue;
            claimed.add(entry.id);
            // A single-page section keeps the section's label and icon, not the
            // page's title: "Actions & hotkeys" is pinned here on purpose.
            items.push({
                type: 'link',
                label: section.label,
                href: href(entry.id),
                id: entry.id,
                icon: section.icon,
            });
            continue;
        }

        const prefix = `${section.directory}/`;
        const children = entries.filter((e) => e.id.startsWith(prefix)).sort(byOrderThenTitle);
        for (const child of children) claimed.add(child.id);
        items.push({
            type: 'group',
            label: section.label,
            icon: section.icon,
            entries: children.map(toLink),
        });
    }

    const unpinned = entries.filter((e) => !claimed.has(e.id)).sort(byOrderThenTitle);
    if (unpinned.length) {
        items.push({
            type: 'group',
            label: UNPINNED_LABEL,
            icon: ICON.compass,
            entries: unpinned.map(toLink),
        });
    }

    return items;
}

/** Every page in sidebar order; what prev/next reads. */
export function flatten(nav: NavItem[]): NavLink[] {
    return nav.flatMap((item) => (item.type === 'link' ? [item] : flatten(item.entries)));
}

/** The pages either side of `id`, or null at the ends. */
export function neighbours(nav: NavItem[], id: string) {
    const flat = flatten(nav);
    const i = flat.findIndex((link) => link.id === id);
    return {
        prev: i > 0 ? flat[i - 1] : null,
        next: i >= 0 && i < flat.length - 1 ? flat[i + 1] : null,
    };
}
