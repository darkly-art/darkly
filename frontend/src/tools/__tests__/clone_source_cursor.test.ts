import { describe, it, expect, vi } from 'vitest';

// The clone source marker tracks the cursor during a stroke exactly the way
// `clone_source.rs` samples the source: aligned mode slides the marker by the
// same offset the cursor has travelled from the stroke's dest anchor;
// anchored mode pins it at the set source. This mirrors the Rust
// `clone_offset` unit test so the two can't silently drift.

// clone_source_cursor pulls in the app/config singletons at import time;
// stub them so the module graph evaluates under Vitest's node env.
vi.mock('../../state/app.svelte', () => ({
    app: { engine: null, canvasEl: null, requestFrame: vi.fn(), activeToolId: null },
}));
vi.mock('../../config/store.svelte', () => ({
    config: { onChange: vi.fn(() => () => undefined) },
}));

import { trackedSourcePos } from '../clone_source_cursor';

describe('trackedSourcePos', () => {
    const anchor = { x: 100, y: 40 };
    const dest = { x: 10, y: 10 };

    it('aligned: slides the marker by the cursor travel from the dest anchor', () => {
        // At stroke start (cursor == dest) the marker sits on the set source.
        expect(trackedSourcePos(anchor, dest, dest, false)).toEqual(anchor);
        // As the cursor moves, the marker follows by the same delta.
        const cursor = { x: 60, y: 90 };
        expect(trackedSourcePos(anchor, dest, cursor, false)).toEqual({
            x: 100 + (60 - 10),
            y: 40 + (90 - 10),
        });
    });

    it('anchored: pins the marker at the set source regardless of cursor', () => {
        expect(trackedSourcePos(anchor, dest, { x: 999, y: -50 }, true)).toEqual(anchor);
    });

    it('falls back to the anchor when not stroking (no dest / cursor)', () => {
        expect(trackedSourcePos(anchor, null, { x: 5, y: 5 }, false)).toEqual(anchor);
        expect(trackedSourcePos(anchor, dest, null, false)).toEqual(anchor);
    });
});
