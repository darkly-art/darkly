import { describe, it, expect } from 'vitest';
import type { BrushInfo } from '../../../engine/protocol_gen';
import { updateTarget } from '../saveTarget';

function brush(id: string, name: string, can_edit: boolean): BrushInfo {
    return { id, name, author: '', description: '', tags: [], icon: null, can_edit };
}

const LIBRARY = [
    brush('ink_pen', 'Ink Pen', false),
    brush('brush-abc', 'My Brush', true),
];

describe('updateTarget', () => {
    it('offers to update a brush the painter owns', () => {
        expect(updateTarget('My Brush', LIBRARY)?.id).toBe('brush-abc');
    });

    it('refuses to update a shipped brush', () => {
        // Saving over one would shadow it until the next boot rebuilt it from
        // YAML, so a modified builtin is saved as new.
        expect(updateTarget('Ink Pen', LIBRARY)).toBeNull();
    });

    it('has nothing to update when no brush is loaded', () => {
        expect(updateTarget(null, LIBRARY)).toBeNull();
    });

    it('has nothing to update when the name no longer resolves', () => {
        expect(updateTarget('Deleted Brush', LIBRARY)).toBeNull();
    });

    it('decides on can_edit, never on the id', () => {
        // The gate is the engine's answer, not a guess from what the id looks
        // like: a painter-owned brush may carry any id at all.
        expect(updateTarget('Ink Pen', [brush('ink_pen', 'Ink Pen', true)])?.id).toBe('ink_pen');
    });
});
