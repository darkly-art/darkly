// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The row leads with the fg/bg swatches, which read the focused instance's
// colors and the two color prefs through the config store (WASM-backed in
// production).
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: (await import('../../__tests__/fakeConfig.svelte')).fakeConfig,
    tooltipForAction: (label: string) => label,
}));

import { fakeConfig } from '../../__tests__/fakeConfig.svelte';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import Harness from './ToolBarLayoutHarness.test.svelte';

const mounted: Array<Record<string, unknown>> = [];

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    fakeConfig.reset();
});
afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    setActiveInstance(null);
    vi.restoreAllMocks();
});

function render() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(Harness, { target }) as Record<string, unknown>);
    flushSync();
    return target;
}

describe('tool options row', () => {
    // The swatches have to be *inside* the wrapping container, not a sibling
    // of it: a `flex: none` sibling of a wrapping flex container cannot wrap
    // with that container's children, only stand beside them, which is what
    // left the swatch block pinned at the far left while the tool's own
    // controls stacked into extra rows. Asserting on `.center`'s first child
    // says exactly that, and says it at the owner, so it covers every tool
    // options component rather than whichever one a test happened to mount.
    it('leads_the_wrapping_row_with_the_color_swatches', () => {
        const target = render();

        const center = target.querySelector('.center');
        expect(center).not.toBeNull();
        expect(center!.firstElementChild!.classList.contains('color-zone')).toBe(true);
        expect(center!.querySelector('.color-zone .swatches')).not.toBeNull();
    });

    it('lays_the_tools_own_controls_out_after_them_in_the_same_container', () => {
        const target = render();

        // Svelte appends its own scoping class, so match on membership.
        const children = [...target.querySelector('.center')!.children];
        expect(children).toHaveLength(2);
        expect(children[0].classList.contains('color-zone')).toBe(true);
        expect(children[1].classList.contains('tool-control')).toBe(true);
    });
});
