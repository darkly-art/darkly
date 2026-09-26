// @vitest-environment jsdom
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The version is the engine's, read through the bridge: there is no second
// derivation on the frontend to agree with it.
const { version } = vi.hoisted(() => ({ version: vi.fn(() => 'v1.2.3-0-gabc1234') }));
vi.mock('../../../wasm/pkg/darkly_wasm', () => ({ version }));

import { about } from '../../state/about.svelte';
import AboutModal from '../AboutModal.svelte';

beforeAll(() => {
    HTMLDialogElement.prototype.showModal = function () { this.open = true; };
    HTMLDialogElement.prototype.close = function () { this.open = false; };
});

let component: Record<string, unknown> | undefined;

afterEach(() => {
    if (component) void unmount(component);
    component = undefined;
    about.open = false;
    document.body.innerHTML = '';
    version.mockClear();
});

function render(): HTMLElement {
    const target = document.createElement('div');
    document.body.append(target);
    component = mount(AboutModal, { target }) as Record<string, unknown>;
    flushSync();
    return target;
}

describe('AboutModal', () => {
    it('shows the version the engine reports', () => {
        about.open = true;
        const target = render();
        expect(target.querySelector('.version-copy code')?.textContent).toBe('v1.2.3-0-gabc1234');
    });

    it('does not read the bridge while closed', () => {
        // App mounts the modal before the bridge is initialized, when calling
        // into it would throw.
        render();
        expect(version).not.toHaveBeenCalled();
        about.open = true;
        flushSync();
        expect(version).toHaveBeenCalled();
    });
});
