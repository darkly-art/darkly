<script lang="ts">
    import Modal from './Modal.svelte';
    import { selectionModify, type SelectionModifyOp } from '../state/selectionModify.svelte';
    import { app } from '../state/app.svelte';
    import type { EngineApi } from '../engine/protocol';

    // Per-op presentation + the typed engine call. One dialog serves all four
    // parameterized selection-modify commands.
    const OPS: Record<
        SelectionModifyOp,
        { title: string; call: (api: EngineApi, req: { radius: number }) => void; default: number }
    > = {
        grow: { title: 'Grow Selection', call: (api, req) => api.growSelection(req), default: 4 },
        shrink: { title: 'Shrink Selection', call: (api, req) => api.shrinkSelection(req), default: 4 },
        border: { title: 'Border Selection', call: (api, req) => api.borderSelection(req), default: 4 },
        feather: { title: 'Feather Selection', call: (api, req) => api.featherSelection(req), default: 6 },
    };

    const MAX_RADIUS = 512;
    let radius = $state(4);

    const meta = $derived(OPS[selectionModify.op]);

    let prevOpen = false;
    $effect(() => {
        if (selectionModify.open && !prevOpen) {
            radius = OPS[selectionModify.op].default;
        }
        prevOpen = selectionModify.open;
    });

    function clamp(v: number): number {
        if (!Number.isFinite(v) || v < 1) return 1;
        return Math.min(MAX_RADIUS, Math.round(v));
    }

    function close() {
        selectionModify.open = false;
    }

    function apply() {
        if (app.engine) meta.call(app.engine.api, { radius: clamp(radius) });
        app.requestFrame();
        close();
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter') {
            e.preventDefault();
            apply();
        }
    }
</script>

<Modal bind:open={selectionModify.open} title={meta.title} size="sm">
    <div class="body" onkeydown={onKeydown} role="presentation">
        <label class="field">
            <span class="field-label">Amount</span>
            <div class="field-num">
                <input type="number" min="1" max={MAX_RADIUS} step="1" bind:value={radius} />
                <span class="unit">px</span>
            </div>
        </label>

        <div class="dialog-actions">
            <button type="button" class="btn" onclick={close}>Cancel</button>
            <button type="button" class="btn primary" onclick={apply}>Apply</button>
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 280px;
    }
</style>
