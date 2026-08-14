// Drag lifecycle for value scrubs: preview locally while the pointer moves,
// commit once when it's released. Kept free of DOM so it can be unit-tested
// headlessly (vitest runs in node — no `window`), and free of app state so the
// caller decides what a preview and a commit mean.
//
// The intermediate values of a scrub are transient session state. Sending each
// one to the engine makes every frame of a gesture a committed mutation, which
// recompiles the brush graph and re-derives its previews for values the user is
// only passing through.

export interface ScrubDragOptions {
    /** Map a pointer position to the value it represents. */
    toValue: (clientX: number, clientY: number) => number;
    /** Called on every move with the value under the pointer. Local only. */
    onPreview: (value: number) => void;
    /** Called at most once, with the last previewed value. */
    onCommit: (value: number) => void;
    /** Called exactly once when the drag finishes, however it finishes — the
     *  hook for releasing whatever the caller acquired at pointerdown. */
    onFinish?: () => void;
}

export interface ScrubDrag {
    /** Preview the value at this pointer position. */
    move: (clientX: number, clientY: number) => void;
    /** Finish the drag, committing the last previewed value. Idempotent, so
     *  `pointerup` and `lostpointercapture` can both route here. */
    end: () => void;
}

/**
 * Start a scrub drag. Nothing is committed until {@link ScrubDrag.end}, and a
 * drag that never moved commits nothing — seed it with an immediate `move` if
 * the gesture should take effect from the pointerdown position (a slider track
 * that jumps to the click).
 *
 * A drag whose pointer capture is lost mid-gesture still commits: the user has
 * already seen the previewed value, so committing is what keeps the caller's
 * local state and the engine in agreement.
 */
export function beginScrubDrag(options: ScrubDragOptions): ScrubDrag {
    let previewed: number | null = null;
    let finished = false;

    return {
        move(clientX: number, clientY: number) {
            if (finished) return;
            previewed = options.toValue(clientX, clientY);
            options.onPreview(previewed);
        },
        end() {
            if (finished) return;
            finished = true;
            if (previewed !== null) options.onCommit(previewed);
            options.onFinish?.();
        },
    };
}
