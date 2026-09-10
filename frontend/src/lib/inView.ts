/**
 * Svelte action: report whether an element is near the scrollport.
 *
 * For gating work that is only worth doing for what the painter can actually
 * see. The brush explorer's preview strips are the motivating case: each
 * mounted one runs a throttled engine round trip plus a bounded
 * `requestAnimationFrame` poll that asks for a frame every tick, so mounting
 * every tile of every pack at once is a render storm at exactly the moment the
 * view opens.
 *
 * `rootMargin` deliberately overshoots the viewport so a tile is ready by the
 * time it scrolls in rather than popping blank.
 */
export interface InViewOptions {
    /** Called with `true` once the element is near the scrollport, and `false`
     *  when it leaves. */
    onChange: (visible: boolean) => void;
    /** How far outside the scrollport still counts as visible. */
    rootMargin?: string;
}

export function inView(node: HTMLElement, options: InViewOptions) {
    let opts = options;

    // No IntersectionObserver (jsdom, an old embedder) means no gating: report
    // visible and let everything mount, which is the pre-existing behaviour.
    if (typeof IntersectionObserver === 'undefined') {
        opts.onChange(true);
        return {
            update(next: InViewOptions) {
                opts = next;
            },
        };
    }

    const observer = new IntersectionObserver(
        entries => {
            for (const entry of entries) opts.onChange(entry.isIntersecting);
        },
        { rootMargin: options.rootMargin ?? '300px' },
    );
    observer.observe(node);

    return {
        update(next: InViewOptions) {
            opts = next;
        },
        destroy() {
            observer.disconnect();
        },
    };
}
