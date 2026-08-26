import { describe, expect, it } from 'vitest';
import type { BrushInfo, BrushPackInfo } from '../../../engine/protocol_gen';
import {
    groupByPack,
    matchesQuery,
    packNamesByBrush,
    withRecents,
    NO_PACK_LABEL,
    RECENTS_ID,
    RECENTS_LABEL,
} from '../grouping';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';

function brush(id: string, name = id, tags: string[] = []): BrushInfo {
    return { id, name, author: '', description: '', tags, icon: null } as BrushInfo;
}

function pack(id: string, name: string, members: string[]): BrushPackInfo {
    return {
        id,
        name,
        description: '',
        icon: 'mdi:brush',
        palette: {
            chroma: '#2f7fe0',
            refraction: '#2fd0c0',
            surface: '#0c1a26',
            ink: '#c3dae9',
        },
        members,
        can_edit_members: true,
        can_edit_identity: true,
    } as BrushPackInfo;
}

/** Identity resolver — icon renderability is the renderer's business and has
 *  its own test. */
const asIs = (icon: string) => icon;
const FALLBACK = 'fa6-solid:folder';

describe('groupByPack', () => {
    it('a_brush_in_two_packs_renders_in_both_groups', () => {
        const ink = brush('ink_pen', 'Ink Pen');
        const groups = groupByPack(
            [ink],
            [pack('basic', 'Basic', ['ink_pen']), pack('favorites', 'Favorites', ['ink_pen'])],
            asIs,
            FALLBACK,
        );

        expect(groups.map(g => g.id)).toEqual(['basic', 'favorites']);
        expect(groups[0].brushes).toEqual([ink]);
        expect(groups[1].brushes).toEqual([ink]);
    });

    it('a_pack_with_no_visible_members_renders_nothing', () => {
        // What keeps an empty Favorites from showing as a broken heading.
        const groups = groupByPack(
            [brush('a')],
            [pack('basic', 'Basic', ['a']), pack('favorites', 'Favorites', [])],
            asIs,
            FALLBACK,
        );
        expect(groups.map(g => g.id)).toEqual(['basic']);
    });

    it('a_pack_whose_members_are_all_filtered_out_renders_nothing', () => {
        const groups = groupByPack(
            [brush('a')],
            [pack('p1', 'One', ['a']), pack('p2', 'Two', ['b'])],
            asIs,
            FALLBACK,
        );
        expect(groups.map(g => g.id)).toEqual(['p1']);
    });

    it('brushes_in_no_pack_render_in_their_own_section', () => {
        const groups = groupByPack(
            [brush('a'), brush('loose')],
            [pack('p1', 'One', ['a'])],
            asIs,
            FALLBACK,
        );
        expect(groups.map(g => g.label)).toEqual(['One', NO_PACK_LABEL]);
        expect(groups[1].brushes.map(b => b.id)).toEqual(['loose']);
        expect(groups[1].id).toBe('');
    });

    it('there_is_no_empty_no_pack_section_when_every_brush_is_grouped', () => {
        const groups = groupByPack([brush('a')], [pack('p1', 'One', ['a'])], asIs, FALLBACK);
        expect(groups).toHaveLength(1);
    });

    it('groups_follow_pack_order_and_members_follow_member_order', () => {
        const groups = groupByPack(
            [brush('x'), brush('y'), brush('z')],
            [pack('second', 'Second', ['z', 'x']), pack('first', 'First', ['y'])],
            asIs,
            FALLBACK,
        );
        expect(groups.map(g => g.id)).toEqual(['second', 'first']);
        expect(groups[0].brushes.map(b => b.id)).toEqual(['z', 'x']);
    });

    it('a_member_naming_a_brush_that_is_gone_is_skipped', () => {
        const groups = groupByPack([brush('a')], [pack('p1', 'One', ['a', 'ghost'])], asIs, FALLBACK);
        expect(groups[0].brushes.map(b => b.id)).toEqual(['a']);
    });

    it('the_icon_is_run_through_the_resolver', () => {
        const groups = groupByPack(
            [brush('a')],
            [pack('p1', 'One', ['a'])],
            () => 'resolved:icon',
            FALLBACK,
        );
        expect(groups[0].icon).toBe('resolved:icon');
    });
});

describe('keyboard cell indexing', () => {
    it('keyboard_navigation_indexes_rendered_cells', () => {
        // A brush in two packs renders twice, so the flat cell list is longer
        // than the filtered brush list. Indexing the filter would highlight
        // the wrong cell.
        const filtered = [brush('ink_pen', 'Ink Pen'), brush('charcoal', 'Charcoal')];
        const groups = groupByPack(
            filtered,
            [
                pack('basic', 'Basic', ['ink_pen']),
                pack('dry', 'Dry Media', ['charcoal']),
                pack('favorites', 'Favorites', ['ink_pen']),
            ],
            asIs,
            FALLBACK,
        );
        const cells = groups.flatMap(g => g.brushes);

        expect(cells.map(b => b.id)).toEqual(['ink_pen', 'charcoal', 'ink_pen']);
        expect(cells).toHaveLength(3);
        expect(filtered).toHaveLength(2);

        // The last cell is the Favorites copy of Ink Pen — under the old flat
        // indexing, index 2 did not exist at all.
        expect(cells[2].id).toBe('ink_pen');
    });
});

describe('search', () => {
    const packs = [pack('wet', 'Wet Media', ['rw'])];
    const names = packNamesByBrush(packs);

    it('an_empty_query_matches_everything', () => {
        expect(matchesQuery(brush('rw', 'Rough Watercolor'), '', names)).toBe(true);
        expect(matchesQuery(brush('rw', 'Rough Watercolor'), '   ', names)).toBe(true);
    });

    it('matches_on_name_tokens_in_any_order', () => {
        const b = brush('rw', 'Rough Watercolor');
        expect(matchesQuery(b, 'rough water', names)).toBe(true);
        expect(matchesQuery(b, 'water rough', names)).toBe(true);
        expect(matchesQuery(b, 'rough xxx', names)).toBe(false);
    });

    it('matches_on_pack_name', () => {
        // Searching "wet" should find the brushes in Wet Media — the facet
        // that moved from the deleted `category` field onto the pack.
        expect(matchesQuery(brush('rw', 'Rough Watercolor'), 'wet', names)).toBe(true);
        expect(matchesQuery(brush('other', 'Other'), 'wet', names)).toBe(false);
    });

    it('matches_on_tags', () => {
        const b = brush('rw', 'Rough Watercolor', ['textured']);
        expect(matchesQuery(b, 'textured', names)).toBe(true);
    });
});

describe('packNamesByBrush', () => {
    it('collects_every_pack_a_brush_is_in', () => {
        const map = packNamesByBrush([
            pack('basic', 'Basic', ['ink_pen']),
            pack('favorites', 'Favorites', ['ink_pen', 'charcoal']),
        ]);
        expect(map.get('ink_pen')).toEqual(['Basic', 'Favorites']);
        expect(map.get('charcoal')).toEqual(['Favorites']);
        expect(map.get('nope')).toBeUndefined();
    });
});

describe('withRecents', () => {
    const visible = [brush('a', 'Alpha'), brush('b', 'Beta'), brush('c', 'Gamma')];
    const base = groupByPack(visible, [pack('p', 'Pack', ['a', 'b', 'c'])], i => i, 'x');

    it('prepends at most `limit`, newest first', () => {
        const out = withRecents(base, ['c', 'b', 'a'], visible, 2, 'star');
        expect(out[0].id).toBe(RECENTS_ID);
        expect(out[0].label).toBe(RECENTS_LABEL);
        expect(out[0].brushes.map(b => b.id)).toEqual(['c', 'b']);
    });

    it('skips ids that no longer resolve rather than placeholding them', () => {
        // A deleted brush, or one the current search excludes.
        const out = withRecents(base, ['gone', 'a'], visible, 5, 'star');
        expect(out[0].brushes.map(b => b.id)).toEqual(['a']);
    });

    it('yields no group at all when nothing resolves', () => {
        expect(withRecents(base, ['gone'], visible, 5, 'star')).toEqual(base);
        expect(withRecents(base, [], visible, 5, 'star')).toEqual(base);
    });

    it('leaves the same brushes in their packs too', () => {
        const out = withRecents(base, ['a'], visible, 5, 'star');
        expect(out[0].brushes.map(b => b.id)).toEqual(['a']);
        expect(out[1].brushes.map(b => b.id)).toEqual(['a', 'b', 'c']);
    });

    it('carries no pack, so nothing about it is editable', () => {
        const out = withRecents(base, ['a'], visible, 5, 'star');
        expect(out[0].pack).toBeNull();
    });
});

describe('BrushGroup.pack', () => {
    it('is the pack for a real group and null for a derived one', () => {
        const p = pack('p', 'Pack', ['a']);
        const groups = groupByPack([brush('a'), brush('loose')], [p], i => i, 'x');
        expect(groups[0].pack).toBe(p);
        // The "in no pack" section is computed, not stored: it has no pack to
        // ask about permissions, which is what stops a consumer from having to
        // recognise its sentinel id.
        expect(groups[1].label).toBe(NO_PACK_LABEL);
        expect(groups[1].pack).toBeNull();
    });
});

describe('BrushGroup.palette', () => {
    it('is the pack\'s own, and the theme\'s neutrals for a derived group', () => {
        // Every group carries a real palette, so no consumer has to ask whether
        // a pack is behind it before it can paint anything.
        const p = pack('p', 'Pack', ['a']);
        const groups = groupByPack([brush('a'), brush('loose')], [p], i => i, 'x');
        expect(groups[0].palette).toEqual(p.palette);
        expect(groups[1].palette).toEqual(NEUTRAL_PALETTE);

        const withR = withRecents(groups, ['a'], [brush('a')], 5, 'star');
        expect(withR[0].palette).toEqual(NEUTRAL_PALETTE);
    });
});
