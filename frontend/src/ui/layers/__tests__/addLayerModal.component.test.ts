// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

/**
 * The end-to-end form of the user's complaint: one modal, one tab per kind,
 * and an effect registered in two registries offered exactly once.
 */

const spawned: string[] = [];

const catalogs: Record<string, any> = {
    filters: {
        id: 'filters',
        title: 'Filters',
        description: null,
        icon: null,
        order: null,
        entries: [
            entry('black_and_white', 'Black and White'),
            entry('chromatic_aberration', 'Chromatic Aberration', { addable: false }),
            entry('invert', 'Invert'),
        ],
    },
    veils: {
        id: 'veils',
        title: 'Veils',
        description: null,
        icon: null,
        order: null,
        entries: [
            entry('black_and_white', 'Black and White', { addable: false }),
            entry('chromatic_aberration', 'Chromatic Aberration'),
            entry('grain', 'Grain'),
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
        captureKind: null,
        addable: true,
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
        catalog: 'filters',
        spawn: async (e: any) => { spawned.push(`filter:${e.type}`); },
    },
}));
vi.mock('../addSources/veils', () => ({
    source: {
        action: 'newVeil',
        catalog: 'veils',
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

function cardNames(): string[] {
    return Array.from(target.querySelectorAll('.card-name')).map(c => c.textContent!.trim());
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
        expect(tabTitles()).toEqual(['Normal', 'Filters', 'Veils', 'Voids', 'Group']);
    });

    it('offers a doubly-registered effect exactly once across the whole modal', () => {
        render();
        const everywhere: string[] = [];
        for (const title of tabTitles()) {
            clickTab(title);
            everywhere.push(...cardNames());
        }
        expect(everywhere.filter(n => n === 'Chromatic Aberration')).toHaveLength(1);
        expect(everywhere.filter(n => n === 'Black and White')).toHaveLength(1);
    });

    it('files each duplicate under the registration that owns its add path', () => {
        render();
        clickTab('Filters');
        expect(cardNames()).toEqual(['Black and White', 'Invert']);
        clickTab('Veils');
        expect(cardNames()).toEqual(['Chromatic Aberration', 'Grain']);
    });

    it('spawns through the owning source when a card is chosen', async () => {
        render();
        clickTab('Veils');
        (target.querySelectorAll('.card')[0] as HTMLButtonElement).click();
        await Promise.resolve();
        expect(spawned).toEqual(['veil:chromatic_aberration']);
    });

    it('lands on the tab a deep link names', () => {
        addLayerModal.show('Voids');
        render();
        expect(cardNames()).toEqual(['Noise']);
    });

    it('gives a catalog-less kind one card that dispatches its action', async () => {
        const dispatch = vi.spyOn(actions, 'dispatch');
        render();
        clickTab('Group');
        expect(cardNames()).toEqual(['New Group']);
        (target.querySelector('.card') as HTMLButtonElement).click();
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
        expect(cardNames()).toEqual(['Black and White', 'Invert']);
    });
});
