/**
 * Global toggle for the "New Document" modal. The `newDocument` action
 * dispatches into this; the hamburger menu reads it.
 */
import { dialogState } from './dialogState.svelte';

export const newDocument = dialogState();
