/**
 * Crash-recovery UI state. At boot (`init`) it asks `recoverySession` which
 * snapshots belong to crashed sessions, and if any exist surfaces them in
 * `RecoveryModal`. Each entry can be Restored (loaded into a fresh tab,
 * marked dirty since it has no backing file) or Discarded. A snapshot whose
 * zip fails to open is reported and dropped rather than throwing.
 */
import { shell } from '../multi_tab/shell.svelte';
import { app } from './app.svelte';
import { loadError, parseLoadErrorMessage } from './loadError.svelte';
import { readSnapshot, removeSnapshot, type RecoveryEntry } from '../storage/recovery';
import { collectRecovery, initRecoverySession } from './recoverySession';
import { processRecording } from '../recording/recorder.svelte';

class RecoveryState {
    /** True while the recovery modal is mounted. */
    open = $state(false);
    /** Snapshots awaiting a decision. */
    entries = $state<RecoveryEntry[]>([]);

    /** Boot the recovery session and prompt if a crash left snapshots. */
    async init(): Promise<void> {
        const { crashed, live } = initRecoverySession();
        // Recording scratch dirs share the snapshots' orphan rule: dirs
        // owned by a cleanly-exited session are garbage, crashed sessions'
        // dirs are kept for adoption by a restore below.
        void processRecording.gcOrphans(crashed, live);
        const offered = await collectRecovery(crashed, live);
        if (offered.length > 0) {
            this.entries = offered;
            this.open = true;
        }
    }

    private drop(entry: RecoveryEntry): void {
        this.entries = this.entries.filter((e) => e !== entry);
        if (this.entries.length === 0) this.open = false;
    }

    /** Load a snapshot into a fresh tab, then delete it from disk. */
    async restore(entry: RecoveryEntry): Promise<void> {
        const bytes = await readSnapshot(entry.sessionId, entry.recoveryId);
        this.drop(entry);
        if (!bytes) return;

        const inst = shell.open(entry.name);
        // The crashed tab's recording scratch survives the crash (that's
        // the point of OPFS scratch) — move it onto the restored tab's
        // identity before its recorder scans for the next segment number.
        void processRecording.adoptScratch(entry, inst);
        inst.onHandleReady = async (engine) => {
            try {
                await engine.api.openDocument(bytes);
                const name = await engine.api.documentName();
                shell.setName(inst.id, name);
                // Recovered work has no backing file — keep it dirty so
                // closing the tab still prompts and autosave re-snapshots it.
                engine.api.markDirty();
                await inst.syncCanvasRect();
                await app.refreshLayerTree();
                await app.refreshVeilList();
                app.requestFrame();
            } catch (e) {
                loadError.show(parseLoadErrorMessage(e));
                shell.close(inst.id);
            }
        };
        await removeSnapshot(entry.sessionId, entry.recoveryId).catch(() => {});
    }

    /** Discard a snapshot without restoring it. */
    async discard(entry: RecoveryEntry): Promise<void> {
        this.drop(entry);
        await removeSnapshot(entry.sessionId, entry.recoveryId).catch(() => {});
        await processRecording.discardScratch(entry).catch(() => {});
    }

    async restoreAll(): Promise<void> {
        for (const e of [...this.entries]) await this.restore(e);
    }

    async discardAll(): Promise<void> {
        for (const e of [...this.entries]) await this.discard(e);
    }

    /** Dismiss without choosing. Unhandled snapshots become orphans of a
     *  no-longer-registered session and are GC'd on the next boot. */
    close(): void {
        this.open = false;
        this.entries = [];
    }
}

export const recovery = new RecoveryState();
