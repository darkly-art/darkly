<script lang="ts">
    import Modal from './Modal.svelte';
    import { closeGuard } from '../multi_tab/closeGuard.svelte';

    function onSave() { void closeGuard.save(); }
    function onDiscard() { closeGuard.discard(); }
    function onCancel() { closeGuard.cancel(); }
</script>

<Modal bind:open={closeGuard.open} title="Unsaved changes" size="sm">
    <p class="message">
        <strong>{closeGuard.tabName}</strong> has unsaved changes.
        Save before closing?
    </p>
    <div class="dialog-actions">
        <button type="button" class="btn" onclick={onCancel}>Cancel</button>
        <button type="button" class="btn danger" onclick={onDiscard}>Discard</button>
        <button type="button" class="btn primary" onclick={onSave}>Save</button>
    </div>
</Modal>

<style>
    .message {
        margin: 0 0 18px;
        font-size: 13px;
        line-height: 1.5;
        color: var(--text);
    }
</style>
