<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { bindingSite } from '../../actions/binding_site';
    import Icon from '../../icons/Icon.svelte';
    import { THUMB_SIZE } from './thumbnails.svelte';
    import { toggleMaskLink, type MaskLinkState } from './maskChain';

    let { mask, thumbnail, active, enabled, onselect, oncontextmenu, onupdate }: {
        mask: MaskLinkState;
        thumbnail: string;
        active: boolean;
        enabled: boolean;
        onselect: (event: MouseEvent) => void;
        oncontextmenu: (event: MouseEvent) => void;
        onupdate: () => void;
    } = $props();

    function toggle(event: MouseEvent) {
        event.stopPropagation();
        if (app.engine && toggleMaskLink(app.engine.api, mask)) onupdate();
    }
</script>

<button
    class="chain-btn"
    class:unlinked={!mask.linkedToHost}
    disabled={!mask.editable}
    onclick={toggle}
    title={mask.linkedToHost ? 'Unlink mask from layer transforms' : 'Link mask to layer transforms'}
    aria-label={mask.linkedToHost ? 'Unlink mask from layer transforms' : 'Link mask to layer transforms'}
>
    <Icon name={mask.linkedToHost ? 'fa6-solid:link' : 'fa6-solid:link-slash'} />
</button>

{#if thumbnail}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <img
        class="thumb"
        class:thumb-active={active}
        class:mask-disabled={!enabled}
        src={thumbnail}
        alt="mask"
        width={THUMB_SIZE}
        height={THUMB_SIZE}
        draggable="false"
        use:bindingSite={{ name: 'maskThumb', ctx: () => ({ layerId: mask.id }) }}
        onclick={onselect}
        oncontextmenu={oncontextmenu}
    />
{/if}

<style>
    .chain-btn {
        width: 18px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        flex-shrink: 0;
        border: 0;
        border-radius: 4px;
        background: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
    }
    .chain-btn:hover:not(:disabled) { color: var(--text); }
    .chain-btn.unlinked { color: var(--text-dim); }
    .chain-btn:disabled { cursor: default; opacity: 0.45; }
    .thumb {
        width: 32px;
        height: 32px;
        border: 2px solid var(--text-dim);
        border-radius: 4px;
        flex-shrink: 0;
        cursor: pointer;
        image-rendering: pixelated;
        background: var(--thumb-bg);
    }
    .thumb-active { border-color: var(--accent); }
    .mask-disabled { opacity: 0.4; }
</style>
