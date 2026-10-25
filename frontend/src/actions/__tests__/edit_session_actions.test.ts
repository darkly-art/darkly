import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { registerActions } from '../index';
import { actions, actionEnablement } from '../registry';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import type { Engine, EngineState } from '../../engine/protocol';

// Apply and Discard reach the engine on every Enter and Escape, because
// hotkey dispatch does not consult `enabled`. The engine answers whether a
// session was open, and only then does the frontend refresh anything.
beforeAll(() => {
    registerActions();
});

/** `Registry.dispatch` fires the handler without returning its promise, so
 *  reach for the registration to await the async work. */
function run(id: string) {
    return actions.get(id)!.handler({});
}

let inst: DarklyInstance;
let applyEditSession: ReturnType<typeof vi.fn>;
let discardEditSession: ReturnType<typeof vi.fn>;
let layerTree: ReturnType<typeof vi.fn>;

beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', vi.fn());
    applyEditSession = vi.fn();
    discardEditSession = vi.fn();
    layerTree = vi.fn().mockResolvedValue({ layers: [] });
    inst = new DarklyInstance();
    inst.requestFrame = vi.fn();
    inst.engine = {
        api: {
            applyEditSession,
            discardEditSession,
            layerTree,
            setIsolatedNode: vi.fn().mockResolvedValue(null),
        },
    } as unknown as Engine;
    setActiveInstance(inst);
});

describe('edit session actions', () => {
    for (const [id, request] of [
        ['applyEditSession', () => applyEditSession],
        ['discardEditSession', () => discardEditSession],
    ] as const) {
        it(`${id} refreshes when the engine ended a session`, async () => {
            request().mockResolvedValue(true);
            await run(id);
            expect(request()).toHaveBeenCalledOnce();
            expect(layerTree).toHaveBeenCalled();
            expect(inst.requestFrame).toHaveBeenCalled();
        });

        it(`${id} refreshes nothing when no session was open`, async () => {
            request().mockResolvedValue(false);
            await run(id);
            expect(request()).toHaveBeenCalledOnce();
            expect(layerTree).not.toHaveBeenCalled();
            expect(inst.requestFrame).not.toHaveBeenCalled();
        });

        it(`${id} is disabled, with a reason, when no session is open`, () => {
            inst.engineState = { editSession: null } as EngineState;
            expect(actionEnablement(actions.get(id)!)).toEqual({
                enabled: false,
                reason: id === 'applyEditSession' ? 'Nothing to apply' : 'Nothing to discard',
            });
            inst.engineState = {
                editSession: { id: 'strokeEdit', displayName: 'Stroke edit', icon: '' },
            } as EngineState;
            expect(actionEnablement(actions.get(id)!)).toEqual({ enabled: true });
        });
    }
});
