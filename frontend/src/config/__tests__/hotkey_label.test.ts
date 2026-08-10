import { describe, it, expect, vi, beforeAll } from 'vitest';

// The config store reads its three layers out of the wasm bundle; stub the
// whole surface so the module graph loads in the node test env, with a
// fixture table standing in for the resolved `user → overlay → defaults`.
const HOTKEYS: Record<string, string> = {
    // Multi-binding action: the YAML list arrives joined with `|`.
    'hotkeys.commandPalette': '$mod+Shift+KeyP|$mod+KeyF',
    'hotkeys.openSettings': '$mod+Comma',
    'hotkeys.isolateLayer': '',
};
vi.mock('../../../wasm/pkg/darkly_wasm', () => ({
    config_get: (key: string) => HOTKEYS[key],
    config_set: () => {},
    config_reset: () => {},
    config_reset_all: () => {},
    config_base_names: () => [],
    config_base_value: (key: string) => HOTKEYS[key],
    config_schema: () => '[]',
    config_version: () => 1,
}));
vi.mock('../../storage', () => ({
    storage: { remove: vi.fn() },
    readJson: vi.fn(async () => null),
    writeJson: vi.fn(async () => {}),
}));

import { config, effectiveHotkeys, formatHotkey, hotkeyLabel, tooltipForAction } from '../store.svelte';

beforeAll(async () => {
    vi.stubGlobal('navigator', { userAgent: 'Linux x86_64' });
    await config.init();
});

describe('hotkey display for multi-binding actions', () => {
    it('splits the `|`-joined config value into separate bindings', () => {
        expect(effectiveHotkeys('commandPalette')).toEqual([
            '$mod+Shift+KeyP',
            '$mod+KeyF',
        ]);
        expect(effectiveHotkeys('isolateLayer')).toEqual([]);
    });

    // Regression: menus, the command palette and tooltips used to hand the raw
    // `hotkeys.<id>` value to `formatHotkey`, which formats a single chord. The
    // second binding leaked through verbatim and `Key…` substitution ran on the
    // spliced part, rendering `Ctrl+Shift+P|$mod+F`.
    it('shows only the first binding, fully substituted', () => {
        expect(hotkeyLabel('commandPalette')).toBe('Ctrl+Shift+P');
        expect(hotkeyLabel('commandPalette')).not.toContain('|');
        expect(hotkeyLabel('commandPalette')).not.toContain('$mod');
    });

    it('carries that through to action tooltips', () => {
        expect(tooltipForAction('Find', 'commandPalette')).toBe('Find (Ctrl+Shift+P)');
    });

    it('leaves single-binding actions alone, and omits unbound ones', () => {
        expect(hotkeyLabel('openSettings')).toBe('Ctrl+,');
        expect(hotkeyLabel('isolateLayer')).toBeUndefined();
        expect(tooltipForAction('Isolate', 'isolateLayer')).toBe('Isolate');
    });

    it('formats one chord at a time', () => {
        expect(formatHotkey('layerPanel:$mod+Shift+KeyP')).toBe('Ctrl+Shift+P');
    });
});
