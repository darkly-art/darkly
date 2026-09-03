// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';

// The menu renders against the focused instance's transform tool: modes come
// from the gizmo, the flips are plain actions on the tool.
const { fakeApp, fakeTool, fakeActions } = vi.hoisted(() => ({
    fakeApp: {
        transformModeMenu: { x: 10, y: 20 } as { x: number; y: number } | null,
        engine: {
            api: { canConvertFloatingToSmartObject: vi.fn(async () => false) },
        } as { api: { canConvertFloatingToSmartObject: () => Promise<boolean> } } | null,
    },
    fakeActions: { dispatch: vi.fn() },
    fakeTool: {
        availableModes: () => [
            { tag: 0, label: 'Free transform' },
            { tag: 1, label: 'Perspective' },
        ],
        activeModeTag: () => 0,
        setMode: vi.fn(),
        flip: vi.fn(),
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../tools/transform.svelte', () => ({ focusedTransformTool: () => fakeTool }));
vi.mock('../../actions/registry', () => ({ actions: fakeActions }));

import TransformModeMenu from '../TransformModeMenu.svelte';

const mounted: Array<Record<string, unknown>> = [];

function render() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(TransformModeMenu, { target }) as Record<string, unknown>);
    return target;
}

afterEach(() => {
    for (const instance of mounted.splice(0)) void unmount(instance);
    document.body.innerHTML = '';
    fakeTool.flip.mockClear();
    fakeActions.dispatch.mockClear();
    fakeApp.transformModeMenu = { x: 10, y: 20 };
});

/** Menu rows render their text in a `.label` span. */
function labels(target: HTMLElement): string[] {
    return Array.from(target.querySelectorAll('button .label')).map((s) => s.textContent ?? '');
}

function click(target: HTMLElement, label: string) {
    const row = Array.from(target.querySelectorAll('button')).find(
        (b) => b.querySelector('.label')?.textContent === label,
    );
    if (!row) throw new Error(`Missing menu item: ${label}`);
    row.click();
}

describe('transform right-click menu', () => {
    it('offers the flips below the modes', () => {
        const target = render();
        expect(labels(target)).toEqual([
            'Free transform',
            'Perspective',
            'Flip Horizontally',
            'Flip Vertically',
        ]);
        expect(target.querySelectorAll('.context-menu-sep').length).toBe(1);
    });

    it('routes each flip to its axis on the tool', () => {
        const target = render();
        click(target, 'Flip Horizontally');
        expect(fakeTool.flip).toHaveBeenCalledWith('h');
        click(target, 'Flip Vertically');
        expect(fakeTool.flip).toHaveBeenLastCalledWith('v');
    });
});

/// Wait for the async convertibility query to settle and Svelte to re-render.
async function settle() {
    await Promise.resolve();
    await Promise.resolve();
    await new Promise((r) => setTimeout(r, 0));
}

describe('convert to smart object entry', () => {
    it('is absent when the engine says the content is not convertible', async () => {
        fakeApp.engine!.api.canConvertFloatingToSmartObject = vi.fn(async () => false);
        const target = render();
        await settle();
        expect(labels(target)).not.toContain('Convert to Smart Object');
    });

    it('appears when the engine says it is, and dispatches rather than opening anything', async () => {
        fakeApp.engine!.api.canConvertFloatingToSmartObject = vi.fn(async () => true);
        const target = render();
        await settle();
        expect(labels(target)).toContain('Convert to Smart Object');

        const row = [...target.querySelectorAll('.label')].find(
            (el) => el.textContent === 'Convert to Smart Object',
        ) as HTMLElement;
        (row.closest('button') ?? row).click();
        expect(fakeActions.dispatch).toHaveBeenCalledWith('convertFloatingToSmartObject');
    });

    it('renders the modes and flips immediately, without waiting on the engine', async () => {
        // The menu must appear on the click. Gating the open on a round trip
        // would make right-click feel broken whenever the engine is busy.
        fakeApp.engine!.api.canConvertFloatingToSmartObject = vi.fn(
            () => new Promise<boolean>(() => {}),
        );
        const target = render();
        expect(labels(target)).toContain('Free transform');
        expect(labels(target)).toContain('Flip Horizontally');
    });
});
