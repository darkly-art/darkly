import { describe, it, expect } from 'vitest';
import { flattenOffer, smartObjectOffer } from '../menu_offers';

// Regression: the Rasterize entry appeared on a smart object's context menu but
// its click handler still early-returned unless the layer had a mask, so the
// entry did nothing: no error, no change, nothing in the console. The entry
// and the handler now read the same answer.

describe('flattenOffer', () => {
    it('offers Rasterize for a layer whose pixels are generated', () => {
        expect(flattenOffer({ paintable: false, hasMask: false })).toBe('Rasterize');
    });

    it('still says Rasterize when that layer also carries a mask', () => {
        expect(flattenOffer({ paintable: false, hasMask: true })).toBe('Rasterize');
    });

    it('offers Flatten for a paintable layer with a mask to bake', () => {
        expect(flattenOffer({ paintable: true, hasMask: true })).toBe('Flatten');
    });

    it('offers nothing for a paintable layer that already is plain pixels', () => {
        expect(flattenOffer({ paintable: true, hasMask: false })).toBeNull();
    });
});

describe('smartObjectOffer', () => {
    it('offers the conversion when the engine says the layer can become one', () => {
        expect(smartObjectOffer({ canBecomeSmartObject: true }, false)).toBe(true);
    });

    it('stays silent when the engine says it cannot', () => {
        expect(smartObjectOffer({ canBecomeSmartObject: false }, false)).toBe(false);
    });

    it('stays silent for a layer that predates the flag rather than guessing', () => {
        expect(smartObjectOffer({}, false)).toBe(false);
    });

    it('stays silent for a multi-row selection, which has no single meaning', () => {
        expect(smartObjectOffer({ canBecomeSmartObject: true }, true)).toBe(false);
    });
});
