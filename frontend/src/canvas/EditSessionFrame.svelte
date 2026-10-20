<script lang="ts">
    import type { EditSessionKind } from '../engine/protocol_gen';
    import Icon from '../icons/Icon.svelte';
    import { tooltipForAction } from '../config/store.svelte';
    import { editSessionControls } from './edit_session_controls';

    // The open session arrives as a prop, read off the instance's
    // `engineState` by the canvas, so each tab's frame follows its own
    // engine and the tests can drive it directly.
    let {
        session,
        onapply,
        ondiscard,
    }: {
        session: EditSessionKind | null;
        onapply: () => void;
        ondiscard: () => void;
    } = $props();

    const Controls = $derived(session ? editSessionControls(session.id) : undefined);
</script>

<!-- A border around the viewport while an edit session is open, with a tab
     at its bottom right naming the session and holding its Discard and
     Apply. The border never takes the pointer; the tab does. -->
{#if session}
    <div class="edit-session-frame">
        <div class="tab" role="toolbar" aria-label={session.displayName}>
            <span class="label">
                <Icon name={session.icon} />
                {session.displayName}
            </span>
            {#if Controls}
                <Controls />
            {/if}
            <button
                type="button"
                class="icon-btn"
                aria-label="Discard"
                title={tooltipForAction('Discard', 'discardEditSession')}
                onclick={ondiscard}
            ><Icon name="fa6-solid:xmark" /></button>
            <button
                type="button"
                class="icon-btn"
                aria-label="Apply"
                title={tooltipForAction('Apply', 'applyEditSession')}
                onclick={onapply}
            ><Icon name="fa6-solid:check" /></button>
        </div>
    </div>
{/if}

<style>
    /* Brand purple: the token reserved for what has to be seen first, and
     * the one colour that stands out against the grey canvas backdrop. */
    .edit-session-frame {
        position: absolute;
        inset: 0;
        border: 2px solid var(--brand);
        pointer-events: none;
    }

    /* Flush with the border's bottom-right corner, in its colour, so the
     * tab reads as part of it. */
    .tab {
        position: absolute;
        right: 0;
        bottom: 0;
        display: flex;
        align-items: center;
        gap: 2px;
        padding: 2px 2px 0 10px;
        border-top-left-radius: var(--radius-md);
        background: var(--brand);
        color: var(--on-brand);
        font-size: 12px;
        pointer-events: auto;
    }

    .label {
        display: flex;
        align-items: center;
        gap: 6px;
        margin-right: 6px;
        white-space: nowrap;
    }

    .icon-btn {
        width: 24px;
        height: 22px;
        color: var(--on-brand);
        font-size: 13px;
    }

    .icon-btn:hover {
        background: rgba(255, 255, 255, 0.18);
        color: var(--on-brand);
    }
</style>
