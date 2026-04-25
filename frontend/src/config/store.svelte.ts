import {
    config_get, config_set, config_reset, config_reset_all,
    config_preset_names, config_preset_values, config_schema,
} from '../../wasm/pkg/darkly_wasm';
import {
    getDir, listEntries, readJson, readText, removeEntry, writeJson, writeText, sanitizeFilename,
} from '../storage/root';
import type { SectionInfo } from './schema';
import { validateOverrides } from './validate';

/**
 * Two layers, no live "preset" layer:
 *
 *   defaults      ← immutable, baked into Rust binary
 *   settings      ← the user's actual values for any key they've changed
 *
 * Resolution: settings[key] ?? defaults[key].
 *
 * Persistence: the user's settings live as a JSON file inside the Darkly
 * directory at `presets/<active-preset-name>.json`. The active preset name
 * itself is stored in `presets/.active`.
 *
 * Built-in templates (Krita / Photoshop / GIMP) are read-only data shipped
 * inside the Rust binary, exposed via `config_preset_values(name)`. They
 * are not stored layers — applying one writes its values into the active
 * preset's file, full stop.
 *
 * User-created presets are saved JSON files under `presets/`. The
 * built-in/user distinction is purely cosmetic from the Settings UI's
 * perspective; both can be applied the same way.
 */

const PRESETS_DIR = 'presets';
const ACTIVE_FILE = '.active';
const DEFAULT_PRESET_NAME = 'My Settings';

const PRESET_DESCRIPTIONS: Record<string, string> = {
    'Krita': 'Default Krita-style keybindings',
    'Photoshop': 'Adobe Photoshop-style keybindings',
    'GIMP': 'GIMP-style keybindings',
};

type ChangeListener = () => void;

export interface BuiltinPreset {
    name: string;
    description: string;
}

class ConfigStore {
    /** Bumped on every mutation to drive Svelte reactivity. */
    #version = $state(0);

    /** Whether init() has finished. */
    #ready = false;

    /** Active preset's value map, mirrored from the file on disk. */
    #values: Record<string, unknown> = {};

    /** Pending-write timer for debounced disk persistence. */
    #writeTimer: ReturnType<typeof setTimeout> | null = null;

    /** Subscribers fired after every mutation (set / reset / applyTemplate / etc.). */
    #listeners: ChangeListener[] = [];

    /** True when no preset has been chosen yet (first launch, or all presets deleted). */
    needsPresetChoice = $state(false);

    /** Currently active user preset name, or null if none yet. */
    activePresetName = $state<string | null>(null);

    /** Read-only built-in template descriptors. */
    builtinPresets = $state<BuiltinPreset[]>([]);

    /** Names of user-created (writable) presets in the Darkly directory. */
    userPresetNames = $state<string[]>([]);

    /** Flat preferences schema, loaded once on init. */
    schema = $state<SectionInfo[]>([]);

    /**
     * Initialize the store. Must be called after WASM init().
     * Reads the schema and the active preset (if any) from OPFS.
     */
    async init() {
        // Schema (one-shot from Rust).
        try {
            this.schema = JSON.parse(config_schema()) as SectionInfo[];
        } catch (e) {
            console.error('[config] failed to parse schema JSON', e);
            this.schema = [];
        }

        // Built-in template names from Rust.
        const names: string[] = config_preset_names();
        this.builtinPresets = names.map(name => ({
            name,
            description: PRESET_DESCRIPTIONS[name] ?? '',
        }));

        // Discover user presets and the active pointer in the Darkly dir.
        try {
            const dir = await getDir(PRESETS_DIR);
            const entries = await listEntries(dir);
            this.userPresetNames = entries
                .filter(e => e.kind === 'file' && e.name.endsWith('.json'))
                .map(e => e.name.slice(0, -'.json'.length))
                .sort();

            const active = (await readText(dir, ACTIVE_FILE))?.trim() || null;
            if (active && this.userPresetNames.includes(active)) {
                await this.#loadIntoMemory(dir, active);
            } else {
                // First launch (or stale active pointer with no surviving preset).
                this.needsPresetChoice = true;
            }
        } catch (e) {
            console.error('[config] storage init failed', e);
            this.needsPresetChoice = true;
        }

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

    /** Read a setting. Returns the default if no setting is present. */
    get(key: string): unknown {
        void this.#version;
        if (!this.#ready) return undefined;
        return config_get(key);
    }

    /** Whether this key currently has a setting (i.e., differs from default). */
    hasOverride(key: string): boolean {
        void this.#version;
        return key in this.#values;
    }

    /** Set a setting. Persists to the active preset's file (debounced). */
    set(key: string, value: unknown) {
        config_set(key, value);
        this.#values = { ...this.#values, [key]: value };
        this.#scheduleWrite();
        this.#version++;
        this.#fire();
    }

    /** Remove a setting, reverting the key to its default. */
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

    /** First-run completion: user picked a built-in template. Auto-create
     *  their first writable preset from that template's values, set it as
     *  active, and start using it. */
    async pickInitialTemplate(templateName: string) {
        const values = this.#templateValues(templateName);
        if (!values) return;
        // Avoid trampling an existing preset of the same name (eg the user
        // already has "My Settings" from a previous setup).
        let name = DEFAULT_PRESET_NAME;
        let i = 2;
        while (this.userPresetNames.includes(name)) {
            name = `${DEFAULT_PRESET_NAME} ${i++}`;
        }
        await this.#createPreset(name, values);
        await this.#switchTo(name);
        this.needsPresetChoice = false;
        this.#version++;
        this.#fire();
    }

    /** Apply a built-in template into the active preset (overwrites every
     *  key the template covers). The preset name is unchanged. */
    async applyTemplate(templateName: string) {
        if (!this.activePresetName) return;
        const values = this.#templateValues(templateName);
        if (!values) return;
        await this.#replaceActiveValues(values);
        this.#version++;
        this.#fire();
    }

    /** Snapshot current settings under a new preset name and switch to it. */
    async saveAsNewPreset(rawName: string) {
        const name = sanitizeFilename(rawName);
        if (!name) return;
        await this.#createPreset(name, this.#values);
        await this.#switchTo(name);
        this.#version++;
        this.#fire();
    }

    /** Switch the active preset to an existing user preset by name. */
    async switchPreset(name: string) {
        if (!this.userPresetNames.includes(name)) return;
        await this.#switchTo(name);
        this.#version++;
        this.#fire();
    }

    /** Delete a user preset. If it was active, switch to another (or back
     *  to the picker if none remain). */
    async deletePreset(name: string) {
        const dir = await getDir(PRESETS_DIR);
        await removeEntry(dir, `${name}.json`);
        this.userPresetNames = this.userPresetNames.filter(n => n !== name);
        if (this.activePresetName === name) {
            const next = this.userPresetNames[0] ?? null;
            if (next) {
                await this.#switchTo(next);
            } else {
                // Last preset gone — back to the first-run picker.
                config_reset_all();
                this.#values = {};
                this.activePresetName = null;
                await this.#flushActiveFile(null);
                this.needsPresetChoice = true;
            }
        }
        this.#version++;
        this.#fire();
    }

    // ---- internals ----

    /** Look up a built-in template's full snapshot. Validates against the
     *  schema as a defensive measure (drops keys that no longer exist). */
    #templateValues(templateName: string): Record<string, unknown> | null {
        const raw = config_preset_values(templateName) as Record<string, unknown> | null;
        if (!raw) return null;
        const { cleaned } = validateOverrides(this.schema, raw);
        return cleaned;
    }

    /** Read a preset file and load it as the active value map. */
    async #loadIntoMemory(dir: FileSystemDirectoryHandle, name: string) {
        const raw = (await readJson<Record<string, unknown>>(dir, `${name}.json`)) ?? {};
        const { cleaned, changed } = validateOverrides(this.schema, raw);
        this.#values = cleaned;
        this.activePresetName = name;
        // Push to Rust.
        config_reset_all();
        for (const [k, v] of Object.entries(cleaned)) config_set(k, v);
        if (changed) {
            // Write the cleaned-up values back so we don't keep warning.
            await writeJson(dir, `${name}.json`, cleaned);
        }
    }

    /** Create a new preset file with the given values. */
    async #createPreset(name: string, values: Record<string, unknown>) {
        const dir = await getDir(PRESETS_DIR);
        await writeJson(dir, `${name}.json`, values);
        if (!this.userPresetNames.includes(name)) {
            this.userPresetNames = [...this.userPresetNames, name].sort();
        }
    }

    /** Switch active preset, loading its values and updating .active. */
    async #switchTo(name: string) {
        const dir = await getDir(PRESETS_DIR);
        await this.#loadIntoMemory(dir, name);
        await this.#flushActiveFile(name);
    }

    /** Replace the active preset's values entirely (used by applyTemplate). */
    async #replaceActiveValues(values: Record<string, unknown>) {
        if (!this.activePresetName) return;
        config_reset_all();
        for (const [k, v] of Object.entries(values)) config_set(k, v);
        this.#values = values;
        const dir = await getDir(PRESETS_DIR);
        await writeJson(dir, `${this.activePresetName}.json`, values);
    }

    /** Persist the .active pointer file (or remove it if name is null). */
    async #flushActiveFile(name: string | null) {
        const dir = await getDir(PRESETS_DIR);
        if (name === null) {
            await removeEntry(dir, ACTIVE_FILE);
        } else {
            await writeText(dir, ACTIVE_FILE, name);
        }
    }

    /** Schedule a debounced write of the active preset's values to disk. */
    #scheduleWrite() {
        if (!this.activePresetName) return;
        if (this.#writeTimer !== null) return;
        this.#writeTimer = setTimeout(() => {
            this.#writeTimer = null;
            const name = this.activePresetName;
            if (!name) return;
            // Snapshot to avoid races with further mutations during the write.
            const snapshot = this.#values;
            (async () => {
                try {
                    const dir = await getDir(PRESETS_DIR);
                    await writeJson(dir, `${name}.json`, snapshot);
                } catch (e) {
                    console.error('[config] preset write failed', e);
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
 * Format a tinykeys-style binding (e.g. "Shift+KeyR", "$mod+KeyA") into a
 * human-readable shortcut string (e.g. "Shift+R", "Ctrl+A" / "Cmd+A").
 */
export function formatHotkey(binding: string | undefined): string | undefined {
    if (!binding) return undefined;
    const isMac = /Mac|iPhone|iPad/.test(navigator.userAgent);
    return binding.split('+').map(part => {
        if (part === '$mod') return isMac ? '⌘' : 'Ctrl';
        if (part === 'Shift') return isMac ? '⇧' : 'Shift';
        if (part === 'Alt') return isMac ? '⌥' : 'Alt';
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
