/**
 * The explorer's own scrolling.
 *
 * Neither pane scrolls natively. The reason is the one thing native scrolling
 * cannot give us here: a **single clock**. A browser scrollport moves on the
 * compositor thread and does not wait for JavaScript, so a pane driven from a
 * `requestAnimationFrame` callback is always mirroring a position its subject
 * may already have left. With two scrollports and a ribbon stretched between
 * them, that one-frame disagreement lands exactly on the joins, which is where
 * it is most visible — the tearing this module exists to make unrepresentable.
 *
 * So the explorer owns one number, advances it once per frame, and derives
 * every column from it. Two panes and a ribbon drawn from one value cannot
 * disagree, because there is nothing left for them to disagree about.
 *
 * The cost is that momentum is ours to write. `glide` is that: an exponential
 * decay, which is what a flick feels like and what the platform was giving us
 * for free.
 *
 * The maths here is pure and testable; `panScroll` is the DOM half.
 */

/** Velocity decay, per second: `v(t) = v0 · e^(−FRICTION·t)`. Higher stops
 *  sooner. Tuned so a hard flick crosses roughly a viewport and settles. */
export const FRICTION = 5;

/** Below this many px/s a glide is over. Without a floor an exponential never
 *  reaches zero and the frame loop never sleeps. */
export const STOP_VELOCITY = 12;

/** How fast a tap-to-jump closes on its target, same exponential form. */
export const JUMP_RATE = 11;

/** Distance a pointer must travel before it is a scroll rather than a click.
 *  Below it, a tap on a brush tile stays a tap. */
export const DRAG_THRESHOLD = 5;

/** How far back a fling looks for its velocity, ms. Long enough to average out
 *  a jittery pen, short enough that a pause before release means a stop. */
export const FLING_WINDOW = 90;

export const clampScroll = (y: number, max: number) => Math.min(Math.max(y, 0), Math.max(0, max));

/**
 * Advance a glide by `dt` seconds.
 *
 * Closed form rather than a per-frame multiply, so the distance travelled does
 * not depend on the frame rate — the same flick goes the same distance whether
 * the browser is delivering 60 frames a second or 30.
 *
 * Hitting either end kills the velocity outright. There is no rubber band: the
 * list has a real top and bottom, and bouncing off them would be motion the
 * wheel beside it cannot mirror, since its own range is a different length.
 */
export function glide(
    y: number,
    v: number,
    dt: number,
    max: number,
): { y: number; v: number } {
    const remaining = v * Math.exp(-FRICTION * dt);
    const next = y + (v - remaining) / FRICTION;
    const stopped = Math.abs(remaining) < STOP_VELOCITY;
    const clamped = clampScroll(next, max);
    return {
        y: clamped,
        v: stopped || clamped !== next ? 0 : remaining,
    };
}

/**
 * Ease `y` toward `target` by `dt` seconds.
 *
 * Also frame-rate independent, and also exponential — so a jump that is
 * interrupted by a flick hands over without a discontinuity, both being the
 * same kind of curve.
 */
export function approach(y: number, target: number, dt: number, rate = JUMP_RATE): number {
    const next = target + (y - target) * Math.exp(-rate * dt);
    return Math.abs(next - target) < 0.5 ? target : next;
}

/** A position and time, for measuring a fling. */
interface Sample {
    t: number;
    y: number;
}

/**
 * Velocity in px/s from the tail of a pointer's travel, or 0 if it rested.
 *
 * Measured over a window rather than from the last two events: pointer events
 * arrive irregularly and a single short interval divides by a tiny `dt`, which
 * turns one jittery sample into a launch across the whole library.
 */
export function flingVelocity(samples: Sample[], now: number): number {
    const recent = samples.filter(s => now - s.t <= FLING_WINDOW);
    if (recent.length < 2) return 0;
    const first = recent[0];
    const last = recent[recent.length - 1];
    const dt = (last.t - first.t) / 1000;
    if (dt <= 0) return 0;
    return (last.y - first.y) / dt;
}

export interface PanScrollOptions {
    /** Input asked the pane to move this many px along its own axis — positive
     *  is "further down the content", matching `scrollTop`. */
    onDelta: (dy: number) => void;
    /** The hand left the pane at this velocity, px/s in the same sense. */
    onFling: (velocity: number) => void;
    /** A hand landed. Whatever the pane was doing on its own should stop. */
    onGrab: () => void;
}

/**
 * Make an element pannable by wheel, pen, mouse and touch.
 *
 * Both panes use this, which is the point: the wheel and the list differ in
 * what a pixel of travel *means*, not in how a hand moves them, so the handling
 * lives once here and each pane converts the delta it is handed.
 *
 * A drag only becomes a drag past {@link DRAG_THRESHOLD}, and a click that
 * followed one is swallowed in the capture phase — otherwise flinging the list
 * by grabbing a brush tile would also load that brush.
 */
export function panScroll(node: HTMLElement, options: PanScrollOptions) {
    let opts = options;
    let samples: Sample[] = [];
    let dragging = false;
    let pointer: number | null = null;
    let lastY = 0;
    let startY = 0;
    let suppressClick = false;

    function onWheel(e: WheelEvent) {
        // The pane is not scrollable in the browser's eyes, so nothing else
        // would consume this and it would bubble out to the page.
        e.preventDefault();
        opts.onGrab();
        opts.onDelta(e.deltaY);
    }

    function onPointerDown(e: PointerEvent) {
        if (pointer !== null || e.button !== 0) return;
        pointer = e.pointerId;
        startY = e.clientY;
        lastY = e.clientY;
        samples = [{ t: e.timeStamp, y: e.clientY }];
        opts.onGrab();
    }

    function onPointerMove(e: PointerEvent) {
        if (e.pointerId !== pointer) return;
        samples.push({ t: e.timeStamp, y: e.clientY });
        if (samples.length > 8) samples.shift();

        if (!dragging) {
            if (Math.abs(e.clientY - startY) < DRAG_THRESHOLD) return;
            dragging = true;
            // Capture only once it is a drag: taking it on pointerdown would
            // steal the hover and click behaviour of everything in the pane.
            node.setPointerCapture(e.pointerId);
        }
        // Dragging content down scrolls toward the top of it.
        opts.onDelta(-(e.clientY - lastY));
        lastY = e.clientY;
    }

    function onPointerUp(e: PointerEvent) {
        if (e.pointerId !== pointer) return;
        if (dragging) {
            if (node.hasPointerCapture(e.pointerId)) node.releasePointerCapture(e.pointerId);
            opts.onFling(-flingVelocity(samples, e.timeStamp));
            suppressClick = true;
        }
        pointer = null;
        dragging = false;
        samples = [];
    }

    function onClickCapture(e: MouseEvent) {
        if (!suppressClick) return;
        suppressClick = false;
        e.stopPropagation();
        e.preventDefault();
    }

    node.addEventListener('wheel', onWheel, { passive: false });
    node.addEventListener('pointerdown', onPointerDown);
    node.addEventListener('pointermove', onPointerMove);
    node.addEventListener('pointerup', onPointerUp);
    node.addEventListener('pointercancel', onPointerUp);
    node.addEventListener('click', onClickCapture, true);

    return {
        update(next: PanScrollOptions) {
            opts = next;
        },
        destroy() {
            node.removeEventListener('wheel', onWheel);
            node.removeEventListener('pointerdown', onPointerDown);
            node.removeEventListener('pointermove', onPointerMove);
            node.removeEventListener('pointerup', onPointerUp);
            node.removeEventListener('pointercancel', onPointerUp);
            node.removeEventListener('click', onClickCapture, true);
        },
    };
}
