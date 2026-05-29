import {
    config_get, config_set, config_reset, config_reset_all,
    config_base_names, config_base_value, config_schema, config_version,
} from '../../wasm/pkg/darkly_wasm';
import { storage, readJson, writeJson } from '../storage';
import type { SectionInfo } from './schema';
import { validateOverrides } from './validate';

/**
 * Three layers, all sourced from Rust at runtime:
 *
 *   defaults    ← editor-AGNOSTIC defaults.yaml (always applied)
 *   overlay     ← one of {Krita, Photoshop, GIMP}, selected by `app.baseSettings`
 *   user        ← personal customizations on top
 *
 * Resolution: `user[key] ?? overlay[active][key] ?? defaults[key]`.
 *
 * Persistence: the user layer is one flat JSON file at `user_settings.json`
 * in the Darkly storage dir. No named user-presets, no `.active` pointer.
 * The base-settings choice (`app.baseSettings`) is itself stored in the user
 * layer, so switching editors is just `config.set('app.baseSettings', ...)`.
 *
 * On-disk envelope: `{ "version": <CONFIG_VERSION>, "values": {...} }`.
 * Pre-release we discard mismatched-version files outright (per CLAUDE.md
 * "No Migrations"); the field exists so post-release migrations have a
 * discriminator to key off.
 */

const USER_SETTINGS_FILE = 'user_settings.json';

interface UserSettingsFile {
    version: number;
    values: Record<string, unknown>;
}

type ChangeListener = () => void;

class ConfigStore {
    /** Bumped on every mutation to drive Svelte reactivity. */
    #version = $state(0);

    /** Whether init() has finished. */
    #ready = false;

    /** User-layer overrides mirrored from disk. */
    #values: Record<string, unknown> = {};

    /** Pending-write timer for debounced disk persistence. */
    #writeTimer: ReturnType<typeof setTimeout> | null = null;

    /** Subscribers fired after every mutation. */
    #listeners: ChangeListener[] = [];

    /** True when no base editor has been picked yet — drives the
     *  first-run PresetPicker. */
    needsPresetChoice = $state(false);

    /** Equal-status overlay names (alphabetical), populated from Rust. */
    baseNames = $state<string[]>([]);

    /** Flat preferences schema, loaded once on init. */
    schema = $state<SectionInfo[]>([]);

    /** Initialize the store. Must be called after WASM init().
     *  Reads the schema, the overlay list, and the user-settings file. */
    async init() {
        try {
            this.schema = JSON.parse(config_schema()) as SectionInfo[];
        } catch (e) {
            console.error('[config] failed to parse schema JSON', e);
            this.schema = [];
        }

        this.baseNames = (config_base_names() as string[]) ?? [];

        // Load any stored user overrides.
        try {
            const raw = await readJson<Partial<UserSettingsFile>>(USER_SETTINGS_FILE);
            const expectedVersion = config_version();
            if (raw && raw.version === expectedVersion && raw.values && typeof raw.values === 'object') {
                const { cleaned, changed } = validateOverrides(this.schema, raw.values);
                this.#values = cleaned;
                for (const [k, v] of Object.entries(cleaned)) {
                    config_set(k, v);
                }
                if (changed) {
                    // Write the cleaned-up file back so we don't keep warning.
                    await writeJson(USER_SETTINGS_FILE, {
                        version: expectedVersion,
                        values: cleaned,
                    } satisfies UserSettingsFile);
                }
            } else if (raw) {
                // File exists but version doesn't match (or envelope is
                // malformed). Pre-release: discard and treat as first run.
                // Post-release: this is the migration entry point.
                console.warn(
                    `[config] user_settings.json version ${raw.version} != expected ${expectedVersion} (or malformed); discarding.`,
                );
                await storage.remove(USER_SETTINGS_FILE);
                this.#values = {};
            } else {
                this.#values = {};
            }
        } catch (e) {
            console.error('[config] user-settings load failed', e);
            this.#values = {};
        }

        const baseChoice = this.#values['app.baseSettings'];
        this.needsPresetChoice = typeof baseChoice !== 'string';

        this.#ready = true;
        this.#version++;
        this.#fire();
    }

    /** Subscribe to mutations. Returns an unsubscribe fn. */
    onChange(fn: ChangeListener): () => void {
        this.#listeners.push(fn);
        return () => {
            const i = this.#listeners.indexOf(fn);
            if (i >= 0) this.#listeners.splice(i, 1);
        };
    }

    /** Resolved value (user → overlay → defaults). */
    get(key: string): unknown {
        void this.#version;
        if (!this.#ready) return undefined;
        return config_get(key);
    }

    /** Layer-below-user value (overlay → defaults). Used by the Settings
     *  UI to label the Reset button with what would be revealed. */
    baseValue(key: string): unknown {
        void this.#version;
        if (!this.#ready) return undefined;
        return config_base_value(key);
    }

    /** Whether this key currently has a user-layer override. */
    hasOverride(key: string): boolean {
        void this.#version;
        return key in this.#values;
    }

    /** Set a user-layer override. Persists to disk (debounced). */
    set(key: string, value: unknown) {
        config_set(key, value);
        this.#values = { ...this.#values, [key]: value };
        this.#scheduleWrite();
        this.#version++;
        this.#fire();
    }

    /** Remove a user override, revealing the overlay/default below. */
    resetKey(key: string) {
        config_reset(key);
        const next = { ...this.#values };
        delete next[key];
        this.#values = next;
        this.#scheduleWrite();
        this.#version++;
        this.#fire();
    }

    /** Reset every pref in a section. */
    resetSection(sectionId: string) {
        const section = this.schema.find(s => s.id === sectionId);
        if (!section) return;
        const next = { ...this.#values };
        for (const pref of section.prefs) {
            if (pref.key in next) {
                config_reset(pref.key);
                delete next[pref.key];
            }
        }
        this.#values = next;
        this.#scheduleWrite();
        this.#version++;
        this.#fire();
    }

    /** Clear every user override **except** `app.baseSettings` — the
     *  picker choice survives a global reset. */
    resetAllOverrides() {
        config_reset_all();
        const next: Record<string, unknown> = {};
        const base = this.#values['app.baseSettings'];
        if (typeof base === 'string') next['app.baseSettings'] = base;
        this.#values = next;
        this.#scheduleWrite();
        this.#version++;
        this.#fire();
    }

    /** Set the active editor overlay. Sugar for setting `app.baseSettings`
     *  and dismissing the first-run picker. */
    setBase(name: string) {
        if (!this.baseNames.includes(name)) {
            console.warn(`[config] unknown base name: ${name}`);
            return;
        }
        this.set('app.baseSettings', name);
        this.needsPresetChoice = false;
    }

    // ---- internals ----

    /** Schedule a debounced write of the user layer to disk. */
    #scheduleWrite() {
        if (this.#writeTimer !== null) return;
        this.#writeTimer = setTimeout(() => {
            this.#writeTimer = null;
            const snapshot = this.#values;
            (async () => {
                try {
                    if (Object.keys(snapshot).length === 0) {
                        // Tidy: an empty user layer doesn't need a file.
                        await storage.remove(USER_SETTINGS_FILE);
                    } else {
                        await writeJson(USER_SETTINGS_FILE, {
                            version: config_version(),
                            values: snapshot,
                        } satisfies UserSettingsFile);
                    }
                } catch (e) {
                    console.error('[config] user-settings write failed', e);
                }
            })();
        }, 200);
    }

    #fire() {
        for (const fn of this.#listeners) {
            try { fn(); } catch (e) { console.error('[config] onChange listener threw:', e); }
        }
    }
}

export const config = new ConfigStore();

/**
 * Format a binding (`"Shift+KeyR"`, `"$mod+KeyA"`, `"$mod+click"`, …) into
 * a human-readable shortcut string (e.g. `"Shift+R"`, `"Ctrl+A"` / `"Cmd+A"`,
 * `"⌘+click"`). Accepts bindings with an optional site/scope prefix
 * (`"layerPanel:Delete"`, `"@paint:KeyB"`, `"canvas@paint:$mod+drag"`) and
 * strips it before formatting — only the chord is user-facing.
 *
 * Handles both the keyboard chord vocabulary (`Shift`/`Alt` capitalized, key
 * codes like `KeyA`/`Comma`) and the mouse chord vocabulary
 * (`shift`/`alt`/`ctrl`/`meta` lowercase, verbs like `click`/`drag`).
 */
export function formatHotkey(binding: string | undefined): string | undefined {
    if (!binding) return undefined;
    const colonIdx = binding.indexOf(':');
    const chord = colonIdx < 0 ? binding : binding.slice(colonIdx + 1);
    if (!chord) return undefined;
    const isMac = /Mac|iPhone|iPad/.test(navigator.userAgent);
    return chord.split('+').map(part => {
        if (part === '$mod') return isMac ? '⌘' : 'Ctrl';
        if (part === 'Shift' || part === 'shift') return isMac ? '⇧' : 'Shift';
        if (part === 'Alt' || part === 'alt') return isMac ? '⌥' : 'Alt';
        if (part === 'ctrl') return isMac ? '⌃' : 'Ctrl';
        if (part === 'meta') return isMac ? '⌘' : 'Win';
        if (part === 'click') return 'click';
        if (part === 'doubleClick') return 'double-click';
        if (part === 'middleClick') return 'middle-click';
        if (part === 'drag') return 'drag';
        if (part === 'middleDrag') return 'middle-drag';
        if (part === 'rightDrag') return 'right-drag';
        if (part.startsWith('Key')) return part.slice(3);
        if (part === 'Delete') return 'Del';
        if (part === 'Comma') return ',';
        if (part === 'Period') return '.';
        if (part === 'Semicolon') return ';';
        if (part === 'Quote') return "'";
        if (part === 'BracketLeft') return '[';
        if (part === 'BracketRight') return ']';
        if (part === 'Backslash') return '\\';
        if (part === 'Minus') return '-';
        if (part === 'Equal') return '=';
        if (part === 'Slash') return '/';
        if (part === 'Backquote') return '`';
        return part;
    }).join('+');
}

/**
 * Build a tooltip combining a label with the action's effective hotkey, if
 * any. The binding comes straight from the resolved config (no
 * action-registry default fallback — defaults live in YAML now).
 * Reactive to the config so the tooltip re-renders whenever the user
 * rebinds or switches editor overlays.
 */
export function tooltipForAction(label: string, actionId: string): string {
    const v = config.get(`hotkeys.${actionId}`);
    if (typeof v !== 'string' || !v) return label;
    const hk = formatHotkey(v.split('|')[0]);
    return hk ? `${label} (${hk})` : label;
}
