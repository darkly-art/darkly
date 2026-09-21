/**
 * Global toggle for the command palette (Ctrl+Shift+P). The `commandPalette`
 * action and the palette's own Escape/click-out write here.
 */
import { dialogState } from './dialogState.svelte';

export const commandPalette = dialogState();
