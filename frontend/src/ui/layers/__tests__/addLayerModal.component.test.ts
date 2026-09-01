// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

/**
 * The end-to-end form of the user's complaint: one modal, one tab per kind,
 * and each effect offered exactly once.
 */

const spawned: string[] = [];

const catalogs: Record<string, any> = {
    // One catalog holding both categories — the shape the merged registry
    // emits, sorted by (category, displayName) as it is on the wire.
    effects: {
        id: 'effects',
        title: 'Effects',
        description: null,
        icon: null,
        order: null,
        entries: [
            entry('black_and_white', 'Black and White', { category: 'Filters' }),
            entry('invert', 'Invert', { category: 'Filters' }),
            entry('chromatic_aberration', 'Chromatic Aberration', { category: 'Veils' }),
            entry('grain', 'Grain', { category: 'Veils' }),
        ],
    },
    voids: {
        id: 'voids',
        title: 'Voids',
        description: null,
        icon: null,
        order: null,
        entries: [entry('noise', 'Noise')],
    },
};

function entry(type: string, displayName: string, over: Record<string, unknown> = {}) {
    return {
        type,
        displayName,
        icon: null,
        description: null,
        category: null,
        hotkeyAction: null,
        params: [],
        supportsPreview: false,
        source: null,
        ...over,
    };
}

const app = {
    // The catalog-less sources dispatch their real action handlers, which reach
    // for these.
    engine: { api: { addGroup: vi.fn(async () => 9), addRaster: vi.fn(async () => 9) } },
    catalogs,
    activeLayerId: 3,
    selectedLayerIds: new Set<number>(),
    veilList: [],
    entries: (id: string) => catalogs[id]?.entries ?? [],
    refreshLayerTree: vi.fn(),
    refreshVeilList: vi.fn(),
    requestFrame: vi.fn(),
    selectLayer: vi.fn(),
    selectVeil: vi.fn(),
    addVeil: vi.fn(),
};

vi.mock('../../../state/app.svelte', () => ({ app }));
// The preview canvas needs a GPU round-trip; the cards' names are what matter.
vi.mock('../../EffectPreview.svelte', async () => {
    const stub = (await import('./EffectPreviewStub.svelte')).default;
    return { default: stub };
});
vi.mock('../addSources/filters', () => ({
    source: {
        action: 'newFilterLayer',
        catalog: 'effects',
        category: 'Filters',
        spawn: async (e: any) => { spawned.push(`filter:${e.type}`); },
    },
}));
vi.mock('../addSources/veils', () => ({
    source: {
        action: 'newVeil',
        catalog: 'effects',
        category: 'Veils',
        spawn: async (e: any) => { spawned.push(`veil:${e.type}`); },
    },
}));
vi.mock('../addSources/voids', () => ({
    source: {
        action: 'newVoid',
        catalog: 'voids',
        spawn: async (e: any) => { spawned.push(`void:${e.type}`); },
    },
}));

const { registerActions } = await import('../../../actions');
const { actions, actionDocs } = await import('../../../actions/registry');
const { addLayerModal } = await import('../../../state/addLayerModal.svelte');
const AddLayerModal = (await import('../AddLayerModal.svelte')).default;

beforeAll(() => {
    registerActions();
    // The rail's labels are Rust-owned; without the docs join every action
    // falls back to showing its id, which is not what ships.
    actions.setDocs(actionDocs([
        entry('newLayer', 'New Layer') as any,
        entry('newGroup', 'New Group') as any,
        entry('newFilterLayer', 'New Filter Layer') as any,
        entry('newVeil', 'New Veil') as any,
        entry('newVoid', 'New Void') as any,
    ]));
    // jsdom implements neither; the modal only needs them not to throw.
    HTMLDialogElement.prototype.showModal = function () { this.open = true; };
    HTMLDialogElement.prototype.close = function () { this.open = false; };
});

let target: HTMLElement;
let instance: Record<string, unknown> | undefined;

function render() {
    target = document.createElement('div');
    document.body.append(target);
    instance = mount(AddLayerModal, { target, props: {} });
    flushSync();
}

function tabTitles(): string[] {
    return Array.from(target.querySelectorAll('.tab')).map(t => t.textContent!.trim());
}

/** Every card in the modal, in list order — the groups all render at once. */
function cardNames(): string[] {
    return Array.from(target.querySelectorAll('.card-name')).map(c => c.textContent!.trim());
}

function sectionTitles(): string[] {
    return Array.from(target.querySelectorAll('section h3')).map(h => h.textContent!.trim());
}

/** Cards under one group heading. */
function cardsIn(title: string): string[] {
    const section = Array.from(target.querySelectorAll('section')).find(
        s => s.querySelector('h3')?.textContent?.trim() === title,
    );
    if (!section) throw new Error(`no section ${title}; have ${sectionTitles().join(', ')}`);
    return Array.from(section.querySelectorAll('.card-name')).map(c => c.textContent!.trim());
}

function cardIn(title: string, index: number): HTMLButtonElement {
    const section = Array.from(target.querySelectorAll('section')).find(
        s => s.querySelector('h3')?.textContent?.trim() === title,
    )!;
    return section.querySelectorAll('.card')[index] as HTMLButtonElement;
}

function activeTabTitle(): string | undefined {
    return target.querySelector('.tab.active')?.textContent?.trim();
}

function clickTab(title: string) {
    const tab = Array.from(target.querySelectorAll('.tab')).find(
        t => t.textContent!.trim() === title,
    ) as HTMLButtonElement;
    if (!tab) throw new Error(`no tab ${title}; have ${tabTitles().join(', ')}`);
    tab.click();
    flushSync();
}

beforeEach(() => {
    spawned.length = 0;
    addLayerModal.show();
    vi.clearAllMocks();
});

afterEach(() => {
    if (instance) unmount(instance);
    instance = undefined;
    target?.remove();
    addLayerModal.hide();
});

describe('the add-layer modal', () => {
    it('shows one tab per kind, in Layer-menu order', () => {
        render();
        expect(tabTitles()).toEqual(['Normal', 'Filters', 'Veils', 'Voids']);
    });

    it('renders every group at once, as one scrollable list', () => {
        // The rail jumps within a single list rather than swapping panes, so
        // nothing is hidden behind a tab.
        render();
        expect(sectionTitles()).toEqual(tabTitles());
    });

    it('offers a doubly-registered effect exactly once across the whole modal', () => {
        render();
        const everywhere = cardNames();
        expect(everywhere.filter(n => n === 'Chromatic Aberration')).toHaveLength(1);
        expect(everywhere.filter(n => n === 'Black and White')).toHaveLength(1);
    });

    it('files each duplicate under the registration that owns its add path', () => {
        render();
        expect(cardsIn('Filters')).toEqual(['Black and White', 'Invert']);
        expect(cardsIn('Veils')).toEqual(['Chromatic Aberration', 'Grain']);
    });

    it('spawns through the owning source when a card is chosen', async () => {
        render();
        cardIn('Veils', 0).click();
        await Promise.resolve();
        expect(spawned).toEqual(['veil:chromatic_aberration']);
    });

    it('marks the group a rail click jumped to', () => {
        render();
        clickTab('Voids');
        expect(activeTabTitle()).toBe('Voids');
    });

    it('marks the group a deep link names', () => {
        addLayerModal.show('Voids');
        render();
        expect(activeTabTitle()).toBe('Voids');
        expect(cardsIn('Voids')).toEqual(['Noise']);
    });

    it('puts Group beside New Layer rather than in a rail entry of its own', async () => {
        const dispatch = vi.spyOn(actions, 'dispatch');
        render();
        expect(cardsIn('Normal')).toEqual(['New Layer', 'New Group']);
        expect(tabTitles()).not.toContain('Group');
        cardIn('Normal', 1).click();
        await Promise.resolve();
        expect(dispatch).toHaveBeenCalledWith('newGroup');
    });

    it('searches across tabs', async () => {
        render();
        const input = target.querySelector('input[type=search]') as HTMLInputElement;
        input.value = 'grain';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        flushSync();
        expect(tabTitles()).toEqual(['Veils']);
        expect(cardNames()).toEqual(['Grain']);
    });

    it('puts the search box in the modal header, not the body', () => {
        render();
        const input = target.querySelector('input[type=search]')!;
        expect(input.closest('header')).not.toBeNull();
    });

    it('leaves Left/Right to the caret while the search box has focus', () => {
        render();
        const input = target.querySelector('input[type=search]') as HTMLInputElement;
        input.focus();
        const ev = new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true });
        input.dispatchEvent(ev);
        flushSync();
        expect(ev.defaultPrevented).toBe(false);
    });

    it('still moves the rail with Up/Down from the search box', () => {
        render();
        const input = target.querySelector('input[type=search]') as HTMLInputElement;
        input.focus();
        input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
        flushSync();
        expect(activeTabTitle()).toBe('Filters');
    });
});
