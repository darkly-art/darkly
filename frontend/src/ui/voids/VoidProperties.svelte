<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { catalogs } from '../../state/catalogs.svelte';
    import Icon from '../../icons/Icon.svelte';
    import ParamRow from '../params/ParamRow.svelte';
    import type { ParamInfo, ParamValue } from '../params/paramSchema';

    // A void's params are `ParamInfo[]` on the wire, the same type a filter
    // carries, so they render through the same row.
    let { node }: {
        node: { id: number; voidType: string; params: ParamInfo[] };
    } = $props();

    function pushParams() {
        if (!app.engine) return;
        // `ParamValue`, not a narrowed scalar union: a void's params go through
        // the shared row now, so a color or vec2 param is renderable and has to
        // survive the push.
        const params: Record<string, ParamValue> = {};
        for (const p of node.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.engine.api.setVoidParams({ id: node.id, params });
        app.refreshLayerTree();
        app.requestFrame();
    }

    function randomizeSeed() {
        const seedParam = node.params.find((p) => p.name === 'seed');
        if (!seedParam) return;
        seedParam.value = Math.floor(Math.random() * 1_000_000);
        pushParams();
    }

    const voidLabel = $derived(catalogs.displayName('voids', node.voidType));

    // Capture kind (camera / screenshare / Blender stream) for this void, or
    // undefined for procedural voids: the single signal that gates every
    // stream-related affordance below.
    const captureKind = $derived(catalogs.voidCaptureKind.get(node.voidType));

    // Stream-backed voids surface source-level errors here so the artist sees a
    // human-readable reason ("Camera access was denied", "Could not connect to
    // the Blender stream…", …) instead of a silently-transparent layer.
    const streamError = $derived(
        captureKind ? app.streamSourceFor(node.id)?.error ?? null : null,
    );

    // Connection status for the same source, or null when none exists. A
    // never-connected void (e.g. loaded from a `.darkly`) shows no status row;
    // the Connect button already communicates that state. A change-driven
    // stream sends nothing while the scene is idle, so "Connected" is the only
    // positive signal that the feed is actually alive.
    const streamStatus = $derived(
        captureKind ? app.streamSourceFor(node.id)?.status ?? null : null,
    );

    const statusLabels = {
        connecting: 'Connecting…',
        connected: 'Connected',
        disconnected: 'Disconnected',
    } as const;

    // True for a stream-backed void whose layer exists but isn't currently
    // streaming, either loaded from a `.darkly` (showing the saved last frame)
    // or stopped externally (the browser's "Stop sharing" bar, a Blender
    // disconnect). The button lets the artist explicitly (re)connect. The session
    // opt-in is cleared on external stop, so this re-appears then too.
    const showResume = $derived(
        !!captureKind
            && !isFrozen(node.params)
            && !app.streamSessionStarted.has(node.id),
    );

    // Per-kind verb for the (re)connect button.
    const resumeLabel = $derived(
        captureKind === 'stream'
            ? 'Connect to Blender'
            : captureKind === 'display'
              ? 'Resume screen share'
              : 'Resume camera',
    );

    function isFrozen(params: ParamInfo[]): boolean {
        const f = params.find((p) => p.name === 'freeze');
        return (f?.value ?? f?.default) === true;
    }

    function resumeStream() {
        if (!captureKind) return;
        // For camera / screenshare this is a user gesture: acquire + start
        // in-gesture so getDisplayMedia's activation requirement holds. For a
        // Blender `stream` void there's no permission gate; `startStreamSource`
        // connects over localhost HTTP immediately.
        app.markStreamVoidStarted(node.id);
        app.startStreamSource(node.id, captureKind);
    }
</script>

<div class="header">
    <span class="type-label">{voidLabel}</span>
    <button
        class="randomize-btn"
        onclick={randomizeSeed}
        title="Randomize seed"
        disabled={!node.params.some((p) => p.name === 'seed')}
    >
        <Icon name="fa6-solid:dice" />
    </button>
</div>

{#if streamStatus !== null}
    <div class="status-row">
        <span class="status-dot {streamStatus}"></span>
        <span>{statusLabels[streamStatus]}</span>
    </div>
{/if}

{#if streamError}
    <div class="notice">
        <Icon name="fa6-solid:triangle-exclamation" />
        <span>{streamError}</span>
    </div>
{/if}

{#if showResume}
    <button class="resume-btn" onclick={resumeStream}>
        <Icon name="fa6-solid:video" />
        <span>{resumeLabel}</span>
    </button>
{/if}

{#if node.params.length === 0}
    <div class="empty">No parameters</div>
{:else}
    {#each node.params as param (param.name)}
        <ParamRow {param} oninput={pushParams} onchange={pushParams} />
    {/each}
{/if}

<style>
    .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding-bottom: 4px;
        border-bottom: 1px solid var(--bg-hover);
        margin-bottom: 2px;
    }

    .type-label {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
    }

    .randomize-btn {
        width: 22px;
        height: 22px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
    }
    .randomize-btn:hover:not(:disabled) {
        background: var(--bg-hover);
        color: var(--accent);
    }
    .randomize-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .empty {
        font-size: 12px;
        color: var(--text-dim);
        text-align: center;
        padding: 4px 0;
    }

    .status-row {
        display: flex;
        align-items: center;
        gap: 6px;
        min-height: 22px;
        font-size: 11px;
        color: var(--text-muted);
    }

    .status-dot {
        width: 8px;
        height: 8px;
        border-radius: 50%;
        flex-shrink: 0;
    }
    .status-dot.connecting {
        background: var(--warning, #d9a23c);
    }
    .status-dot.connected {
        background: var(--success, #4caf7d);
    }
    .status-dot.disconnected {
        background: var(--danger, #e35858);
    }

    .notice {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 8px;
        margin: 4px 0;
        background: color-mix(in srgb, var(--accent) 12%, transparent);
        border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
        border-radius: var(--radius-sm);
        font-size: 11px;
        color: var(--text);
    }

    .resume-btn {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 100%;
        padding: 6px 8px;
        margin: 4px 0;
        background: var(--bg-hover);
        border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 11px;
        cursor: pointer;
        justify-content: center;
    }
    .resume-btn:hover {
        background: var(--bg-active);
        border-color: var(--accent);
    }
</style>
