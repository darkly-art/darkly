/**
 * Global toggle for the Export Timelapse modal. The `export-timelapse`
 * action dispatches into this; the modal reads the active tab's recording
 * info and drives the MP4 / GIF export + download once the user confirms.
 */
class ExportTimelapseState {
    open = $state(false);
}

export const exportTimelapse = new ExportTimelapseState();
