/**
 * Global toggle for the "Image Size" (rescale) modal. The `rescaleImage`
 * action dispatches into this; the modal reads it. Distinct from
 * `resizeCanvas` (which resizes the window without scaling content).
 */
import { dialogState } from './dialogState.svelte';

export const imageRescale = dialogState();
