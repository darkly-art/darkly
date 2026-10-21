import { describe, it, expect } from 'vitest';
import { reorderAnchor } from '../reorderAnchor';

describe('reorderAnchor', () => {
    it('anchors a middle item on the item after it', () => {
        expect(reorderAnchor(['a', 'c', 'b', 'd'], 'c')).toBe('b');
    });

    it('anchors an item dropped at the top on the former first item', () => {
        expect(reorderAnchor(['d', 'a', 'b', 'c'], 'd')).toBe('a');
    });

    it('sends null for an item dropped at the end', () => {
        expect(reorderAnchor(['b', 'c', 'a'], 'a')).toBeNull();
    });

    it('rejects a key that is not in the order', () => {
        expect(() => reorderAnchor(['a', 'b'], 'z')).toThrow();
    });
});
