import {
    config_get, config_set, config_reset, config_reset_all,
    config_apply_preset, config_preset_names, config_defaults,
} from '../../wasm/pkg/darkly_wasm';
import type { Preset } from './schema';

const USER_STORAGE_KEY = 'darkly-user-config';
const PRESET_KEY = 'darkly-preset';

// Preset descriptions (cosmetic, JS-only).
const PRESET_DESCRIPTIONS: Record<string, string> = {
    'Krita': 'Default Krita-style keybindings',
    'Photoshop': 'Adobe Photoshop-style keybindings',
    'GIMP': 'GIMP-style keybindings',
};

class ConfigStore {
    /** Bumped on every mutation to trigger Svelte reactivity. */
    #version = $state(0);

    /** User overrides tracked for localStorage persistence. */
    #userOverrides: Record<string, any> = {};

    /** Currently active preset name. */
    activePresetName = $state('Krita');

    /** All available presets, populated on init. */
    presets = $state<Preset[]>([]);

    /**
     * Initialize the config store. Must be called after WASM init().
     * Loads saved preset + user overrides from localStorage and pushes
     * them into the Rust config.
     */
    init() {
        // Build preset list from Rust
        const names: string[] = config_preset_names();
        this.presets = names.map(name => ({
            name,
            description: PRESET_DESCRIPTIONS[name] ?? '',
        }));

        // Load saved state from localStorage
        const savedPreset = this.#loadPresetName();
        const savedOverrides = this.#loadOverrides();

        // Apply preset layer in Rust
        this.activePresetName = savedPreset;
        config_apply_preset(savedPreset);

        // Apply user overrides on top
        this.#userOverrides = savedOverrides;
        for (const [key, value] of Object.entries(savedOverrides)) {
            config_set(key, value);
        }

        this.#version++;
    }

    /** Read a config value by dot-path key. */
    get(key: string): any {
        void this.#version;
        return config_get(key);
    }

    /** Set a user override. Persists to localStorage. */
    set(key: string, value: any) {
        config_set(key, value);
        this.#userOverrides[key] = value;
        this.#saveOverrides();
        this.#version++;
    }

    /** Remove a user override, reverting the key to preset/default. */
    resetKey(key: string) {
        config_reset(key);
        delete this.#userOverrides[key];
        this.#saveOverrides();
        this.#version++;
    }

    /** Switch to a named preset. User overrides are preserved. */
    applyPreset(name: string) {
        config_apply_preset(name);
        // Re-apply user overrides — they must win over the new preset.
        for (const [key, value] of Object.entries(this.#userOverrides)) {
            config_set(key, value);
        }
        this.activePresetName = name;
        localStorage.setItem(PRESET_KEY, name);
        this.#version++;
    }

    /** Reset everything: clear user overrides, revert to default preset. */
    reset() {
        config_reset_all();
        config_apply_preset('Krita');
        this.activePresetName = 'Krita';
        this.#userOverrides = {};
        localStorage.removeItem(USER_STORAGE_KEY);
        localStorage.removeItem(PRESET_KEY);
        this.#version++;
    }

    /** Get all default values as a flat object. */
    defaults(): Record<string, any> {
        return config_defaults();
    }

    // --- localStorage helpers ---

    #loadPresetName(): string {
        try {
            return localStorage.getItem(PRESET_KEY) ?? 'Krita';
        } catch {
            return 'Krita';
        }
    }

    #loadOverrides(): Record<string, any> {
        try {
            const raw = localStorage.getItem(USER_STORAGE_KEY);
            return raw ? JSON.parse(raw) : {};
        } catch {
            return {};
        }
    }

    #saveOverrides() {
        try {
            if (Object.keys(this.#userOverrides).length > 0) {
                localStorage.setItem(USER_STORAGE_KEY, JSON.stringify(this.#userOverrides));
            } else {
                localStorage.removeItem(USER_STORAGE_KEY);
            }
        } catch { /* storage full or unavailable */ }
    }
}

export const config = new ConfigStore();
