// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';

// The menu renders against the focused instance's transform tool: modes come
// from the gizmo, the flips are plain actions on the tool.
const { fakeApp, fakeTool } = vi.hoisted(() => ({
    fakeApp: { transformModeMenu: { x: 10, y: 20 } as { x: number; y: number } | null },
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
