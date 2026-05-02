<script lang="ts">
    let { onpick, onclose }: {
        onpick: (kind: 'layer' | 'group' | 'veil') => void;
        onclose: () => void;
    } = $props();

    function onWindowClick(e: MouseEvent) {
        const target = e.target as HTMLElement;
        if (!target.closest('.new-layer-menu') && !target.closest('.new-layer-trigger')) {
            onclose();
        }
    }

    function onKeyDown(e: KeyboardEvent) {
        if (e.key === 'Escape') onclose();
    }
</script>

<svelte:window onclick={onWindowClick} onkeydown={onKeyDown} />

<div class="new-layer-menu" role="menu">
    <button class="item" role="menuitem" onclick={() => onpick('layer')}>
        <i class="fa-solid fa-image"></i>
        <span>Normal Layer</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('group')}>
        <i class="fa-solid fa-folder"></i>
        <span>Layer Group</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('veil')}>
        <i class="fa-solid fa-wand-magic-sparkles"></i>
        <span>Veil</span>
    </button>
</div>

<style>
    .new-layer-menu {
        position: absolute;
        top: 100%;
        left: 0;
        margin-top: 4px;
        z-index: 100;
        min-width: 160px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
    }

    .item {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        padding: 6px 12px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 12px;
        text-align: left;
        cursor: pointer;
        transition: background var(--transition-fast);
    }

    .item:hover {
        background: var(--bg-hover);
    }

    .item i {
        width: 14px;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }

    .item:hover i {
        color: var(--accent);
    }
</style>
