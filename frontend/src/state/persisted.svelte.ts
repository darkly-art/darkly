/**
 * Tiny localStorage-backed reactive value. The first generic state in
 * `state/` that wants raw localStorage (the theme rides the config store);
 * factor the read/parse/persist pattern once here so the second consumer
 * (`menuBar`) and any future one don't copy-paste it.
 *
 * Returns an object with a `.value` accessor: reads are reactive, writes
 * update the rune *and* persist to localStorage (best-effort — a failing
 * `setItem`, e.g. private-mode quota, is swallowed so the UI never breaks).
 */
export function persistedState<T>(key: string, initial: T) {
    function read(): T {
        if (typeof localStorage === 'undefined') return initial;
        try {
            const raw = localStorage.getItem(key);
            return raw === null ? initial : (JSON.parse(raw) as T);
        } catch {
            return initial;
        }
    }

    let value = $state<T>(read());

    return {
        get value() {
            return value;
        },
        set value(next: T) {
            value = next;
            try {
                localStorage?.setItem(key, JSON.stringify(next));
            } catch {
                // Persistence is best-effort; ignore quota / private-mode errors.
            }
        },
    };
}
