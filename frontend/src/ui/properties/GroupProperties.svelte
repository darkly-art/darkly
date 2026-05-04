<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { group }: {
        group: { id: number; passthrough: boolean };
    } = $props();

    function onPassthroughChange(e: Event) {
        const checked = (e.target as HTMLInputElement).checked;
        app.handle?.set_group_passthrough(group.id, checked);
        app.refreshLayerTree();
        app.requestFrame();
    }
</script>

<label class="row">
    <input
        type="checkbox"
        class="checkbox"
        checked={group.passthrough}
        onchange={onPassthroughChange}
    />
    <span class="label">Passthrough</span>
</label>

<style>
    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
        cursor: pointer;
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
    }

    .checkbox {
        accent-color: var(--accent);
        cursor: pointer;
    }
</style>
