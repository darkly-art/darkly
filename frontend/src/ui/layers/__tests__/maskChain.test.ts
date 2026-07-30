import { describe, expect, it, vi } from 'vitest';
import { toggleMaskLink } from '../maskChain';

describe('mask transform chain action', () => {
    it('toggles from the document-projected value', () => {
        const setMaskLinkedToHost = vi.fn();

        expect(toggleMaskLink({ setMaskLinkedToHost }, {
            id: 42,
            linkedToHost: true,
            editable: true,
        })).toBe(true);
        expect(setMaskLinkedToHost).toHaveBeenCalledWith({ id: 42, linked: false });
    });

    it('does not mutate a locked relationship', () => {
        const setMaskLinkedToHost = vi.fn();

        expect(toggleMaskLink({ setMaskLinkedToHost }, {
            id: 42,
            linkedToHost: false,
            editable: false,
        })).toBe(false);
        expect(setMaskLinkedToHost).not.toHaveBeenCalled();
    });
});
