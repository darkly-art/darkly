/**
 * Which "add a layer" picker modal is open, if any. The `newVeil` / `newVoid`
 * / `newFilterLayer` actions set this; `ui/layers/LayerPickers.svelte` mounts
 * the matching modal.
 */
export type LayerPickerKind = 'veil' | 'void' | 'filter';

class LayerPickerState {
    kind = $state<LayerPickerKind | null>(null);
}

export const layerPicker = new LayerPickerState();
