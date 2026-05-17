/**
 * Global toggle for the Export Image modal. The `export-image` action
 * dispatches into this; the modal itself drives the readback + encode +
 * download once the user confirms.
 */
class ExportImageState {
    open = $state(false);
}

export const exportImage = new ExportImageState();
