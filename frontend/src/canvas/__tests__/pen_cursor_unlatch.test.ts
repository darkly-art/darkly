import { describe, it, expect } from 'vitest';
import canvasViewSource from '../CanvasView.svelte?raw';

// Regression: on Chromium/Linux with a touchscreen pen, when the OS mouse
// cursor is over the canvas at pen-contact, Chromium warps the mouse
// cursor to the pen position and keeps it following the pen for the whole
// stroke. CSS cursor:none on canvas/body/html does not suppress this; nor
// does dropping setPointerCapture, nor `* { cursor: none !important }`,
// nor Pointer Lock (which has its own UX issues). The only thing that
// breaks it is changing which element the cursor's hit-test resolves to:
// briefly setting `canvas.style.pointerEvents = 'none'` for one frame at
// pen pointerdown forces Chromium to re-resolve the cursor against the
// parent (.canvas-container). The captured pen pointer continues to flow
// to canvas via setPointerCapture, so the stroke is unaffected.
//
// This test pins the unlatch step. It fails if a future refactor removes
// the pointer-events toggle, omits the rAF restore, or moves the toggle
// outside the pen branch.

function onPointerDownBody(): string {
    const match = canvasViewSource.match(/function onPointerDown\([\s\S]*?\n {4}\}\n/);
    if (!match) throw new Error('onPointerDown function not found in CanvasView.svelte');
    return match[0];
}

describe('pen cursor unlatch on stroke start', () => {
    it("sets canvas.style.pointerEvents = 'none' in the pen branch", () => {
        const body = onPointerDownBody();
        // The pen branch is `if (e.pointerType === 'pen') { ... }` and must
        // contain the pointer-events toggle.
        const penBranch = body.match(
            /if \(e\.pointerType === 'pen'\) \{[\s\S]*?\n {8}\}/,
        );
        expect(penBranch, "pen branch in onPointerDown").not.toBeNull();
        const pb = penBranch![0];

        expect(pb, "drops canvas out of hit-test").toMatch(
            /canvas\.style\.pointerEvents\s*=\s*['"]none['"]/,
        );
        expect(pb, "restores canvas hit-test on the next frame")
            .toMatch(/requestAnimationFrame\(/);
        expect(pb, "restore sets pointerEvents back to ''")
            .toMatch(/canvas\.style\.pointerEvents\s*=\s*['"]['"]/);
    });

    it('does NOT apply the unlatch to mouse or touch input', () => {
        const body = onPointerDownBody();
        // Only one pointerEvents = 'none' assignment, and it must be inside
        // the pen branch. Mouse and touch don't trigger Chromium's
        // pen-cursor-warp behavior, so they must not be subject to the
        // toggle (which would briefly disable hit-test for non-pen drags
        // and break their cursor / selection feedback).
        const matches = body.match(/canvas\.style\.pointerEvents\s*=\s*['"]none['"]/g);
        expect(matches?.length, "exactly one pointer-events:none assignment")
            .toBe(1);
    });
});
