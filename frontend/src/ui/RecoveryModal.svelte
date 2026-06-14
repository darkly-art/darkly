<script lang="ts">
    import { onDestroy } from 'svelte';
    import Modal from './Modal.svelte';
    import { recovery } from '../state/recovery.svelte';
    import { readSnapshot, snapshotThumbnail, type RecoveryEntry } from '../storage/recovery';

    // Object URLs minted for thumbnails — revoked on teardown.
    const urls: string[] = [];
    onDestroy(() => {
        for (const u of urls) URL.revokeObjectURL(u);
    });

    /** Load a snapshot's thumbnail as an object URL, or null if absent. */
    async function loadThumb(entry: RecoveryEntry): Promise<string | null> {
        const bytes = await readSnapshot(entry.sessionId, entry.recoveryId);
        if (!bytes) return null;
        const png = snapshotThumbnail(bytes);
        if (!png) return null;
        const url = URL.createObjectURL(
            new Blob([png as Uint8Array<ArrayBuffer>], { type: 'image/png' }),
        );
        urls.push(url);
        return url;
    }

    function onRestore(entry: RecoveryEntry) { void recovery.restore(entry); }
    function onDiscard(entry: RecoveryEntry) { void recovery.discard(entry); }
    function onRestoreAll() { void recovery.restoreAll(); }
    function onDiscardAll() { void recovery.discardAll(); }
</script>

<Modal bind:open={recovery.open} title="Recover unsaved work" size="md">
    <p class="message">
        Darkly didn't shut down cleanly. These documents had unsaved changes —
        restore the ones you want to keep.
    </p>

    <ul class="entries">
        {#each recovery.entries as entry (entry.recoveryId)}
            <li class="entry">
                <div class="thumb">
                    {#await loadThumb(entry) then url}
                        {#if url}
                            <img src={url} alt="" />
                        {:else}
                            <div class="thumb-placeholder"></div>
                        {/if}
                    {/await}
                </div>
                <span class="name" title={entry.name}>{entry.name}</span>
                <div class="row-actions">
                    <button type="button" class="danger" onclick={() => onDiscard(entry)}>
                        Discard
                    </button>
                    <button type="button" class="primary" onclick={() => onRestore(entry)}>
                        Restore
                    </button>
                </div>
            </li>
        {/each}
    </ul>

    <div class="actions">
        <button type="button" class="danger" onclick={onDiscardAll}>Discard all</button>
        <button type="button" class="primary" onclick={onRestoreAll}>Restore all</button>
    </div>
</Modal>

<style>
    .message {
        margin: 0 0 16px;
        font-size: 13px;
        line-height: 1.5;
        color: var(--text);
    }

    .entries {
        list-style: none;
        margin: 0 0 18px;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 6px;
        max-height: 320px;
        overflow-y: auto;
    }

    .entry {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 6px 8px;
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
    }

    .thumb {
        width: 44px;
        height: 44px;
        flex: none;
        display: flex;
        align-items: center;
        justify-content: center;
        background: var(--bg-hover);
        border-radius: 3px;
        overflow: hidden;
    }

    .thumb img {
        max-width: 100%;
        max-height: 100%;
        object-fit: contain;
    }

    .thumb-placeholder {
        width: 100%;
        height: 100%;
    }

    .name {
        flex: 1;
        min-width: 0;
        font-size: 13px;
        color: var(--text);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .row-actions {
        display: flex;
        gap: 6px;
        flex: none;
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        border-top: 1px solid var(--bg-hover);
        padding-top: 14px;
    }

    button {
        padding: 6px 14px;
        font-size: 13px;
        border-radius: 4px;
        border: 1px solid var(--bg-hover);
        background: transparent;
        color: var(--text);
        cursor: pointer;
    }

    button:hover:not(:disabled) {
        background: var(--bg-hover);
    }

    .primary {
        background: var(--accent);
        border-color: var(--accent);
        color: #ffffff;
    }

    .primary:hover:not(:disabled) {
        filter: brightness(1.1);
        background: var(--accent);
    }

    .danger {
        color: var(--danger, #e35858);
        border-color: var(--danger, #e35858);
    }

    .danger:hover {
        background: rgba(227, 88, 88, 0.12);
    }
</style>
