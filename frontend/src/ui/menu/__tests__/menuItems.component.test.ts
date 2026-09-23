// @vitest-environment jsdom
//
// A submenu entry renders as a hover flyout, and a flyout nested inside a
// flyout is reachable. `tsc` and `svelte-check` prove the recursive render
// typechecks against the menu model; only mounting it proves it draws.
import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { actions } from '../../../actions/registry';
import MenuItems from '../MenuItems.svelte';
import type { MenuEntry } from '../menuModel';

const mounted: Array<Record<string, unknown>> = [];

beforeAll(() => {
    // Rows resolve their label and icon through the registry, so the two
    // actions the fixtures name have to exist in it.
    actions.register({
        id: 'blackAndWhite',
        doc: { displayName: 'Black and White', category: 'layers', description: '', icon: 'tabler:circle-half' },
        handler: () => {},
    });
    actions.register({
        id: 'grain',
        doc: { displayName: 'Grain', category: 'layers', description: '', icon: 'tabler:grain' },
        handler: () => {},
    });
});

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
});

function render(entries: MenuEntry[]): HTMLElement {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(MenuItems, { target, props: { entries } }) as Record<string, unknown>);
    flushSync();
    return target;
}

/** Open a submenu the way a pointer does, and hand back its flyout. */
function openSubmenu(scope: ParentNode, title: string): HTMLElement {
    const row = Array.from(scope.querySelectorAll('.submenu-row')).find(
        candidate => candidate.querySelector('.label')?.textContent === title,
    );
    expect(row, `a submenu row titled ${title}`).toBeTruthy();
    row!.dispatchEvent(new MouseEvent('mouseenter', { bubbles: false }));
    flushSync();
    const flyout = row!.querySelector('.flyout');
    expect(flyout, `${title} opens a flyout on hover`).toBeTruthy();
    return flyout as HTMLElement;
}

const VEILS: MenuEntry[] = [
    { kind: 'action', actionId: 'blackAndWhite' },
    { kind: 'submenu', title: 'Veils', entries: [{ kind: 'action', actionId: 'grain' }] },
];

describe('MenuItems submenus', () => {
    it('opens a submenu on hover and renders its rows', () => {
        const target = render(VEILS);
        const flyout = openSubmenu(target, 'Veils');
        expect(flyout.querySelector('.label')?.textContent).toBe('Grain');
    });

    it('renders a flyout nested inside a flyout', () => {
        // The hamburger's shape: a top menu is itself a submenu, so Veils is
        // a second level of flyout. Nothing rendered this before.
        //
        // Whether the outer flyout *clips* the inner one is a layout question
        // jsdom cannot answer (it has no layout engine), which is why
        // `.flyout` keeps its overflow at `visible` and says why in the CSS
        // rather than being defended here.
        const target = render([{ kind: 'submenu', title: 'Filters', entries: VEILS }]);
        const filters = openSubmenu(target, 'Filters');
        const veils = openSubmenu(filters, 'Veils');
        expect(veils.querySelector('.label')?.textContent).toBe('Grain');
    });

    it('gives a submenu row the same leading gutter an action row has', () => {
        // Without it, a submenu's label starts where its icon-bearing
        // siblings' icons do, and the menu reads as ragged.
        const target = render(VEILS);
        const row = target.querySelector('.submenu-row')!;
        expect(row.querySelector('.icon')).toBeTruthy();
        expect(row.firstElementChild).toBe(row.querySelector('.icon'));
    });
});
