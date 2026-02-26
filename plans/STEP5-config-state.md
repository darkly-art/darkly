# Phase 2, Session 1 — Configuration System + App State

## Scope

Steps 1–2 from the Phase 2 plan. Build the config schema, preset system, reactive stores with localStorage persistence, hotkey infrastructure, and the AppState singleton. This is the foundation that all other Phase 2 sessions depend on.

## Prerequisites

Phase 1 complete: tiled raster layers, GPU compositing with dirty-rect optimization, modular filter system, COW undo/redo, noise filter, full WASM bridge, and end-to-end painting demo working.

## Done When

- `ProjectStore` and `UserStore` resolve correctly with defaults
- Preset switching changes hotkey values in `user.resolved`
- User overrides persist via localStorage and survive page reload
- Project config is independent from user config
- `AppState` singleton is reactive (colors, active tool, view state)
- `defu` and `tinykeys` installed

---

## Context

Phase 2 adds the user-facing shell: UI, canvas navigation, tools, config, and layer groups. The frontend is now first-class — no more throwaway scaffolding. The same engineering standard that Phase 1 applied to the Rust engine now applies to TypeScript.

**Two config scopes:**

| Scope | Persistence | Contents |
|-------|------------|----------|
| **Project** (`ProjectConfig`) | Saved with the document (Phase 2: localStorage keyed by project) | Canvas dimensions, background color |
| **User** (`UserConfig`) | `localStorage` (global, survives across documents) | Hotkeys, UI preferences, active preset |

Presets only apply to `UserConfig` — they override hotkeys, never document properties or tool state.

**Resolution order (user config):** `user overrides > active preset > defaults`.

---

## Step 1: Configuration system

### Install dependencies

```bash
cd frontend && npm install defu tinykeys
```

### `frontend/src/config/schema.ts` — Config interfaces + defaults

```typescript
// Tool hotkey imports (each tool defines its own default hotkey)
// NOTE: Tool files don't exist yet — use string constants for now.
// Session 3 will create the tool files that export these constants.

// ─── Project config (saved per document) ───

export interface ProjectConfig {
    canvas: {
        width: number;
        height: number;
        backgroundColor: string;
    };
}

export const PROJECT_DEFAULTS: ProjectConfig = {
    canvas: {
        width: 1920,
        height: 1080,
        backgroundColor: '#1a1a1a',
    },
};

// ─── User config (global, persists across documents) ───

export interface NavModifiers {
    /** Key code held to enter navigation mode (e.g. 'Space') */
    trigger: string;
    /** Modifier key for rotate while trigger is held */
    rotate: 'Shift' | 'Ctrl' | 'Alt';
    /** Modifier key for zoom while trigger is held */
    zoom: 'Shift' | 'Ctrl' | 'Alt';
}

export interface HotkeyMap {
    nav: NavModifiers;

    // Color
    resetColors: string;
    swapColors: string;

    // Edit
    undo: string;
    redo: string;

    // Tools
    brushTool: string;
    eraserTool: string;
    fillTool: string;
    gradientTool: string;
    colorPickerTool: string;

    // Brush size / opacity
    brushSizeUp: string;
    brushSizeDown: string;
    opacityUp: string;
    opacityDown: string;
}

export interface UserConfig {
    colors: {
        defaultForeground: string;
        defaultBackground: string;
    };
    ui: {
        leftSidebarWidth: number;
        rightSidebarWidth: number;
    };
    hotkeys: HotkeyMap;
}

export type DeepPartial<T> = {
    [P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];
};

export interface Preset {
    name: string;
    description: string;
    overrides: DeepPartial<UserConfig>;
}

export const USER_DEFAULTS: UserConfig = {
    colors: {
        defaultForeground: '#000000',
        defaultBackground: '#ffffff',
    },
    ui: {
        leftSidebarWidth: 48,
        rightSidebarWidth: 260,
    },
    hotkeys: {
        nav: {
            trigger: 'Space',
            rotate: 'Shift',
            zoom: 'Ctrl',
        },
        resetColors: 'KeyD',
        swapColors: 'KeyX',
        undo: '$mod+KeyZ',
        redo: '$mod+Shift+KeyZ',
        brushTool: 'KeyB',
        eraserTool: 'KeyE',
        fillTool: 'KeyF',
        gradientTool: 'KeyG',
        colorPickerTool: 'KeyP',
        brushSizeUp: 'BracketRight',
        brushSizeDown: 'BracketLeft',
        opacityUp: 'KeyO',
        opacityDown: 'KeyI',
    },
};
```

Note: Tool hotkey defaults are hardcoded in `USER_DEFAULTS` for now. When Session 3 creates the tool files with exported hotkey constants, the schema will import them. This avoids circular dependencies — the schema doesn't need to exist before the tools, and vice versa.

### `frontend/src/config/presets/krita.ts`

```typescript
import type { Preset } from '../schema';

export const PRESET_KRITA: Preset = {
    name: 'Krita',
    description: 'Default Krita-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyB',
            eraserTool: 'KeyE',
            fillTool: 'KeyF',
            gradientTool: 'KeyG',
            colorPickerTool: 'KeyP',
        },
    },
};
```

### `frontend/src/config/presets/photoshop.ts`

```typescript
import type { Preset } from '../schema';

export const PRESET_PHOTOSHOP: Preset = {
    name: 'Photoshop',
    description: 'Adobe Photoshop-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyB',
            eraserTool: 'KeyE',
            fillTool: 'KeyG',
            gradientTool: 'Shift+KeyG',
            colorPickerTool: 'KeyI',
        },
    },
};
```

### `frontend/src/config/presets/gimp.ts`

```typescript
import type { Preset } from '../schema';

export const PRESET_GIMP: Preset = {
    name: 'GIMP',
    description: 'GIMP-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyP',
            eraserTool: 'Shift+KeyE',
            fillTool: 'Shift+KeyB',
            gradientTool: 'KeyG',
            colorPickerTool: 'KeyO',
        },
    },
};
```

### `frontend/src/config/store.svelte.ts` — Reactive config stores

Two stores with separate storage keys and resolution logic. Presets only apply to user config.

```typescript
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
    const name = localStorage.getItem(PRESET_KEY);
    return (name && PRESETS[name]) || PRESET_KRITA;
}

// ─── Project config (per document) ───

class ProjectStore {
    overrides = $state<DeepPartial<ProjectConfig>>({});

    get resolved(): ProjectConfig {
        return defu(this.overrides, PROJECT_DEFAULTS) as ProjectConfig;
    }

    load(overrides: DeepPartial<ProjectConfig>) {
        this.overrides = overrides;
    }

    serialize(): DeepPartial<ProjectConfig> {
        return structuredClone(this.overrides);
    }
}

// ─── User config (global) ───

class UserStore {
    overrides = $state<DeepPartial<UserConfig>>(loadJson<UserConfig>(USER_STORAGE_KEY));
    activePreset = $state<Preset>(loadPreset());

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
```

### `frontend/src/config/hotkeys.svelte.ts` — Hotkey registration

```typescript
import { tinykeys } from 'tinykeys';
import { user } from './store.svelte';

let cleanup: (() => void) | null = null;

export function registerHotkeys(actions: Record<string, () => void>) {
    cleanup?.();

    const hotkeys = user.resolved.hotkeys;
    const bindings: Record<string, (e: KeyboardEvent) => void> = {};

    for (const [action, handler] of Object.entries(actions)) {
        const key = (hotkeys as any)[action];
        if (key && typeof key === 'string') {
            bindings[key] = (e: KeyboardEvent) => {
                e.preventDefault();
                handler();
            };
        }
    }

    cleanup = tinykeys(window, bindings);
}

export function unregisterHotkeys() {
    cleanup?.();
    cleanup = null;
}
```

### Verification

- Both stores resolve correctly with defaults
- `project.resolved` returns canvas defaults (1920x1080, #1a1a1a)
- `user.resolved` merges user overrides + preset + defaults
- Switching presets changes hotkey values
- User values persist via localStorage
- Project config is independent from user config

---

## Step 2: Application state

### `frontend/src/state/app.svelte.ts`

A reactive singleton holding global state shared across all UI components and tools.

```typescript
import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';

export interface Color {
    r: number; g: number; b: number; a: number;
}

class AppState {
    handle = $state<DarklyHandle | null>(null);

    // Colors
    foreground = $state<Color>({ r: 0, g: 0, b: 0, a: 255 });
    background = $state<Color>({ r: 255, g: 255, b: 255, a: 255 });

    // Active tool
    activeToolId = $state<string>('brush');

    // Active layer
    activeLayerId = $state<number | null>(null);

    // Tool runtime state
    brushSize = $state(24);
    brushOpacity = $state(1.0);
    fillTolerance = $state(32);
    fillAll = $state(false);
    gradientType = $state<'linear' | 'radial'>('linear');

    // View transform (controlled by canvas navigation)
    panX = $state(0);
    panY = $state(0);
    zoom = $state(1.0);
    rotation = $state(0);

    swapColors() {
        const tmp = { ...this.foreground };
        this.foreground = { ...this.background };
        this.background = tmp;
    }

    resetColors() {
        this.foreground = { r: 0, g: 0, b: 0, a: 255 };
        this.background = { r: 255, g: 255, b: 255, a: 255 };
    }
}

export const app = new AppState();
```

### Verification

- `app.foreground` / `app.background` are reactive
- `app.swapColors()` swaps correctly
- `app.resetColors()` resets to black/white
- All state fields are reactive and can be read from Svelte components

---

## Files Created This Session

```
frontend/
├── src/
│   ├── config/
│   │   ├── schema.ts
│   │   ├── presets/
│   │   │   ├── krita.ts
│   │   │   ├── photoshop.ts
│   │   │   └── gimp.ts
│   │   ├── store.svelte.ts
│   │   └── hotkeys.svelte.ts
│   └── state/
│       └── app.svelte.ts
```
