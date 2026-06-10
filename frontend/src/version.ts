/**
 * Darkly's version — the single frontend home for it. Consumers import
 * `darklyVersion` from here; nobody else touches the injected constant.
 *
 * The raw value comes from `git describe --tags --long` at build time (injected
 * by Vite as `__DARKLY_VERSION__`; see vite.config.ts) and always has the shape
 * `TAG-COMMITS-gSHA`, e.g. `v0.3.0-1-gf0c3ea9`. The tag is Darkly's release
 * version (the same v* tags darkly-deploy/ builds from); COMMITS is the height
 * since that tag (0 when HEAD *is* the tag).
 */

export interface DarklyVersion {
    /** The latest reachable tag, e.g. `v0.3.0`. */
    tag: string;
    /** Commit height since the tag — 0 when HEAD is exactly the tag. */
    commits: number;
    /** Abbreviated commit hash (without the `g` prefix git adds). */
    sha: string;
}

/** Parse `git describe --tags --long` output (`TAG-COMMITS-gSHA`). */
export function parseVersion(raw: string): DarklyVersion {
    // Tags themselves can contain `-`, so anchor on the trailing
    // `-<commits>-g<sha>` that --long always appends.
    const m = /^(.*)-(\d+)-g([0-9a-zA-Z]+)$/.exec(raw);
    if (!m) {
        return { tag: raw, commits: 0, sha: '' };
    }
    return { tag: m[1], commits: Number(m[2]), sha: m[3] };
}

/**
 * Human-readable version string. At a tagged release (height 0) this is just
 * the tag, e.g. `v0.3.0`. Otherwise the commit height is appended — the
 * required signal that the build is ahead of the tag — with the short SHA for
 * dev clarity, e.g. `v0.3.0 +1 ·f0c3ea9`.
 */
export function formatVersion(raw: string): string {
    const { tag, commits, sha } = parseVersion(raw);
    if (commits === 0) return tag;
    return sha ? `${tag} +${commits} ·${sha}` : `${tag} +${commits}`;
}

// `typeof` guard so importing this module never throws even if `define` isn't
// applied in some context — the value is replaced inline at build/test time.
const RAW = typeof __DARKLY_VERSION__ === 'string' ? __DARKLY_VERSION__ : '0.0.0-0-gunknown';

export const darklyVersion = formatVersion(RAW);
