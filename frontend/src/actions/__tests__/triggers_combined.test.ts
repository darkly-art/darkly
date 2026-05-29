import { describe, it, expect } from 'vitest';
import {
    parseTriggerStrings,
    serializeTriggers,
    detectKind,
} from '../triggers_combined';

describe('parseTriggerStrings', () => {
    it('returns [] for empty / undefined storage', () => {
        expect(parseTriggerStrings(undefined, undefined)).toEqual([]);
        expect(parseTriggerStrings('', '')).toEqual([]);
    });

    it('parses keyboard-only list', () => {
        expect(parseTriggerStrings('KeyZ|$mod+Shift+KeyE', '')).toEqual([
            { kind: 'kbd', binding: 'KeyZ' },
            { kind: 'kbd', binding: '$mod+Shift+KeyE' },
        ]);
    });

    it('parses mouse-only list', () => {
        expect(parseTriggerStrings('', 'layerThumb:alt+click|maskThumb:alt+click')).toEqual([
            { kind: 'mouse', binding: 'layerThumb:alt+click' },
            { kind: 'mouse', binding: 'maskThumb:alt+click' },
        ]);
    });

    it('groups keyboard first, then mouse', () => {
        // The visible row order matters: the Settings UI shows triggers in
        // this exact sequence. Mouse-after-keyboard keeps the dispatch
        // priority within each namespace intact (resolution is per-namespace).
        const triggers = parseTriggerStrings(
            'KeyA',
            'canvas:click',
        );
        expect(triggers).toEqual([
            { kind: 'kbd', binding: 'KeyA' },
            { kind: 'mouse', binding: 'canvas:click' },
        ]);
    });

    it('drops empty pipe-segments', () => {
        // Preset overrides can leave dangling `|` if a binding is unset
        // mid-list. The Settings UI must not surface ghost rows for them.
        expect(parseTriggerStrings('KeyA||KeyB', '')).toEqual([
            { kind: 'kbd', binding: 'KeyA' },
            { kind: 'kbd', binding: 'KeyB' },
        ]);
    });
});

describe('serializeTriggers', () => {
    it('round-trips through parseTriggerStrings', () => {
        const triggers = parseTriggerStrings('KeyZ|KeyA', 'canvas:click');
        expect(serializeTriggers(triggers)).toEqual({
            kbd: 'KeyZ|KeyA',
            mouse: 'canvas:click',
        });
    });

    it('writes empty strings for namespaces with no triggers', () => {
        // A user who only has keyboard triggers should reset the mouse
        // namespace to an empty (explicit-unbound) override — not leave it
        // dangling at the prior value.
        expect(serializeTriggers([{ kind: 'kbd', binding: 'KeyZ' }])).toEqual({
            kbd: 'KeyZ',
            mouse: '',
        });
    });

    it('drops triggers whose binding is empty (ghost rows)', () => {
        // Freshly-added rows that the user never finished capturing into
        // shouldn't pollute storage when they save.
        expect(serializeTriggers([
            { kind: 'kbd', binding: 'KeyZ' },
            { kind: 'kbd', binding: '' },
            { kind: 'mouse', binding: 'canvas:click' },
        ])).toEqual({
            kbd: 'KeyZ',
            mouse: 'canvas:click',
        });
    });

    it('drops triggers whose chord is empty even if site prefix is present', () => {
        // When the action requires a site (no Anywhere option), addTrigger
        // seeds the new row with `<site>:` and a blank chord. If the user
        // navigates away without capturing, the half-baked binding must
        // not be persisted.
        expect(serializeTriggers([
            { kind: 'mouse', binding: 'canvas:' },
            { kind: 'mouse', binding: 'canvas:alt+click' },
        ])).toEqual({
            kbd: '',
            mouse: 'canvas:alt+click',
        });
    });
});

describe('detectKind', () => {
    it('classifies bare keyboard chords as kbd', () => {
        expect(detectKind('KeyA')).toBe('kbd');
        expect(detectKind('Delete')).toBe('kbd');
        expect(detectKind('$mod+Shift+KeyE')).toBe('kbd');
        expect(detectKind('Space')).toBe('kbd');
    });

    it('classifies mouse chords by their terminal verb', () => {
        // The chord namespace is distinguished only at the *terminal* —
        // everything else looks alike (`$mod+Shift+…` works for both).
        expect(detectKind('click')).toBe('mouse');
        expect(detectKind('alt+click')).toBe('mouse');
        expect(detectKind('$mod+drag')).toBe('mouse');
        expect(detectKind('shift+doubleClick')).toBe('mouse');
        expect(detectKind('middleClick')).toBe('mouse');
        expect(detectKind('rightDrag')).toBe('mouse');
    });

    it('treats empty chord as kbd (default for unbound rows)', () => {
        // An empty trigger row's kind is meaningful only when the user
        // actually captures — defaulting to kbd matches the most common
        // "Press a key…" prompt.
        expect(detectKind('')).toBe('kbd');
    });
});

describe('namespace migration on kind change (round-trip)', () => {
    it('moves a trigger from hotkeys to mouseclicks when its chord becomes a mouse chord', () => {
        // Round-trip: read kbd-only list, change one trigger's kind to
        // mouse, serialize — the trigger has migrated namespaces while
        // its row-order position is preserved within each list.
        const before = parseTriggerStrings('KeyZ|KeyA', '');
        const after = [
            before[0],                                                 // unchanged kbd
            { kind: 'mouse' as const, binding: 'canvas:$mod+click' },  // migrated
        ];
        expect(serializeTriggers(after)).toEqual({
            kbd: 'KeyZ',
            mouse: 'canvas:$mod+click',
        });
    });

    it('removes the moved trigger from its old namespace', () => {
        // The same Trigger.binding present in both namespaces would never
        // happen organically, but a sloppy migration could double-write.
        // Confirm the new state writes ONLY to the new namespace.
        const after = [
            { kind: 'mouse' as const, binding: 'canvas:alt+click' },
        ];
        const out = serializeTriggers(after);
        expect(out.kbd).toBe('');
        expect(out.mouse).toBe('canvas:alt+click');
    });
});
