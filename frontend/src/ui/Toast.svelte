<script lang="ts">
    import { toast } from '../state/toast.svelte';

    const levelColors: Record<string, string> = {
        success: '#4caf50',
        info: '#2196f3',
        warning: '#ff9800',
        error: '#f44336',
    };
</script>

{#if toast.toasts.length > 0}
    <div class="toast-container">
        {#each toast.toasts as t (t.id)}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="toast"
                style:border-left-color={levelColors[t.level]}
                onclick={() => toast.dismiss(t.id)}
            >
                {t.message}
            </div>
        {/each}
    </div>
{/if}

<style>
    .toast-container {
        position: fixed;
        bottom: 24px;
        left: 50%;
        transform: translateX(-50%);
        display: flex;
        flex-direction: column;
        gap: 8px;
        z-index: 1000;
        pointer-events: none;
    }

    .toast {
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-left: 4px solid;
        border-radius: 6px;
        padding: 10px 16px;
        font-size: 13px;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
        pointer-events: auto;
        cursor: pointer;
        min-width: 240px;
        max-width: 480px;
    }
</style>
