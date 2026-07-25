<!--
  Dev-only input-latency HUD. Reads the instrumentation the engine surfaces on
  `EngineState` (see `crates/darkly/src/engine/perf.rs` `InputLatencyMeter`):
  input→frame latency and the coalesced-sample fidelity counter. Hidden unless a
  DEV build or `?latency` is present in the URL, so it costs nothing in release.
-->
<script lang="ts">
    import { app } from '../state/app.svelte';

    const enabled =
        import.meta.env.DEV ||
        (typeof location !== 'undefined' &&
            new URLSearchParams(location.search).has('latency'));

    const state = $derived(app.engineState);
</script>

{#if enabled && state}
    <div class="latency-hud" aria-hidden="true">
        <span>tip {state.inputLatencyTipMs.toFixed(1)}ms</span>
        <span>worst {state.inputLatencyMs.toFixed(1)}ms</span>
        <span>samp/frame {state.strokeSamplesLastFrame}</span>
    </div>
{/if}

<style>
    .latency-hud {
        position: absolute;
        top: 8px;
        left: 8px;
        display: flex;
        gap: 12px;
        padding: 4px 8px;
        font-family: var(--font-mono, monospace);
        font-size: 11px;
        line-height: 1.4;
        color: #9fe;
        background: rgba(0, 0, 0, 0.6);
        border-radius: 4px;
        pointer-events: none;
        user-select: none;
        z-index: 10;
    }
</style>
