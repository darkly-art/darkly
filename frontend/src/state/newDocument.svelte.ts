/**
 * Global toggle for the "New Document" modal. The `newDocument` action
 * dispatches into this; the hamburger menu reads it.
 */
class NewDocumentState {
    open = $state(false);
}

export const newDocument = new NewDocumentState();
