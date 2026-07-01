import { makeApi, type EngineApi } from './protocol_gen';

/** A minimal fake engine transport for unit tests: `send`/`post` spies plus a
 *  real {@link EngineApi} that forwards to them. Because `api` closes over the
 *  same spies, assertions can still inspect `engine.send`/`engine.post` calls by
 *  kind — the typed client is just sugar over the same transport. */
export interface MockEngine {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    send: (kind: string, payload?: object, bytes?: Uint8Array) => Promise<any>;
    post: (kind: string, payload?: object, bytes?: Uint8Array) => void;
    api: EngineApi;
}

/** Attach a real `api` to a fake engine whose `send`/`post` are test spies.
 *  `post` is optional — a mock exercising only awaited requests can omit it. */
export function withApi<T extends { send: MockEngine['send']; post?: MockEngine['post'] }>(
    engine: T,
): T & { api: EngineApi } {
    // Forward with the exact arity of the old direct `send`/`post` calls
    // (dropping trailing `undefined` payload/bytes) so tests can assert the
    // precise `(kind)` / `(kind, payload)` shape via `toHaveBeenCalledWith`.
    const args = (kind: string, payload?: object, bytes?: Uint8Array): unknown[] => {
        if (bytes !== undefined) return [kind, payload, bytes];
        if (payload !== undefined) return [kind, payload];
        return [kind];
    };
    const api = makeApi({
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        request: (kind, payload, bytes) => (engine.send as any)(...args(kind, payload, bytes)),
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        postFF: (kind, payload, bytes) => (engine.post as any)?.(...args(kind, payload, bytes)),
    });
    return Object.assign(engine, { api });
}
