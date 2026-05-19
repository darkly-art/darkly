<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry, type Tool, type ToolCluster } from '../tools/registry';
    import { config, formatHotkey } from '../config/store.svelte';

    interface Props { cluster: ToolCluster; }
    let { cluster }: Props = $props();

    let open = $state(false);
    let pinned = $state(false);
    let containerEl: HTMLDivElement | undefined = $state();

    const members = $derived(
        cluster.toolIds
            .map(id => toolRegistry.get(id))
            .filter((t): t is Tool => !!t)
    );

    const activeMember = $derived(
        members.find(t => t.id === app.activeToolId) ?? null
    );

    // The cluster button mirrors a single member tool's icon:
    //   • the currently-active member when one belongs to this cluster
    //   • otherwise the default member
    // The cluster never owns an icon of its own — it's pure routing.
    const iconSource = $derived(
        activeMember ?? toolRegistry.get(cluster.defaultToolId) ?? null
    );

    function onClusterClick() {
        const target = app.lastToolByCluster[cluster.id] ?? cluster.defaultToolId;
        app.activeToolId = target;
        pinned = !pinned;
        open = pinned;
    }

    function onClusterEnter() {
        open = true;
    }

    function onContainerLeave() {
        if (!pinned) open = false;
    }

    function pickTool(id: string) {
        app.activeToolId = id;
        open = false;
        pinned = false;
    }

    // Dismiss the pinned state when the user clicks outside this cluster.
    // Mirrors LeftSidebar's color-picker dismissal pattern.
    $effect(() => {
        if (!pinned) return;
        const onPointerDown = (e: PointerEvent) => {
            const t = e.target as Node | null;
            if (!t || !containerEl) return;
            if (containerEl.contains(t)) return;
            pinned = false;
            open = false;
        };
        window.addEventListener('pointerdown', onPointerDown, true);
        return () => window.removeEventListener('pointerdown', onPointerDown, true);
    });

    function toolTitle(t: Tool): string {
        const hk = formatHotkey(config.get(`hotkeys.${t.hotkeyAction}`) as string | undefined);
        const name = app.toolDisplayName(t.id);
        return hk ? `${name} (${hk})` : name;
    }

    const clusterTitle = $derived(
        activeMember ? toolTitle(activeMember) : cluster.displayName
    );
</script>

<div
    class="cluster"
    bind:this={containerEl}
    onmouseleave={onContainerLeave}
    role="presentation"
>
    <button
        class="tool cluster-btn"
        class:active={!!activeMember}
        onclick={onClusterClick}
        onmouseenter={onClusterEnter}
        title={clusterTitle}
    >
        {#if iconSource?.iconSvg}
            {@html iconSource.iconSvg}
        {:else if iconSource?.faIcon}
            <i class={iconSource.faIcon}></i>
        {/if}
    </button>

    <div
        class="popout"
        class:open
    >
        {#each members as tool}
            <button
                class="tool"
                class:active={app.activeToolId === tool.id}
                onclick={() => pickTool(tool.id)}
                title={toolTitle(tool)}
            >
                {#if tool.iconSvg}
                    {@html tool.iconSvg}
                {:else if tool.faIcon}
                    <i class={tool.faIcon}></i>
                {/if}
            </button>
        {/each}
    </div>
</div>

<style>
    .cluster {
        position: relative;
    }

    /* Vertical column of sub-tool buttons. Positioned flush against the
       toolbar's right edge — the 6px margin-left bridges from the cluster
       button (32px wide, centered in the 44px toolbar) to the toolbar's
       right edge, so the popout's left edge butts cleanly onto the sidebar
       with no visible gap. Vertically centered on the cluster button's
       center via `top: 50%; translateY(-50%)`. */
    .popout {
        position: absolute;
        top: 50%;
        left: 100%;
        margin-left: 6px;
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 6px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-left: none;
        border-radius: 0 6px 6px 0;
        box-shadow: 4px 4px 12px rgba(0, 0, 0, 0.3);
        z-index: 90;
        opacity: 0;
        transform: translate(-8px, -50%);
        pointer-events: none;
        transition: opacity 140ms ease-out, transform 140ms ease-out;
    }

    .popout.open {
        opacity: 1;
        transform: translate(0, -50%);
        pointer-events: auto;
    }

    /* Invisible hit-area bridge — extends the popout's pointer hit zone
       leftward by 6px to cover the toolbar's right padding between the
       cluster button and the popout. Without this, the pointer crosses
       "background of toolbar" mid-transit and fires `mouseleave` on the
       cluster container, closing a non-pinned popout. */
    .popout::before {
        content: '';
        position: absolute;
        left: -6px;
        top: 0;
        width: 6px;
        height: 100%;
    }

    /* Reuse .tool styling — duplicated here because Svelte scoped styles
       don't reach into this component. Kept in sync with LeftSidebar's. */
    .tool {
        width: 32px;
        height: 32px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 6px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 14px;
        transition: background 0.1s, color 0.1s;
        flex-shrink: 0;
    }

    /* Normalize inline SVG icons. Forces 1em sizing regardless of the
       source <svg>'s width/height attributes, and sets `fill: currentColor`
       so SVGs downloaded from icon sets (Font Awesome, Boxicons, etc.)
       inherit the toolbar's text color exactly like the webfont icons do.
       Without this, raw FA SVG downloads default to black because their
       paths have no explicit fill. Descendant paths inherit fill from
       the <svg> element, so per-element fills in fancier SVGs still win. */
    .tool :global(svg) {
        width: 1em;
        height: 1em;
        fill: currentColor;
    }

    .tool:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .tool.active {
        background: var(--accent);
        color: #ffffff;
    }
</style>
