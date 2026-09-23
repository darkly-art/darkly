/**
 * Global toggle for the Settings modal. The action registry dispatches
 * `openSettings` into this; the hamburger menu also writes here.
 */
import { dialogState } from './dialogState.svelte';

export const settings = dialogState();
