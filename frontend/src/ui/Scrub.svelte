<script lang="ts">
    import { exposedDragSpeed } from '../state/brush_graph.svelte';
    import { app } from '../state/app.svelte';
    import { beginScrubDrag } from '../lib/scrubDrag';
    import Icon from '../icons/Icon.svelte';

    type DragProps = {
        mode: 'drag';
        icon?: string;
        label: string;
        value: number;
        min: number;
        max: number;
        default: number;
        formatValue?: (v: number) => string;
        /** Apply the value under the pointer. Fires on every move, so it must
         *  be cheap: local or session state, not an engine round-trip. */
        onChange: (v: number) => void;
        /** The value the user settled on, once per gesture. For consumers
         *  whose real work is too expensive to repeat mid-drag; omit it when
         *  `onChange` is already the whole story. */
        onCommit?: (v: number) => void;
        title?: string;
    };
    type ToggleProps = {
        mode: 'toggle';
        icon?: string;
        label: string;
        valueLabel: string;
        active: boolean;
        onToggle: () => void;
        title?: string;
    };

    let props: DragProps | ToggleProps = $props();

    let dragging = $state(false);

    const DEFAULT_ICON = 'fa6-solid:sliders';
    const DEFAULT_FORMAT = (v: number) => v.toFixed(2);

    const valueText = $derived(
        props.mode === 'drag'
            ? (props.formatValue ?? DEFAULT_FORMAT)(props.value)
            : props.valueLabel,
    );

    function startDrag(e: PointerEvent) {
        if (props.mode !== 'drag') return;
        e.preventDefault();
        const { min, max, onChange, onCommit } = props;
        const startX = e.clientX;
        const startVal = props.value;
        const speed = exposedDragSpeed(min, max);
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        dragging = true;
        app.beginInteraction();
        const drag = beginScrubDrag({
            toValue: (clientX) =>
                Math.min(max, Math.max(min, startVal + (clientX - startX) * speed)),
            onPreview: onChange,
            onCommit: (v) => onCommit?.(v),
            onFinish: () => {
                dragging = false;
                app.endInteraction();
                el.removeEventListener('pointermove', onMove);
                el.removeEventListener('pointerup', onEnd);
                el.removeEventListener('lostpointercapture', onEnd);
            },
        });
        const onMove = (ev: PointerEvent) => drag.move(ev.clientX, ev.clientY);
        const onEnd = () => drag.end();
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onEnd);
        el.addEventListener('lostpointercapture', onEnd);
    }

    /** Double-click restores the default: one discrete change, so it previews
     *  and commits in the same step. */
    function resetDefault() {
        if (props.mode !== 'drag') return;
        props.onChange(props.default);
        props.onCommit?.(props.default);
    }
</script>

{#snippet body()}
    <Icon name={props.icon ?? DEFAULT_ICON} class="scrub-icon" />
    <div class="bar-control-text">
        <span class="bar-control-label">{props.label}</span>
        <span class="bar-control-value">{valueText}</span>
    </div>
{/snippet}

{#if props.mode === 'drag'}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="scrub bar-control"
        class:dragging
        title={props.title}
        onpointerdown={startDrag}
        ondblclick={resetDefault}
    >
        {@render body()}
    </div>
{:else}
    <button
        type="button"
        class="scrub bar-control toggle"
        class:on={props.active}
        title={props.title}
        onclick={props.onToggle}
    >
        {@render body()}
    </button>
{/if}

<style>
    /* Base look comes from the shared `.bar-control`; the rules here are
       scrub-specific behavior: drag cursor, the accent active/dragging
       state, tabular value digits, and the button reset. */
    .scrub {
        flex-shrink: 0;
        cursor: col-resize;
    }

    .scrub.dragging,
    .scrub.on {
        background: var(--accent);
    }

    .scrub.dragging :global(.scrub-icon),
    .scrub.dragging .bar-control-label,
    .scrub.dragging .bar-control-value,
    .scrub.on :global(.scrub-icon),
    .scrub.on .bar-control-label,
    .scrub.on .bar-control-value {
        color: #ffffff;
    }

    .toggle {
        border: none;
        font: inherit;
        cursor: pointer;
    }

    .scrub :global(.scrub-icon) {
        font-size: 14px;
        color: var(--text-muted);
    }

    /* Values are numeric; keep digits from shifting width while scrubbing. */
    .scrub .bar-control-value {
        font-variant-numeric: tabular-nums;
    }
</style>
