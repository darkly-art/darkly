<script lang="ts">
    import { app } from '../state/app.svelte';
    import { actions } from '../actions/registry';
    import { focusedTransformTool } from '../tools/transform.svelte';
    import ContextMenu, { type ContextMenuItem } from './ContextMenu.svelte';

    // The transform tool opens the menu by setting `app.transformModeMenu` to
    // the right-click position; we render against it and clear it on close.
    let menu = $derived(app.transformModeMenu);

    // Whether the thing under the gizmo is floating content that could become a
    // smart object. Only the engine knows — a destructive transform session and
    // a mask float both look like "the gizmo is up" from here, and neither
    // qualifies.
    //
    // Resolved asynchronously *after* the menu opens rather than gating the
    // open on a round trip: the menu must appear on the click, and a hook whose
    // session can die mid-await is the wrong place to block. The entry settles
    // a frame later, which is invisible in practice and honest when it isn't.
    let convertible = $state(false);
    $effect(() => {
        const open = menu;
        const engine = app.engine;
        if (!open || !engine) {
            convertible = false;
            return;
        }
        let live = true;
        void engine.api
            .canConvertFloatingToSmartObject()
            .then((ok) => {
                if (live) convertible = ok;
            })
            .catch((e) => {
                // Logged, not swallowed: a failure here makes the entry
                // silently absent, which is indistinguishable from "not
                // convertible" and leaves nothing to debug.
                console.error('[transform-menu] convertibility query failed', e);
                if (live) convertible = false;
            });
        return () => {
            live = false;
        };
    });

    // The mode queries are plain calls into the focused instance's transform
    // tool (not reactive state), so this derived would otherwise compute once
    // and freeze. Reading the reactive `menu` ties it to each menu open,
    // re-resolving the active mode (the checkmark) every time.
    //
    // Mode switches first, then the flips — the same grouping Krita's transform
    // tool uses (`kis_tool_transform.cc::popupActionsMenu`).
    let items = $derived.by<ContextMenuItem[]>(() => {
        void menu;
        void convertible;
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
            // Only for convertible floating content, and last: it ends the
            // transform rather than adjusting it, so it does not belong among
            // the modes and flips.
            ...(convertible
                ? [
                      { separator: true } as ContextMenuItem,
                      {
                          label: 'Convert to Smart Object',
                          onclick: () => actions.dispatch('convertFloatingToSmartObject'),
                      } as ContextMenuItem,
                  ]
                : []),
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
