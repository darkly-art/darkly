import { describe, it, expect } from 'vitest';
import {
    NEUTRAL_PALETTE,
    PALETTE_CLASS,
    PALETTE_ROLES,
    packPalette,
    type PackPalette,
} from '../packPalette';

/** A node the action can dress. Vitest runs in the node environment with no
 *  DOM, so this is a plain fake with the two surfaces the action touches — the
 *  same technique `clickOutside.test.ts` uses for `window`. */
function fakeNode() {
    const classes = new Set<string>();
    const props = new Map<string, string>();
    return {
        classes,
        props,
        node: {
            classList: { add: (c: string) => void classes.add(c) },
            style: { setProperty: (k: string, v: string) => void props.set(k, v) },
        } as unknown as HTMLElement,
    };
}

const PACK: PackPalette = {
    chroma: '#2f7fe0',
    refraction: '#2fd0c0',
    surface: '#0c1a26',
};

describe('the pack palette action', () => {
    it('sets every role and the class that derives from them', () => {
        // The coupling is the whole point of this being one action: the derived
        // tokens are substituted on the element declaring the absolutes, so
        // properties without the class — or the class without the properties —
        // silently renders every pack in the neutral palette.
        const { classes, props, node } = fakeNode();
        packPalette(node, PACK);

        expect(classes.has(PALETTE_CLASS)).toBe(true);
        expect(Object.fromEntries(props)).toEqual({
            '--pack-chroma': '#2f7fe0',
            '--pack-refraction': '#2fd0c0',
            '--pack-surface': '#0c1a26',
        });
    });

    it('re-applies a changed palette', () => {
        const { props, node } = fakeNode();
        const { update } = packPalette(node, PACK);
        update({ ...PACK, chroma: '#4f9e46' });
        expect(props.get('--pack-chroma')).toBe('#4f9e46');
    });

    it('passes a translucent surface through unchanged', () => {
        // Alpha is how a pack lets the background it sits on show through, so
        // the eight-digit form has to reach CSS intact.
        const { props, node } = fakeNode();
        packPalette(node, { ...PACK, surface: '#2a2148cc' });
        expect(props.get('--pack-surface')).toBe('#2a2148cc');
    });

    it('passes the neutral palette through as custom-property references', () => {
        // A derived group follows the theme, which it does by holding `var()`
        // references rather than literals. Quoting or mangling them would leave
        // Recents unpainted.
        const { props, node } = fakeNode();
        packPalette(node, NEUTRAL_PALETTE);
        expect(props.get('--pack-surface')).toBe('var(--bg-hover)');
        expect(props.get('--pack-chroma')).toBe('var(--text-dim)');
    });

    it('enumerates exactly the roles a palette has', () => {
        // Guards the one thing a type checker cannot: a role added to the Rust
        // struct, generated into `PackPalette`, and never emitted because this
        // list was not updated.
        expect([...PALETTE_ROLES].sort()).toEqual(Object.keys(PACK).sort());
    });
});
