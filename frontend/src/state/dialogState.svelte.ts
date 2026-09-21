/**
 * The open/closed flag behind a modal dialog.
 *
 * Eight dialogs are exactly this and nothing more: an action sets `open`, a
 * component mounts against it. Each used to declare its own one-field class,
 * which is eight copies of the same three lines and eight places for the shape
 * to drift.
 *
 * Dialogs that carry more than the flag (which tab to land on, which node to
 * act against) keep their own class and compose this rather than extending it:
 * the extra state is theirs, and a factory that grew a config parameter per
 * dialog would be the duplication written a second way.
 *
 * The file per dialog stays. It is where that dialog's doc comment lives, and
 * the comment is the part worth keeping: `packExport` explains why a chooser
 * exists at all, `imageRescale` explains how it differs from `resizeCanvas`.
 */
export interface DialogState {
    open: boolean;
    show(): void;
    hide(): void;
}

export function dialogState(): DialogState {
    let open = $state(false);
    return {
        get open() {
            return open;
        },
        set open(v: boolean) {
            open = v;
        },
        show() {
            open = true;
        },
        hide() {
            open = false;
        },
    };
}
