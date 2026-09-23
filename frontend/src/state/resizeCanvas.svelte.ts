/**
 * Global toggle for the "Canvas Size" (resize) modal. The `resizeCanvas`
 * action dispatches into this; the modal reads it.
 */
import { dialogState } from './dialogState.svelte';

export const resizeCanvas = dialogState();
