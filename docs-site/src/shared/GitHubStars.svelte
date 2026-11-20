<script lang="ts">
    /*
     * Stargazer count for the header's GitHub link.
     *
     * Client-side on purpose. The site builds to static files, so a build-time
     * fetch would freeze the number at whatever it was when CI last ran; this
     * way it is current on every visit, and costs nothing at build time.
     *
     * The count ramps from zero rather than appearing, which is the whole point
     * of putting it there.
     */
    import { onMount } from 'svelte';

    const ENDPOINT = 'https://api.github.com/repos/darkly-art/darkly';
    // Every page here is a full document load, so without a cache a visitor
    // reading five doc pages spends five calls against the unauthenticated
    // rate limit (60/hour/IP). An hour is far finer than the number moves.
    const CACHE_KEY = 'darkly-stars';
    const CACHE_TTL = 60 * 60 * 1000;
    const DURATION = 1100;

    let target = $state<number | null>(null);
    let shown = $state(0);

    const format = (n: number) => n.toLocaleString('en-US');

    function readCache(): { count: number; fresh: boolean } | null {
        try {
            const raw = localStorage.getItem(CACHE_KEY);
            if (!raw) return null;
            const { count, at } = JSON.parse(raw);
            if (typeof count !== 'number' || typeof at !== 'number') return null;
            return { count, fresh: Date.now() - at < CACHE_TTL };
        } catch {
            return null;
        }
    }

    function writeCache(count: number) {
        try {
            localStorage.setItem(CACHE_KEY, JSON.stringify({ count, at: Date.now() }));
        } catch {}
    }

    let frame = 0;

    function rampTo(count: number) {
        cancelAnimationFrame(frame);
        target = count;

        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        // Also skip the ramp when the number barely moved: a cached 325 being
        // corrected to 326 should tick over, not replay the whole flourish.
        if (reduced || Math.abs(count - shown) <= 2) {
            shown = count;
            return;
        }

        const from = shown;
        const start = performance.now();
        const step = (now: number) => {
            const t = Math.min((now - start) / DURATION, 1);
            const eased = 1 - Math.pow(1 - t, 3);
            shown = Math.round(from + (count - from) * eased);
            if (t < 1) frame = requestAnimationFrame(step);
        };
        frame = requestAnimationFrame(step);
    }

    onMount(() => {
        const cached = readCache();
        // A cached value is on screen before first paint's worth of network, so
        // the ramp starts immediately and the header stops reflowing a second
        // later. A stale one still ramps; it is just corrected afterwards.
        if (cached) rampTo(cached.count);

        if (!cached?.fresh) {
            fetch(ENDPOINT, { headers: { Accept: 'application/vnd.github+json' } })
                .then((res) => (res.ok ? res.json() : null))
                .then((data) => {
                    const count = data?.stargazers_count;
                    if (typeof count !== 'number') return;
                    writeCache(count);
                    if (count !== target) rampTo(count);
                })
                // Rate limited, offline, blocked: the link still works, and a
                // missing count is better than a zero or an error in the nav.
                .catch(() => {});
        }

        return () => cancelAnimationFrame(frame);
    });
</script>

{#if target !== null}
    <span class="stars">
        <!-- The invisible copy of the final value is what sets the width, so the
             box is the size it will end at from the first frame and neither the
             GitHub mark before it nor the star after it moves while the digits
             climb. -->
        <span class="count">
            <span class="sizer" aria-hidden="true">{format(target)}</span>
            <span class="live">{format(shown)}</span>
        </span>
        <!--
          Lucide's `star`, inlined.

          Not `<i class="fa-regular fa-star">` like the GitHub and Discord marks
          beside it, for two reasons. Font Awesome's outline star is a filled
          path cut to look hollow, so there is nothing to fill on hover; Lucide's
          is a stroked outline, and filling it is one property. And an icon font
          reserves no width until it loads, which is the one thing the sizer
          above exists to prevent.

          Stroke lives on the <svg> rather than on the path so `fill` inherits
          down; a `fill="none"` presentation attribute on the path would sit
          between the CSS below and the shape it is trying to fill.
        -->
        <svg
            class="star"
            viewBox="0 0 24 24"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path
                d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.12 2.12 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.12 2.12 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.12 2.12 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.12 2.12 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.12 2.12 0 0 0 1.597-1.16z"
            />
        </svg>
    </span>
{/if}

<style>
    /* `--stars-display: none` hides the count, for a header too narrow for
       it; the link keeps its icon. */
    .stars {
        display: var(--stars-display, inline-flex);
        align-items: center;
        gap: 0.3em;
        font-size: 0.68em;
        font-weight: 400;
        font-variant-numeric: tabular-nums;
        line-height: 1;
        color: inherit;
        animation: stars-in 0.35s ease-out both;
    }
    .count {
        display: inline-grid;
    }
    .sizer,
    .live {
        grid-area: 1 / 1;
        justify-self: start;
    }
    .sizer {
        visibility: hidden;
    }

    /*
     * Sized against the digits, not the nav: `.stars` already shrank to 0.68em,
     * so 1.2em of that keeps the star a little taller than the numerals, which
     * is the proportion the outline reads at.
     *
     * Hollow at rest and filled on hover, because the link is the thing you
     * click to add one. `:global(a:hover)` rather than a class of the header
     * that mounts it, so the component needs to know nothing about where it
     * was mounted; it is always inside the GitHub anchor.
     */
    .star {
        width: 1.2em;
        height: 1.2em;
        flex: none;
        fill: transparent;
        transition: fill 0.3s ease;
    }
    :global(a:hover) .star {
        fill: currentColor;
    }

    @keyframes stars-in {
        from {
            opacity: 0;
        }
        to {
            opacity: 1;
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .stars {
            animation: none;
        }
        .star {
            transition: none;
        }
    }
</style>
