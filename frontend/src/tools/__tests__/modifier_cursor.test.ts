import { describe, it, expect, vi, beforeEach } from 'vitest';

// Feature tests for the shared modifier-cursor engagement machinery: the
// single owner of hover suppression, the `app.toolCursor` slot for
// chord-engaged cursors (last-engaged wins, disengage re-asserts the
// survivor), and the suspend/restore handoff to the active tool.

const { fakeApp, paintTool, sessionEngine } = vi.hoisted(() => {
    const sessionEngine = {
        api: {
            clearOverlay: vi.fn(),
        },
    };
    const paintTool: {
        group: string;
        suspendHover?: ReturnType<typeof vi.fn>;
        restoreHover?: ReturnType<typeof vi.fn>;
    } = {
        group: 'paint',
        suspendHover: vi.fn(),
        restoreHover: vi.fn(),
    };
    const fakeApp = {
        activeToolId: 'brush',
        engine: sessionEngine,
        // The focused instance's tool is looked up via `inst.tool(id)`.
        tool: (_id: string) => paintTool,
        canvasEl: {
            getBoundingClientRect: () => ({ left: 0, top: 0, right: 100, bottom: 100 }),
        } as unknown as HTMLCanvasElement | null,
        toolCursor: null as string | null,
        requestFrame: vi.fn(),
    };
    return { fakeApp, paintTool, sessionEngine };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => fakeApp }));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: (sx: number, sy: number) => ({ x: sx, y: sy }),
}));

// Module state (engagement map, pointer tracking) must not leak between
// tests — re-import a fresh instance each time.
let mc: typeof import('../modifier_cursor');

beforeEach(async () => {
    vi.resetModules();
    vi.clearAllMocks();
    fakeApp.toolCursor = null;
    paintTool.suspendHover = vi.fn();
    paintTool.restoreHover = vi.fn();
    mc = await import('../modifier_cursor');
});

describe('engagement and the cursor slot', () => {
    it('suspends the tool hover on first engage only; last-engaged cursor wins', () => {
        mc.engageModifierCursor('a', 'crosshair');
        expect(paintTool.suspendHover).toHaveBeenCalledTimes(1);
        expect(fakeApp.toolCursor).toBe('crosshair');
        expect(mc.isToolHoverSuppressed()).toBe(true);

        mc.engageModifierCursor('b', 'copy');
        expect(paintTool.suspendHover).toHaveBeenCalledTimes(1); // still once
        expect(fakeApp.toolCursor).toBe('copy');
    });

    it('disengaging a non-final engager re-asserts the survivor, no restore', () => {
        mc.engageModifierCursor('a', 'crosshair');
        mc.engageModifierCursor('b', 'copy');
        mc.disengageModifierCursor('b');
        expect(fakeApp.toolCursor).toBe('crosshair');
        expect(mc.isToolHoverSuppressed()).toBe(true);
        expect(paintTool.restoreHover).not.toHaveBeenCalled();
    });

    it('final disengage releases the cursor and restores hover at the tracked position', () => {
        mc.trackPointer({ clientX: 30, clientY: 40 });
        mc.engageModifierCursor('a', 'crosshair');
        mc.disengageModifierCursor('a');
        expect(fakeApp.toolCursor).toBe(null);
        expect(mc.isToolHoverSuppressed()).toBe(false);
        expect(paintTool.restoreHover).toHaveBeenCalledTimes(1);
        const [cx, cy] = paintTool.restoreHover!.mock.calls[0];
        expect(cx).toBe(30);
        expect(cy).toBe(40);
    });

    it('release: false skips both the cursor release and the restore', () => {
        mc.trackPointer({ clientX: 30, clientY: 40 });
        mc.engageModifierCursor('a', 'crosshair');
        mc.disengageModifierCursor('a', { release: false });
        expect(fakeApp.toolCursor).toBe('crosshair'); // untouched — new owner will set it
        expect(paintTool.restoreHover).not.toHaveBeenCalled();
        expect(mc.isToolHoverSuppressed()).toBe(false);
    });

    it('falls back to clearOverlay when the tool has no suspendHover', () => {
        paintTool.suspendHover = undefined;
        mc.engageModifierCursor('a', 'crosshair');
        expect(sessionEngine.api.clearOverlay).toHaveBeenCalledTimes(1);
    });

    it('does not restore when the pointer is off-canvas', () => {
        mc.trackPointer({ clientX: 500, clientY: 500 }); // outside the 100x100 rect
        mc.engageModifierCursor('a', 'crosshair');
        mc.disengageModifierCursor('a');
        expect(mc.lastCanvasPos()).toBe(null);
        expect(paintTool.restoreHover).not.toHaveBeenCalled();
    });

    it('updateModifierCursor writes the slot only for the current winner', () => {
        mc.engageModifierCursor('a', 'crosshair');
        mc.engageModifierCursor('b', 'copy');
        mc.updateModifierCursor('a', 'grab');
        expect(fakeApp.toolCursor).toBe('copy'); // 'b' still wins
        mc.updateModifierCursor('b', 'help');
        expect(fakeApp.toolCursor).toBe('help');
        mc.disengageModifierCursor('b');
        expect(fakeApp.toolCursor).toBe('grab'); // 'a' re-asserts its updated value
    });
});

describe('pointer tracking', () => {
    it('tracks the pointer-down gate and notifies release subscribers', () => {
        const cb = vi.fn();
        const off = mc.onPointerRelease(cb);
        expect(mc.isPointerDown()).toBe(false);
        mc.notePointerDown();
        expect(mc.isPointerDown()).toBe(true);
        mc.notePointerUp();
        expect(mc.isPointerDown()).toBe(false);
        expect(cb).toHaveBeenCalledTimes(1);
        off();
        mc.notePointerUp();
        expect(cb).toHaveBeenCalledTimes(1);
    });

    it('defers the disengage-restore to pointer release while a pointer is down', () => {
        mc.trackPointer({ clientX: 10, clientY: 10 });
        mc.engageModifierCursor('a', 'crosshair');
        mc.notePointerDown();
        mc.disengageModifierCursor('a');
        expect(fakeApp.toolCursor).toBe(null); // cursor releases immediately
        expect(paintTool.restoreHover).not.toHaveBeenCalled(); // restore waits
        mc.notePointerUp();
        expect(paintTool.restoreHover).toHaveBeenCalledTimes(1);
    });

    it('a re-engagement cancels a pending deferred restore', () => {
        mc.trackPointer({ clientX: 10, clientY: 10 });
        mc.engageModifierCursor('a', 'crosshair');
        mc.notePointerDown();
        mc.disengageModifierCursor('a');
        mc.engageModifierCursor('b', 'copy');
        mc.notePointerUp();
        expect(paintTool.restoreHover).not.toHaveBeenCalled();
        expect(fakeApp.toolCursor).toBe('copy');
    });
});
