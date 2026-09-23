/**
 * Stand-in for the WASM-backed config store, for tests that mount components
 * reading prefs.
 *
 * It mirrors the one contract of the real store that callers actually depend
 * on: `get` is reactive. `ConfigStore` reads a `$state` version counter on
 * every `get` and bumps it on every mutation (`config/store.svelte.ts:47,136`),
 * which is what lets a `$derived` over a pref recompute when the pref changes.
 * A plain object cannot do that, and a module-level `$derived` reading one
 * caches its first value forever.
 */
class FakeConfigStore {
    #version = $state(0);

    #values: Record<string, unknown> = {};

    get(key: string): unknown {
        void this.#version;
        return this.#values[key];
    }

    number(key: string, fallback: number): number {
        const raw = this.get(key);
        return typeof raw === 'number' && Number.isFinite(raw) ? raw : fallback;
    }

    set(key: string, value: unknown) {
        this.#values[key] = value;
        this.#version++;
    }

    /** Drop every value and invalidate readers. Call between tests. */
    reset() {
        this.#values = {};
        this.#version++;
    }

    /** Subscribers are never fired: nothing here mutates config behind a
     *  test's back. Present because modules like `state/theme.svelte.ts`
     *  subscribe at import time, so the method has to exist. */
    onChange(_fn: () => void): () => void {
        return () => {};
    }
}

export const fakeConfig = new FakeConfigStore();
