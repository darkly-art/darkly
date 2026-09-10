/**
 * Opaque id generation.
 *
 * Ids are minted here rather than in Rust because the `darkly` crate has no
 * random-number source, and adding one for wasm means the `getrandom/js`
 * feature dance. The browser already has `crypto.randomUUID`; Rust's job is
 * to reject an empty or duplicate id, which is deterministic and testable.
 */

/** A fresh opaque id, prefixed so it reads clearly wherever it surfaces.
 *
 *  Falls back to `Math.random` where `crypto.randomUUID` is unavailable
 *  (non-secure contexts, older embedders). The fallback is not
 *  cryptographically strong and does not need to be: these are local
 *  identifiers, not secrets. */
export function newId(prefix: string): string {
    if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
        return `${prefix}-${crypto.randomUUID()}`;
    }
    const rand = () => Math.random().toString(36).slice(2);
    return `${prefix}-${rand()}${rand()}`;
}
