<script lang="ts">
    import { sampleCurve, evaluateCurve } from '../lib/curve_math';

    type Point = [number, number];

    interface Props {
        points: Point[];
        onchange: (points: Point[]) => void;
        oninput?: (points: Point[]) => void;
        pinEndpoints?: boolean;
        gridDivisions?: number;
    }

    let {
        points,
        onchange,
        oninput,
        pinEndpoints = true,
        gridDivisions = 4,
    }: Props = $props();

    const PAD = 6; // padding in px so edge points aren't clipped
    const POINT_R = 4;
    const POINT_R_HOVER = 5;

    let svgEl: SVGSVGElement;
    let w = $state(0);
    let h = $state(0);
    let selectedIndex: number | null = $state(null);
    let draggingIndex: number | null = $state(null);
    let localPoints: Point[] | null = $state(null);

    // Use local points during drag, otherwise use prop points.
    let activePoints = $derived(localPoints ?? points);

    // Inner drawing area (excluding padding).
    let iw = $derived(Math.max(1, w - PAD * 2));
    let ih = $derived(Math.max(1, h - PAD * 2));

    // Convert normalized [0,1] to pixel coordinates.
    function toPixel(nx: number, ny: number): [number, number] {
        return [PAD + nx * iw, PAD + (1 - ny) * ih];
    }

    // Convert pixel coordinates to normalized [0,1].
    function toNorm(px: number, py: number): Point {
        const nx = Math.max(0, Math.min(1, (px - PAD) / iw));
        const ny = Math.max(0, Math.min(1, 1 - (py - PAD) / ih));
        return [nx, ny];
    }

    // Sample the curve for the polyline path.
    let curvePath = $derived(() => {
        const samples = sampleCurve(activePoints, 64);
        return samples.map(([x, y]) => {
            const [px, py] = toPixel(x, y);
            return `${px},${py}`;
        }).join(' ');
    });

    // Grid lines in pixel coords.
    let gridLines = $derived(() => {
        const lines: { x1: number; y1: number; x2: number; y2: number }[] = [];
        for (let i = 1; i < gridDivisions; i++) {
            const v = i / gridDivisions;
            const [vx] = toPixel(v, 0);
            const [, vy] = toPixel(0, v);
            lines.push({ x1: vx, y1: PAD, x2: vx, y2: PAD + ih }); // vertical
            lines.push({ x1: PAD, y1: vy, x2: PAD + iw, y2: vy }); // horizontal
        }
        return lines;
    });

    // Identity diagonal in pixel coords.
    let identityLine = $derived(() => {
        const [x1, y1] = toPixel(0, 0);
        const [x2, y2] = toPixel(1, 1);
        return { x1, y1, x2, y2 };
    });

    function svgToNorm(e: PointerEvent | MouseEvent): Point {
        const rect = svgEl.getBoundingClientRect();
        const px = e.clientX - rect.left;
        const py = e.clientY - rect.top;
        return toNorm(px, py);
    }

    function onPointDown(e: PointerEvent, index: number) {
        e.stopPropagation();
        e.preventDefault();
        selectedIndex = index;
        draggingIndex = index;
        localPoints = [...activePoints.map(p => [...p] as Point)];
        svgEl.setPointerCapture(e.pointerId);
    }

    function onSvgPointerMove(e: PointerEvent) {
        if (draggingIndex === null || localPoints === null) return;
        const [nx, ny] = svgToNorm(e);

        const pts = localPoints.map(p => [...p] as Point);
        const i = draggingIndex;
        const isFirst = i === 0;
        const isLast = i === pts.length - 1;

        if (pinEndpoints && (isFirst || isLast)) {
            pts[i] = [pts[i][0], ny];
        } else {
            const minX = i > 0 ? pts[i - 1][0] + 0.005 : 0;
            const maxX = i < pts.length - 1 ? pts[i + 1][0] - 0.005 : 1;
            pts[i] = [Math.max(minX, Math.min(maxX, nx)), ny];
        }

        localPoints = pts;
        oninput?.(pts);
    }

    function onSvgPointerUp(e: PointerEvent) {
        if (draggingIndex !== null && localPoints !== null) {
            svgEl.releasePointerCapture(e.pointerId);
            const pts = localPoints;
            draggingIndex = null;
            localPoints = null;
            onchange(pts);
        }
    }

    function onSvgPointerDown(e: PointerEvent) {
        const target = e.target as Element;
        if (target.classList.contains('curve-point')) return;

        // Spawn a new point on the curve and immediately start dragging it.
        const [nx, _ny] = svgToNorm(e);
        const pts = [...activePoints.map(p => [...p] as Point)];
        let insertIdx = pts.findIndex(p => p[0] > nx);
        if (insertIdx === -1) insertIdx = pts.length;
        if (pinEndpoints) {
            if (insertIdx === 0) insertIdx = 1;
        }

        const curveY = evaluateCurve(activePoints, nx);
        pts.splice(insertIdx, 0, [nx, curveY]);

        selectedIndex = insertIdx;
        draggingIndex = insertIdx;
        localPoints = pts;
        svgEl.setPointerCapture(e.pointerId);
        oninput?.(pts);
    }

    function onPointDblClick(e: MouseEvent, index: number) {
        e.stopPropagation();
        e.preventDefault();
        deletePoint(index);
    }

    function deletePoint(index: number) {
        if (activePoints.length <= 2) return;
        if (pinEndpoints && (index === 0 || index === activePoints.length - 1)) return;
        const pts = activePoints.filter((_, i) => i !== index);
        selectedIndex = null;
        onchange(pts);
    }

    function onKeyDown(e: KeyboardEvent) {
        if (selectedIndex === null) return;
        if (e.key === 'Delete' || e.key === 'Backspace') {
            e.preventDefault();
            e.stopPropagation();
            deletePoint(selectedIndex);
        }
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
    class="curve-editor"
    onkeydown={onKeyDown}
    tabindex="0"
    role="application"
    aria-label="Curve editor"
    bind:clientWidth={w}
    bind:clientHeight={h}
>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <svg
        bind:this={svgEl}
        onpointerdown={onSvgPointerDown}
        onpointermove={onSvgPointerMove}
        onpointerup={onSvgPointerUp}
    >
        <!-- Grid -->
        {#each gridLines() as line}
            <line
                x1={line.x1} y1={line.y1}
                x2={line.x2} y2={line.y2}
                class="grid-line"
            />
        {/each}

        <!-- Identity diagonal -->
        <line
            x1={toPixel(0, 0)[0]} y1={toPixel(0, 0)[1]}
            x2={toPixel(1, 1)[0]} y2={toPixel(1, 1)[1]}
            class="identity-line"
        />

        <!-- Curve path -->
        <polyline points={curvePath()} class="curve-line" />

        <!-- Control points -->
        {#each activePoints as point, i}
            {@const [cx, cy] = toPixel(point[0], point[1])}
            <circle
                {cx} {cy}
                r={selectedIndex === i || draggingIndex === i ? POINT_R_HOVER : POINT_R}
                class="curve-point"
                class:selected={selectedIndex === i}
                class:dragging={draggingIndex === i}
                onpointerdown={(e) => onPointDown(e, i)}
                ondblclick={(e) => onPointDblClick(e, i)}
            />
        {/each}
    </svg>
</div>

<style>
    .curve-editor {
        position: relative;
        width: 128px;
        height: 128px;
        margin: 0 auto;
        background: color-mix(in srgb, var(--bg) 80%, black);
        border-radius: 3px;
        overflow: hidden;
        outline: none;
        cursor: crosshair;
    }
    .curve-editor:focus-visible {
        outline: 1px solid var(--accent);
    }
    svg {
        display: block;
        width: 100%;
        height: 100%;
    }
    .grid-line {
        stroke: color-mix(in srgb, var(--text) 8%, transparent);
        stroke-width: 1;
    }
    .identity-line {
        stroke: color-mix(in srgb, var(--text) 15%, transparent);
        stroke-width: 1;
        stroke-dasharray: 3 3;
    }
    .curve-line {
        fill: none;
        stroke: var(--accent);
        stroke-width: 1.5;
    }
    .curve-point {
        fill: var(--accent);
        stroke: var(--bg);
        stroke-width: 1.5;
        cursor: grab;
    }
    .curve-point:hover {
        fill: color-mix(in srgb, var(--accent) 80%, white);
    }
    .curve-point.selected {
        fill: white;
        stroke: var(--accent);
        stroke-width: 2;
    }
    .curve-point.dragging {
        cursor: grabbing;
    }
</style>
