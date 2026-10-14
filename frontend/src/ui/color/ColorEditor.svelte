<script lang="ts">
    /**
     * The color editor: the one widget behind every color pick, hosted by the
     * docked Color panel and by the floating popup (the fg/bg swatches, the
     * palette popup's spectrum leaf, and every `ColorInput`). Hosts own what
     * `value` is bound to and where the editor sits; the editor owns the tabs
     * and everything inside them.
     *
     * Tabs are data: each entry names its pane, and a tab about how strokes
     * use paint (Jitter) declares `paint`. A host editing one of the paint
     * colors sets `paint`; a generic color field leaves it off and gets the
     * wheel alone, with no tab strip. A new color tab is one more entry.
     */
    import type { Snippet } from 'svelte';
    import type { Color } from '../../lib/color';
    import Tabs, { type Tab } from '../Tabs.svelte';
    import ColorWheel from './ColorWheel.svelte';
    import HexField from './HexField.svelte';
    import JitterControls from './JitterControls.svelte';

    let {
        value,
        oninput,
        onchange,
        size,
        paint = false,
    }: {
        value: Color;
        oninput: (c: Color) => void;
        onchange: (c: Color) => void;
        /** Wheel diameter in px, and the editor's width on every tab. */
        size: number;
        /** The host edits a paint color, so tabs about painting apply. */
        paint?: boolean;
    } = $props();

    interface ColorTab extends Tab {
        pane: Snippet;
        paint?: boolean;
    }

    const TABS: ColorTab[] = [
        { id: 'wheel', label: 'Wheel', pane: wheelPane },
        { id: 'jitter', label: 'Jitter', pane: jitterPane, paint: true },
    ];

    let tabs = $derived(TABS.filter((t) => paint || !t.paint));
    let tab = $state('wheel');
    let current = $derived(tabs.find((t) => t.id === tab) ?? tabs[0]);
</script>

{#snippet wheelPane()}
    <ColorWheel {value} {oninput} {onchange} {size} />
    <HexField {value} {onchange} />
{/snippet}

{#snippet jitterPane()}
    <JitterControls />
{/snippet}

<div class="color-editor" style:width="{size}px">
    {#if tabs.length > 1}
        <Tabs {tabs} active={current.id} onselect={(id) => (tab = id)} />
    {/if}
    {@render current.pane()}
</div>

<style>
    .color-editor {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
</style>
