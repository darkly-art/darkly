/**
 * Recently-used brushes and colors.
 *
 * Two bounded, deduplicated, most-recently-used lists sharing one
 * `recents.json`. They belong to the painter, not to a canvas: they are not
 * document state (they must not ride a `.darkly` file into someone else's
 * hands), not session state (they survive reload), and not derivable from
 * anything. So they are a file in the Darkly directory, alongside
 * `user_settings.json`, and they travel with `exportRootAsZip`.
 *
 * Both producers and both consumers live in the frontend, so nothing here
 * crosses the wasm boundary.
 */
import { jsonFile } from '../storage/jsonStore';
import type { DarklyStorage } from '../storage/types';

/** How many of each we keep. Deep enough to be worth reaching for, shallow
 *  enough that a radial widget can show them all without paging. */
const BRUSH_CAP = 12;
const COLOR_CAP = 16;

const RECENTS_FILE = 'recents.json';

interface RecentsFile {
    brushes: string[];
    colors: string[];
}

const EMPTY = (): RecentsFile => ({ brushes: [], colors: [] });

/** A stored file is arbitrary JSON — possibly hand-edited, possibly from an
 *  older shape. Anything that is not a list of strings reads as empty rather
 *  than propagating a bad value into the UI. */
function strings(v: unknown): string[] {
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

function validate(raw: unknown): RecentsFile {
    if (typeof raw !== 'object' || raw === null) return EMPTY();
    const o = raw as Record<string, unknown>;
    return { brushes: strings(o.brushes), colors: strings(o.colors) };
}

export interface RecentList {
    /** The list, newest first. */
    readonly items: string[];
    /** Record `value` as just-used: moved to the front if present, prepended
     *  if not, and truncated to the cap. Writes nothing when `value` is
     *  already at the front, which is what makes calling this per pointer
     *  event free. */
    use(value: string): void;
    /** Drop entries that no longer resolve — a brush that has been deleted.
     *  Rewrites only if something was actually dropped. */
    retain(keep: (value: string) => boolean): void;
}

export interface Recents {
    brushes: RecentList;
    colors: RecentList;
    /** Read `recents.json` into memory. Idempotent — the first call does the
     *  read and every later one awaits the same promise. */
    load(): Promise<void>;
    /** Write anything pending immediately — for `beforeunload`. */
    flush(): Promise<void>;
}

/**
 * Build a recents store over one `recents.json`.
 *
 * Exported so tests can drive it against an in-memory storage; the app uses
 * the module-level singleton below.
 */
export function createRecents(storage?: DarklyStorage): Recents {
    const file = jsonFile<RecentsFile>(RECENTS_FILE, EMPTY, validate, storage);

    /** The in-memory mirror. `$state` so the picker and the future radial
     *  widget re-derive when a list changes; the file is the durable copy. */
    const state = $state<RecentsFile>(EMPTY());
    let loaded: Promise<void> | null = null;

    /**
     * A bounded, deduplicated MRU list backed by one field of the file.
     *
     * `key` collapses values that should count as the same entry; it defaults
     * to identity. Colors use it to dedupe on RGB while storing the alpha they
     * were last used at, so scrubbing opacity does not flood the list with one
     * hue.
     */
    function list(
        field: keyof RecentsFile,
        cap: number,
        key: (v: string) => string = v => v,
    ): RecentList {
        return {
            get items() {
                return state[field];
            },
            use(value: string): void {
                const current = state[field];
                if (current.length > 0 && key(current[0]) === key(value)) {
                    // Already the most recent: nothing to reorder, nothing to
                    // write. This is what makes a per-`pointermove` call free.
                    return;
                }
                const k = key(value);
                state[field] = [value, ...current.filter(v => key(v) !== k)].slice(0, cap);
                file.write({ brushes: state.brushes, colors: state.colors });
            },
            retain(keep: (value: string) => boolean): void {
                const current = state[field];
                const next = current.filter(keep);
                if (next.length === current.length) return;
                state[field] = next;
                file.write({ brushes: state.brushes, colors: state.colors });
            },
        };
    }

    return {
        brushes: list('brushes', BRUSH_CAP),
        colors: list('colors', COLOR_CAP, c => c.slice(0, 7).toLowerCase()),
        load(): Promise<void> {
            loaded ??= file.read().then(v => {
                state.brushes = v.brushes;
                state.colors = v.colors;
            });
            return loaded;
        },
        flush: () => file.flush(),
    };
}

const recents = createRecents();

/** Recently used brushes, keyed by brush id. */
export const recentBrushes = recents.brushes;

/** Recently used colors as canonical `#rrggbbaa`. Deduplicated on the RGB
 *  half: the same hue at a different opacity is the same swatch. */
export const recentColors = recents.colors;

export const loadRecents = () => recents.load();
export const flushRecents = () => recents.flush();
