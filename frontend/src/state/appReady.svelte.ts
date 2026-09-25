/**
 * Whether the app has finished coming up: the WASM engine is live, the canvas
 * is sized, and the first frame is on its way.
 *
 * Chrome that would rather not interrupt the boot waits on this instead of on
 * `window`'s load event, which fires while the engine is still being fetched
 * and built and so says nothing useful about whether the editor is up.
 *
 * Marked once per session by whichever root brought the app up. A boot that
 * threw still marks: the editor is as loaded as it is going to get, and the
 * reader whose engine just failed is the one most likely to want whatever the
 * chrome has to say.
 */
let ready = $state(false);

export const appReady = {
    get value(): boolean {
        return ready;
    },
};

export function markAppReady() {
    ready = true;
}
