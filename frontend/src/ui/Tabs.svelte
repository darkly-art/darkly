<script lang="ts" module>
    export interface Tab {
        id: string;
        label: string;
    }
</script>

<script lang="ts">
    /**
     * A strip of pane-switcher tabs. Controlled: the host owns the active id
     * and hears every selection, so it can do more than swap panes (the add
     * layer modal scrolls one list to the chosen section). `row` lays the
     * strip along the top with the accent under the active tab; `column` is a
     * left rail with the accent on its right edge.
     *
     * Arrow keys along the strip's axis move the selection and the focus, so
     * a keyboard user walks the tabs the way the tablist pattern expects.
     */
    let {
        tabs,
        active,
        onselect,
        orientation = 'row',
    }: {
        tabs: Tab[];
        active: string;
        onselect: (id: string) => void;
        orientation?: 'row' | 'column';
    } = $props();

    function onKeydown(e: KeyboardEvent, i: number) {
        const [prev, next] = orientation === 'row' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown'];
        let j: number;
        if (e.key === next) j = Math.min(i + 1, tabs.length - 1);
        else if (e.key === prev) j = Math.max(i - 1, 0);
        else return;
        e.preventDefault();
        if (j === i) return;
        onselect(tabs[j].id);
        const strip = (e.currentTarget as HTMLElement).parentElement;
        (strip?.children[j] as HTMLElement | undefined)?.focus();
    }
</script>

<div
    class="tab-strip"
    class:column={orientation === 'column'}
    role="tablist"
    aria-orientation={orientation === 'column' ? 'vertical' : 'horizontal'}
>
    {#each tabs as tab, i (tab.id)}
        <button
            type="button"
            class="tab"
            class:active={tab.id === active}
            role="tab"
            aria-selected={tab.id === active}
            tabindex={tab.id === active ? 0 : -1}
            onclick={() => onselect(tab.id)}
            onkeydown={(e) => onKeydown(e, i)}
        >{tab.label}</button>
    {/each}
</div>

<style>
    .tab-strip {
        display: flex;
        flex-direction: row;
        gap: 2px;
        flex-shrink: 0;
        border-bottom: 1px solid var(--bg-hover);
    }
    .tab-strip.column {
        flex-direction: column;
        padding: 8px 0;
        border-bottom: none;
        border-right: 1px solid var(--bg-hover);
        min-width: 140px;
    }
    .tab {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 13px;
        font-weight: 500;
        padding: 8px 16px;
        cursor: pointer;
        position: relative;
        border-radius: 0;
        text-align: left;
    }
    .tab:hover { color: var(--text); }
    .tab.active { color: var(--text); }
    .tab.active::after {
        content: '';
        position: absolute;
        background: var(--accent);
        left: 6px;
        right: 6px;
        bottom: -1px;
        height: 2px;
    }
    .column .tab.active::after {
        top: 6px;
        bottom: 6px;
        left: auto;
        right: -1px;
        width: 2px;
        height: auto;
    }
</style>
