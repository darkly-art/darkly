import { defu } from 'defu';
import {
    PROJECT_DEFAULTS, USER_DEFAULTS,
    type ProjectConfig, type UserConfig, type DeepPartial, type Preset,
} from './schema';
import { PRESET_KRITA } from './presets/krita';
import { PRESET_PHOTOSHOP } from './presets/photoshop';
import { PRESET_GIMP } from './presets/gimp';

const USER_STORAGE_KEY = 'darkly-user-config';
const PRESET_KEY = 'darkly-preset';

const PRESETS: Record<string, Preset> = {
    'Krita': PRESET_KRITA,
    'Photoshop': PRESET_PHOTOSHOP,
    'GIMP': PRESET_GIMP,
};

function loadJson<T>(key: string): DeepPartial<T> {
    try {
        const raw = localStorage.getItem(key);
        return raw ? JSON.parse(raw) : {};
    } catch { return {} as DeepPartial<T>; }
}

function loadPreset(): Preset {
    try {
        const name = localStorage.getItem(PRESET_KEY);
        return (name && PRESETS[name]) || PRESET_KRITA;
    } catch { return PRESET_KRITA; }
}

// --- Project config (per document) ---

class ProjectStore {
    overrides = $state<DeepPartial<ProjectConfig>>({});

    get resolved(): ProjectConfig {
        return defu(this.overrides, PROJECT_DEFAULTS) as ProjectConfig;
    }

    /** Load project config from a document (future: from .darkly file).
     *  Phase 2: called with {} on new document. */
    load(overrides: DeepPartial<ProjectConfig>) {
        this.overrides = overrides;
    }

    /** Serialize project config for saving with the document. */
    serialize(): DeepPartial<ProjectConfig> {
        return structuredClone(this.overrides);
    }
}

// --- User config (global) ---

class UserStore {
    overrides = $state<DeepPartial<UserConfig>>(loadJson<UserConfig>(USER_STORAGE_KEY));
    activePreset = $state<Preset>(loadPreset());

    /** Resolved config: user overrides > active preset > defaults */
    get resolved(): UserConfig {
        return defu(
            this.overrides,
            this.activePreset.overrides,
            USER_DEFAULTS
        ) as UserConfig;
    }

    get availablePresets(): Preset[] {
        return Object.values(PRESETS);
    }

    applyPreset(preset: Preset) {
        this.activePreset = preset;
        localStorage.setItem(PRESET_KEY, preset.name);
    }

    setOverride(path: string, value: any) {
        const parts = path.split('.');
        let obj: any = this.overrides;
        for (let i = 0; i < parts.length - 1; i++) {
            if (!obj[parts[i]]) obj[parts[i]] = {};
            obj = obj[parts[i]];
        }
        obj[parts[parts.length - 1]] = value;
        localStorage.setItem(USER_STORAGE_KEY, JSON.stringify(this.overrides));
    }

    reset() {
        this.overrides = {};
        this.activePreset = PRESET_KRITA;
        localStorage.removeItem(USER_STORAGE_KEY);
        localStorage.removeItem(PRESET_KEY);
    }
}

export const project = new ProjectStore();
export const user = new UserStore();
