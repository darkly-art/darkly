/**
 * Whether the "which pack do you want to export?" chooser is open.
 *
 * Import needs no chooser — the OS file picker is the chooser — but export
 * does, and without pack-management UI there is nowhere else to invoke it
 * from. Follows `layerPicker`'s shape: the action sets a flag, a component
 * mounts the modal.
 *
 * Superseded when the pack-management push lands: the affordance moves onto
 * the pack row and this goes away.
 */
class PackExportState {
    open = $state(false);
}

export const packExport = new PackExportState();
