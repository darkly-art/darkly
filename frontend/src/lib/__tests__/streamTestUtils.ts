/**
 * Shared fakes for `HttpStreamSource` tests (unit + app-state level): the
 * on-wire length-prefix framing and a hand-cranked `fetch` body reader, so
 * frame/heartbeat delivery and stream close can be interleaved with the code
 * under test in the node vitest env (which has no real `fetch`).
 */

/** `[4-byte big-endian length][payload]`: the on-wire frame format. */
export function lenPrefixed(payload: Uint8Array): Uint8Array {
    const out = new Uint8Array(4 + payload.length);
    new DataView(out.buffer).setUint32(0, payload.length, false);
    out.set(payload, 4);
    return out;
}

/** Transport heartbeat: a zero-length frame (just the length prefix). */
export const HEARTBEAT = lenPrefixed(new Uint8Array(0));

/** A reader whose `read()` resolves only when the test pushes a chunk or
 *  closes, so delivery can be interleaved with ticks. */
export function controllableReader() {
    const waiters: Array<(r: { done: boolean; value?: Uint8Array }) => void> = [];
    const buffered: Array<{ done: boolean; value?: Uint8Array }> = [];
    const deliver = (r: { done: boolean; value?: Uint8Array }) => {
        const w = waiters.shift();
        if (w) w(r);
        else buffered.push(r);
    };
    return {
        reader: {
            read: () =>
                new Promise<{ done: boolean; value?: Uint8Array }>((resolve) => {
                    const b = buffered.shift();
                    if (b) resolve(b);
                    else waiters.push(resolve);
                }),
        },
        push: (value: Uint8Array) => deliver({ done: false, value }),
        close: () => deliver({ done: true }),
    };
}
