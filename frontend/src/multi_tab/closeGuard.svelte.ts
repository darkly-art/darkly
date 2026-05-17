/**
 * Unsaved-changes guard for tab close. Holds the in-flight "user clicked
 * × on a dirty tab" prompt state and routes the modal's three buttons
 * (Save / Discard / Cancel) back through the shell.
 *
 * The guard always focuses the target tab before showing the modal —
 * `saveDocument()` operates on `app.handle`, so the engine the user is
 * being prompted about must be the active one. Side effect: the modal
 * doubles as a visual cue for which tab is in question.
 */

import { shell } from './shell.svelte';
import { saveDocument } from '../storage/saveDocument';

class CloseGuardState {
    /** True while the modal is mounted. */
    open = $state(false);
    /** The tab id awaiting a decision. Empty string when `open` is false. */
    tabId = $state('');
    /** Snapshot of the tab's display name at the time the modal opened.
     *  Snapshotted (rather than re-read each render) so a rename racing
     *  the modal doesn't shift the prompt text mid-decision. */
    tabName = $state('');

    /** Open the modal for `id`, focusing the tab first so the save flow
     *  (which operates on `app.handle`) sees the correct engine. */
    private openFor(id: string) {
        shell.focus(id);
        this.tabId = id;
        this.tabName = shell.nameOf(id);
        this.open = true;
    }

    /** Public entry point — close `id`, prompting on dirty work. */
    guardedClose(id: string) {
        const inst = shell.instances.find(i => i.id === id);
        if (!inst) return;
        if (!inst.handle?.is_dirty()) {
            shell.close(id);
            return;
        }
        this.openFor(id);
    }

    /** "Cancel" button — close the modal, leave the tab open. */
    cancel() {
        this.open = false;
        this.tabId = '';
    }

    /** "Discard" button — close the tab without saving. */
    discard() {
        const id = this.tabId;
        this.open = false;
        this.tabId = '';
        shell.close(id);
    }

    /** "Save" button — run the save flow against the focused tab. On
     *  success (dirty bit cleared) the tab closes; if the user cancelled
     *  the file picker mid-save the dirty bit stays set and the tab
     *  stays open with the modal dismissed (user can retry from the ×). */
    async save() {
        const id = this.tabId;
        this.open = false;
        this.tabId = '';
        await saveDocument({ forceAs: false });
        const inst = shell.instances.find(i => i.id === id);
        if (inst?.handle && !inst.handle.is_dirty()) {
            shell.close(id);
        }
    }
}

export const closeGuard = new CloseGuardState();

/** True when any open tab has unsaved changes — backs the
 *  `window.beforeunload` handler so the browser prompts on accidental
 *  reload / navigation. Walks every instance because the user may have
 *  multiple dirty tabs open at once. */
export function anyTabDirty(): boolean {
    return shell.instances.some(i => i.handle?.is_dirty() === true);
}
