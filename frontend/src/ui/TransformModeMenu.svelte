<script lang="ts">
    import { app } from '../state/app.svelte';
    import { focusedTransformTool } from '../tools/transform.svelte';
    import ContextMenu, { type ContextMenuItem } from './ContextMenu.svelte';

    // The transform tool opens the menu by setting `app.transformModeMenu` to
    // the right-click position; we render against it and clear it on close.
    let menu = $derived(app.transformModeMenu);

    // The mode queries are plain calls into the focused instance's transform
    // tool (not reactive state), so this derived would otherwise compute once
    // and freeze. Reading the reactive `menu` ties it to each menu open,
    // re-resolving the active mode (the checkmark) every time.
    //
    // Mode switches first, then the flips — the same grouping Krita's transform
    // tool uses (`kis_tool_transform.cc::popupActionsMenu`).
    let items = $derived.by<ContextMenuItem[]>(() => {
        void menu;
        const tool = focusedTransformTool();
        if (!tool) return [];
        const active = tool.activeModeTag();
        return [
            ...tool.availableModes().map((m) => ({
                label: m.label,
                checked: m.tag === active,
                onclick: () => tool.setMode(m.tag),
            })),
            { separator: true },
            { label: 'Flip Horizontally', onclick: () => tool.flip('h') },
            { label: 'Flip Vertically', onclick: () => tool.flip('v') },
        ];
    });
</script>

{#if menu}
    <ContextMenu
        x={menu.x}
        y={menu.y}
        {items}
        onclose={() => (app.transformModeMenu = null)}
    />
{/if}
