<script lang="ts">
    import { getContext } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { toolRegistry, type ToolDescriptor, type ToolCluster } from '../../tools/registry';
    import Icon from '../../icons/Icon.svelte';
    import { TOOL_STRIP_OUT, type ToolStripOut } from './context';

    interface Props { cluster: ToolCluster; }
    let { cluster }: Props = $props();

    let open = $state(false);
    let pinned = $state(false);
    let containerEl: HTMLDivElement | undefined = $state();

    const members = $derived(
        cluster.toolIds
            .map(id => toolRegistry.get(id))
            .filter((t): t is ToolDescriptor => !!t)
    );

    const activeMember = $derived(
        members.find(t => t.id === app.activeToolId) ?? null
    );

    // The cluster button mirrors a single member tool's icon:
    //   • the currently-active member when one belongs to this cluster
    //   • otherwise the default member
    // The cluster never owns an icon of its own; it's pure routing.
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
        if (settling) return;
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

    // Dismiss the pinned state when the artist clicks outside this cluster.
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

    // A flyout left open when the strip tucks would hang in space over the
    // canvas, anchored to a button that is no longer there.
    const strip = getContext<ToolStripOut | undefined>(TOOL_STRIP_OUT);

    /** Set for one frame after the strip slides out. The 160ms slide can drag a
     *  cluster button under a motionless pointer, and browsers re-evaluate hover
     *  when a transform moves what is underneath it, so `onmouseenter` would
     *  fire and open a flyout the artist never asked for. */
    let settling = $state(false);

    $effect(() => {
        const isOut = strip?.out ?? true;
        if (!isOut) {
            open = false;
            pinned = false;
            return;
        }
        settling = true;
        const id = requestAnimationFrame(() => { settling = false; });
        return () => cancelAnimationFrame(id);
    });

    const clusterTitle = $derived(
        activeMember ? app.toolTooltip(activeMember.id) : cluster.displayName
    );
</script>

<div
    class="cluster"
    bind:this={containerEl}
    onmouseleave={onContainerLeave}
    role="presentation"
>
    <button
        class="icon-btn square tool"
        class:active={!!activeMember}
        onclick={onClusterClick}
        onmouseenter={onClusterEnter}
        title={clusterTitle}
    >
        {#if iconSource}
            <Icon name={app.toolGlyph(iconSource.id)} />
        {/if}
    </button>

    <div
        class="popout"
        class:open
    >
        {#each members as tool}
            <button
                class="icon-btn square tool"
                class:active={app.activeToolId === tool.id}
                onclick={() => pickTool(tool.id)}
                title={app.toolTooltip(tool.id)}
            >
                <Icon name={app.toolGlyph(tool.id)} />
            </button>
        {/each}
    </div>
</div>

<style>
    .cluster {
        position: relative;
    }

    /* The flyout's geometry (which way it opens, its rounding, its shadow, the
       invisible hit bridge that keeps it from closing mid-transit) lives in
       `styles/tool-strip-dock.css`, read from custom properties the strip sets.
       This component has no edge prop and no conditionals: it never learns which
       way it is facing. Only its own box and state styling are here. */
    .popout {
        min-width: 0;
        min-height: 0;
    }

    /* Box and reset come from `.icon-btn.square` in tokens.css; only the
       flyout's own needs live here. */
    .tool {
        flex-shrink: 0;
    }

    /* `fill: currentColor` so SVGs downloaded from icon sets (Font Awesome,
       Boxicons, etc.) inherit the button's text color exactly like the webfont
       icons do. Without it, raw FA SVG downloads render black because their
       paths have no explicit fill. Descendant paths inherit fill from the
       <svg> element, so per-element fills in fancier SVGs still win. */
    .tool :global(svg) {
        fill: currentColor;
    }

    .tool.active {
        background: var(--accent);
        color: #ffffff;
    }
</style>
