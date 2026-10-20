// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { EditSessionKind } from '../../engine/protocol_gen';

vi.mock('../../config/store.svelte', () => ({
    tooltipForAction: (label: string) => label,
}));

import EditSessionFrame from '../EditSessionFrame.svelte';
import { registerEditSessionControls } from '../edit_session_controls';
import ControlStub from './EditSessionControlStub.svelte';

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

function render(session: EditSessionKind | null) {
    const host = document.createElement('div');
    document.body.append(host);
    const onapply = vi.fn();
    const ondiscard = vi.fn();
    mounted.push(
        mount(EditSessionFrame, {
            target: host,
            props: { session, onapply, ondiscard },
        }) as Record<string, unknown>,
    );
    flushSync();
    const button = (label: string) =>
        host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
    return { host, onapply, ondiscard, button };
}

const strokeEdit: EditSessionKind = {
    id: 'strokeEdit',
    displayName: 'Stroke edit',
    icon: 'fa6-solid:pen-nib',
};

describe('EditSessionFrame', () => {
    it('draws nothing with no session open', () => {
        const { host } = render(null);
        expect(host.querySelector('.edit-session-frame')).toBeNull();
    });

    it('names the session and offers Discard and Apply', () => {
        const { host, button } = render(strokeEdit);
        expect(host.querySelector('.edit-session-frame')).not.toBeNull();
        expect(host.textContent).toContain('Stroke edit');
        expect(button('Discard')).not.toBeNull();
        expect(button('Apply')).not.toBeNull();
    });

    it('routes each button to its callback once', () => {
        const { button, onapply, ondiscard } = render(strokeEdit);
        button('Discard')!.click();
        expect(ondiscard).toHaveBeenCalledOnce();
        expect(onapply).not.toHaveBeenCalled();
        button('Apply')!.click();
        expect(onapply).toHaveBeenCalledOnce();
        expect(ondiscard).toHaveBeenCalledOnce();
    });

    it('draws a kind it has never heard of from the kind alone', () => {
        const { host } = render({ id: 'futureMode', displayName: 'Future', icon: 'fa6-solid:check' });
        expect(host.textContent).toContain('Future');
    });

    it("shows a kind's registered controls, and only for that kind", () => {
        registerEditSessionControls('futureMode', ControlStub);
        const future = render({ id: 'futureMode', displayName: 'Future', icon: 'fa6-solid:check' });
        expect(future.host.querySelector('.tab .control-stub')).not.toBeNull();
        const other = render(strokeEdit);
        expect(other.host.querySelector('.control-stub')).toBeNull();
    });
});
