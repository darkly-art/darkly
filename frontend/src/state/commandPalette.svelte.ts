/**
 * Global toggle for the command palette (Ctrl+Shift+P). The `commandPalette`
 * action and the palette's own Escape/click-out write here.
 */
class CommandPaletteState {
    open = $state(false);
}

export const commandPalette = new CommandPaletteState();
