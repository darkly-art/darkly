<script lang="ts">
    import type { ActionRegistration } from '../../actions/registry';
    import TriggerListEditor from './TriggerListEditor.svelte';

    type Props = { action: ActionRegistration; showScope: boolean };
    let { action, showScope }: Props = $props();

    const desc = $derived(action.description ?? null);
</script>

<div class="row">
    <div class="label-col">
        <div class="label">{action.displayName}</div>
        {#if desc}<div class="desc">{desc}</div>{/if}
    </div>

    <div class="triggers-col">
        <TriggerListEditor {action} {showScope} />
    </div>
</div>

<style>
    .row {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 16px;
        align-items: start;
        padding: 10px 12px;
        border-bottom: 1px solid color-mix(in srgb, var(--bg-hover) 50%, transparent);
    }
    .row:last-child { border-bottom: none; }

    .label-col { min-width: 0; }
    .label {
        font-size: 13px;
        color: var(--text);
        overflow-wrap: anywhere;
    }
    .desc {
        font-size: 11px;
        color: var(--text-muted);
        margin-top: 2px;
        overflow-wrap: anywhere;
    }

    .triggers-col { min-width: 0; }
</style>
