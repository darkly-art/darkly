/**
 * The build's announcement: a strip of operator-authored HTML shown at the top
 * of the app until the reader dismisses it. The content is baked in at build
 * time from the `DARKLY_BANNER` environment variable (see docs/build-configuration.md);
 * a build that sets none has no announcement and renders nothing.
 *
 * The dismissal is keyed on the message text itself rather than on a flag or a
 * version, so shipping different copy re-announces itself to a reader who
 * dismissed the previous message, while redeploying identical copy stays
 * dismissed. Comparing the strings directly is exact and needs no digest: the
 * message is a few hundred bytes and the current one is already in memory to
 * render it.
 *
 * The value is trimmed once, here, because deploy pipelines routinely hand over
 * a trailing newline (a YAML block scalar, a `$(cat banner.html)`); untrimmed,
 * that would make whitespace count as content and turn an unchanged message
 * into a spurious re-announcement.
 */
import { persistedState } from './persisted.svelte';

/** Exported for tests; the app reads the `announcement` singleton below. */
export class Announcement {
    readonly html: string;
    #dismissed = persistedState<string | null>('darkly.announcementDismissed', null);

    constructor(html: string) {
        this.html = html.trim();
    }

    get visible(): boolean {
        return this.html !== '' && this.#dismissed.value !== this.html;
    }

    dismiss() {
        this.#dismissed.value = this.html;
    }
}

// `typeof` guard so importing this module never throws in a context where
// Vite's `define` was not applied; the value is replaced inline at build time.
// Same shape and same reason as version.ts.
export const announcement = new Announcement(
    typeof __DARKLY_BANNER_HTML__ === 'string' ? __DARKLY_BANNER_HTML__ : '',
);
