import { describe, it, expect, vi } from 'vitest';

// A paint target the engine refuses (a smart object, a locked layer) used to
// fail silently: `begin_stroke` was fire-and-forget, so the refusal reached
// `reportEngineError`'s console log and the user saw a brush that did nothing.
// The reason has to reach a toast.

const { show } = vi.hoisted(() => ({ show: vi.fn() }));
vi.mock('../../state/toast.svelte', () => ({ toast: { show } }));

import { beginPaintStroke } from '../paint_stroke';
import { ToolSessionCancelled } from '../tool_session';

function engineWith(result: Promise<null>) {
    return { api: { beginStroke: vi.fn(() => result) } } as never;
}

describe('beginPaintStroke', () => {
    it('is silent when the stroke opens', async () => {
        show.mockClear();
        beginPaintStroke(engineWith(Promise.resolve(null)), 7);
        await Promise.resolve();
        expect(show).not.toHaveBeenCalled();
    });

    it("toasts the engine's reason when the target refuses paint", async () => {
        show.mockClear();
        const refusal = Promise.reject({
            message: '"Smart Object" can\'t be painted on; right-click it and choose Rasterize',
        });
        beginPaintStroke(engineWith(refusal as never), 7);
        await new Promise((r) => setTimeout(r, 0));
        expect(show).toHaveBeenCalledWith('warning', expect.stringContaining('Rasterize'));
    });

    it('stays silent when the tool session died mid-flight', async () => {
        show.mockClear();
        const cancelled = Promise.reject(new ToolSessionCancelled());
        beginPaintStroke(engineWith(cancelled as never), 7);
        await new Promise((r) => setTimeout(r, 0));
        expect(show).not.toHaveBeenCalled();
    });
});
