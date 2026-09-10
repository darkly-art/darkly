import { describe, it, expect } from 'vitest';
import { hexToColor, colorToHex, colorToHexRgb, hexToRgb01, rgb01ToHex } from '../color';

describe('color hex conversions', () => {
    it('hex_round_trips_through_color_in_both_widths', () => {
        // The 8-digit case is what `hexToRgb01` alone could not express: it
        // matched 6 digits only and returned black for anything wider.
        expect(colorToHex(hexToColor('#3355ff')!)).toBe('#3355ffff');
        expect(colorToHex(hexToColor('#3355ffaa')!)).toBe('#3355ffaa');
    });

    it('a_six_digit_hex_is_opaque', () => {
        expect(hexToColor('#3355ff')).toEqual({ r: 0x33, g: 0x55, b: 0xff, a: 255 });
    });

    it('a_malformed_hex_is_null_not_black', () => {
        for (const bad of ['#xyz', 'ff00', '#ff00', '#12345', '#1234567', '', 'rebeccapurple']) {
            expect(hexToColor(bad), `for ${bad}`).toBeNull();
        }
    });

    it('parsing_accepts_a_missing_hash_and_mixed_case', () => {
        expect(hexToColor('3355FF')).toEqual({ r: 0x33, g: 0x55, b: 0xff, a: 255 });
        expect(hexToColor('  #3355ff  ')).toEqual({ r: 0x33, g: 0x55, b: 0xff, a: 255 });
    });

    it('the_display_form_drops_alpha', () => {
        expect(colorToHexRgb({ r: 0x33, g: 0x55, b: 0xff, a: 0x80 })).toBe('#3355ff');
    });

    it('components_are_clamped_and_padded', () => {
        expect(colorToHex({ r: -5, g: 300, b: 0, a: 255 })).toBe('#00ff00ff');
    });

    it('the_rgb01_helpers_still_round_trip', () => {
        expect(rgb01ToHex(hexToRgb01('#3355ff'))).toBe('#3355ff');
        // Alpha is discarded rather than corrupting the triple.
        expect(hexToRgb01('#3355ffaa')).toEqual(hexToRgb01('#3355ff'));
        // The documented fallback for callers that never handled null.
        expect(hexToRgb01('nonsense')).toEqual([0, 0, 0]);
    });
});
