<script lang="ts">
    import Modal from './Modal.svelte';
    import { closeGuard } from '../multi_tab/closeGuard.svelte';
    import { canSave } from '../storage/fileHandle';

    function onSave() { void closeGuard.save(); }
    function onDiscard() { closeGuard.discard(); }
    function onCancel() { closeGuard.cancel(); }

    const noSaveTooltip =
        "Filesystem save isn't supported in this browser — try Chrome, Edge, or Safari.";
</script>

<Modal bind:open={closeGuard.open} title="Unsaved changes" size="sm">
    <p class="message">
        <strong>{closeGuard.tabName}</strong> has unsaved changes.
        Save before closing?
    </p>
    <div class="actions">
        <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
        <button type="button" class="danger" onclick={onDiscard}>Discard</button>
        <button
            type="button"
            class="primary"
            disabled={!canSave}
            title={canSave ? undefined : noSaveTooltip}
            onclick={onSave}
        >Save</button>
    </div>
</Modal>

<style>
    .message {
        margin: 0 0 18px;
        font-size: 13px;
        line-height: 1.5;
        color: var(--text);
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }

    .actions button {
        padding: 6px 14px;
        font-size: 13px;
        border-radius: 4px;
        border: 1px solid var(--bg-hover);
        background: transparent;
        color: var(--text);
        cursor: pointer;
    }

    .actions button:hover:not(:disabled) {
        background: var(--bg-hover);
    }

    .actions button:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }

    .actions .primary {
        background: var(--accent);
        border-color: var(--accent);
        color: #ffffff;
    }

    .actions .primary:hover:not(:disabled) {
        filter: brightness(1.1);
        background: var(--accent);
    }

    .actions .danger {
        color: var(--danger, #e35858);
        border-color: var(--danger, #e35858);
    }

    .actions .danger:hover {
        background: rgba(227, 88, 88, 0.12);
    }
</style>
