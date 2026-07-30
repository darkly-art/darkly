<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { bindingSite } from '../../actions/binding_site';
    import Icon from '../../icons/Icon.svelte';
    import { THUMB_SIZE } from './thumbnails.svelte';
    import { requestedMaskLink, type MaskLinkState } from './maskChain';

    let { mask, thumbnail, active, enabled, onselect, oncontextmenu, onupdate }: {
        mask: MaskLinkState;
        thumbnail: string;
        active: boolean;
        enabled: boolean;
        onselect: (event: MouseEvent) => void;
        oncontextmenu: (event: MouseEvent) => void;
        onupdate: () => void;
    } = $props();

    let pendingLinked = $state<boolean | null>(null);

    $effect(() => {
        if (pendingLinked === mask.linkedToHost) pendingLinked = null;
    });

    function toggle(event: MouseEvent) {
        event.stopPropagation();
        if (!app.engine || pendingLinked !== null) return;
        const desired = requestedMaskLink(mask);
        if (desired === null) return;
        pendingLinked = desired;
        app.engine.api.setMaskLinkedToHost({ id: mask.id, linked: desired });
        onupdate();
    }

    function selectMask(event: MouseEvent) {
        event.stopPropagation();
        onselect(event);
    }

    function openMaskMenu(event: MouseEvent) {
        event.stopPropagation();
        oncontextmenu(event);
    }
</script>

<div class="mask-chain-control">
    <button
        class="chain-btn"
        class:unlinked={!mask.linkedToHost}
        disabled={!mask.editable || pendingLinked !== null}
        onclick={toggle}
        title={mask.linkedToHost ? 'Unlink mask from layer transforms' : 'Link mask to layer transforms'}
        aria-label={mask.linkedToHost ? 'Unlink mask from layer transforms' : 'Link mask to layer transforms'}
    >
        <Icon name={mask.linkedToHost ? 'fa6-solid:link' : 'fa6-solid:link-slash'} />
    </button>

    {#if thumbnail}
        <button
            class="thumb-btn"
            class:thumb-active={active}
            class:mask-disabled={!enabled}
            type="button"
            aria-label="Edit mask"
            aria-pressed={active}
            onclick={selectMask}
            oncontextmenu={openMaskMenu}
        >
            <img
                class="thumb"
                src={thumbnail}
                alt=""
                width={THUMB_SIZE}
                height={THUMB_SIZE}
                draggable="false"
                use:bindingSite={{ name: 'maskThumb', ctx: () => ({ layerId: mask.id, maskId: mask.id }) }}
            />
        </button>
    {/if}
</div>

<style>
    .mask-chain-control {
        display: flex;
        align-items: center;
        gap: 2px;
        margin-left: -6px;
        flex-shrink: 0;
    }
    .chain-btn {
        width: 12px;
        height: 24px;
        padding: 0;
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
    .thumb-btn {
        width: 32px;
        height: 32px;
        padding: 0;
        border: 2px solid var(--text-dim);
        border-radius: 4px;
        flex-shrink: 0;
        cursor: pointer;
        overflow: hidden;
        background: var(--thumb-bg);
    }
    .thumb {
        display: block;
        width: 100%;
        height: 100%;
        image-rendering: pixelated;
    }
    .thumb-active { border-color: var(--accent); }
    .mask-disabled { opacity: 0.4; }
</style>
