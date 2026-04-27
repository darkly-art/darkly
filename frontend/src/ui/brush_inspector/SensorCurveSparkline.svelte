<script lang="ts">
    interface Props {
        points: [number, number][];
        size?: number;
    }
    let { points, size = 48 }: Props = $props();

    // Krita curves are domain [0,1] x range [0,1]. Pad by a pixel so the
    // stroke isn't clipped by the SVG edge.
    const pad = 2;
    const inner = $derived(size - pad * 2);

    const pathD = $derived.by(() => {
        if (points.length === 0) return '';
        return points
            .map(([x, y], i) => {
                const px = pad + x * inner;
                const py = pad + (1 - y) * inner;
                return `${i === 0 ? 'M' : 'L'}${px.toFixed(2)},${py.toFixed(2)}`;
            })
            .join(' ');
    });
</script>

<svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} class="sparkline">
    <rect x="0.5" y="0.5" width={size - 1} height={size - 1} class="frame" />
    <path d={pathD} class="curve" />
    {#each points as [x, y] (x + ',' + y)}
        <circle
            cx={pad + x * inner}
            cy={pad + (1 - y) * inner}
            r="1.5"
            class="point"
        />
    {/each}
</svg>

<style>
    .sparkline {
        display: block;
    }
    .frame {
        fill: var(--bg);
        stroke: var(--text-dim);
        stroke-width: 1;
    }
    .curve {
        fill: none;
        stroke: var(--accent);
        stroke-width: 1.5;
    }
    .point {
        fill: var(--accent);
    }
</style>
