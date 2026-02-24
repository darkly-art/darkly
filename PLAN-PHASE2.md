# Darkly Phase 2 — UI, Canvas Navigation, Configuration & Layer Groups

## Context

Phase 1 established the core engine: tiled raster layers, GPU compositing with dirty-rect optimization, a modular filter system, and COW undo/redo. Phase 2 adds the user-facing shell: a minimal but properly engineered UI, canvas pan/zoom/rotate, a modular tool system, a preset-based configuration system, and layer groups with passthrough compositing.

**Engineering principle (evolved from Phase 1):** Every system that is implemented must be implemented properly. No hacks, no shortcuts, no hardcoding — in Rust or TypeScript. Phase 1 allowed the frontend to cut corners because it was throwaway scaffolding. Phase 2's frontend *is* the product: the UI, tools, config system, and navigation are all first-class systems that must be built correctly on the first iteration. The same standard that Phase 1 applied to the engine now applies to everything.

**Phase 2 scope:**
- Left sidebar: color picker + tool buttons
- Right sidebar: layer panel with groups, reordering via drag-and-drop
- Canvas navigation: pan (two-finger scroll or Space+drag), rotate (Shift+Space+drag), zoom (ctrl+scroll or Ctrl+Space+drag)
- Modular tool system with per-tool configuration (brush, fill, gradient, eraser, color picker)
- Configuration system with presets (Krita default, Photoshop, GIMP hotkey mappings)
- Layer groups with passthrough compositing mode
- View transform uniform in the present shader

---

## Architecture Decisions

### Configuration System

**Approach:** Custom system built from minimal pieces — no heavy config library.

| Component | Solution | Size |
|-----------|----------|------|
| Schema & defaults | TypeScript interfaces + const objects | 0 kB |
| Preset overlay | `defu` (recursive defaults merge) | ~1 kB |
| Hotkey binding | `tinykeys` (modern, `KeyboardEvent.code`) | ~650 B |
| Reactivity | Svelte 5 `$state` class in `.svelte.ts` | 0 kB |
| Persistence | `localStorage` + JSON | 0 kB |
| **Total added deps** | | **~1.7 kB** |

**Why not a library:** Existing config libraries (convict, nconf, electron-store) solve server/Node.js problems (env vars, CLI args, filesystem). Darkly needs typed defaults + preset layering + Svelte reactivity + browser storage — all simple, composable problems that a 100-line custom system handles better than any off-the-shelf solution.

**Two config scopes:**

| Scope | Persistence | Contents |
|-------|------------|----------|
| **Project** (`ProjectConfig`) | Saved with the document (future: embedded in `.darkly` file; Phase 2: `localStorage` keyed by project) | Canvas dimensions, background color |
| **User** (`UserConfig`) | `localStorage` (global, survives across documents) | Hotkeys, UI preferences, active preset |

Presets only apply to `UserConfig` — they override hotkeys, never document properties or tool state.

**Resolution order (user config):** `user overrides > active preset > defaults`. The `defu` utility handles recursive merging with leftmost-wins semantics: `defu(userOverrides, presetOverrides, USER_DEFAULTS)`.

**Tool state:** Each tool declares its own runtime properties (brush size, fill tolerance, gradient type, etc.) as mutable state in its module file, with sensible initial values as constants. These are working values the user adjusts while painting — not persistent config. They serialize with the project so reopening a document restores the tool state you were working with. Each tool file also exports its default hotkey, which the schema imports for the hotkey map.

### Canvas View Transform

The present shader currently maps 1:1 pixel coordinates. Phase 2 adds a **view transform uniform** to the present pipeline that applies pan, zoom, and rotation. The compositing pipeline is unchanged — compositing still happens in canvas-pixel space. Only the final blit to the screen is transformed.

This means:
- No GPU object reallocation when the view changes (just a uniform buffer write)
- Compositing still uses scissor-rect optimization (dirty rects are in canvas space, unaffected by view)
- The present shader applies a 3x3 affine matrix to map canvas pixels to screen pixels

Mouse coordinates from the browser must be inverse-transformed from screen space back to canvas space before being passed to paint operations.

### Tool System

Informed by analysis of Krita's tool architecture (see `KRITA-TOOL-ARCHITECTURE.md`), Darkly's tool system is split across two layers:

**What we take from Krita:**
- Semantic stroke lifecycle (`begin`/`continue`/`end`) as the tool interface — tools interpret gestures, not raw input events
- Stroke-as-unit-of-work for undo — the stroke boundary handles transactions; tools never call begin/commit transaction directly
- Different execution strategies for different tool types — brush is incremental (per-point), fill is one-shot, gradient is two-point
- Tool registry with metadata (id, name, icon, shortcut)
- Tools don't write pixels directly — they call through Document operations that handle dirty tracking and undo

**What we don't take:**
- Two-layer base class hierarchy (KoToolBase/KisTool split is KOffice legacy — one trait is enough)
- Plugin loading / dynamic factories (static registration; all tools compiled in)
- Async stroke queue / worker threads (WASM is single-threaded; our operations are cheap; GPU does the heavy work)
- Event routing proxy (KoToolProxy is needless indirection for single-tool-at-a-time)
- `KisResourcesSnapshot` as a separate object (just capture the few values we need in the stroke params)
- PaintOp plugin system (we have one painting method; brush engines are a future concern)
- Interaction strategies (no vector tools yet)

**Rust side (`darkly`):** Tool operations follow the same modular auto-discovery pattern as filters. Each tool op is a self-contained file in `tools/` that defines its param struct, an `apply()` method, and a `register()` function. The `build.rs` script generates `tools/mod.rs` with a `ToolRegistry` that maps type ID strings to factory functions — no central enum, no central match. The WASM bridge exposes a stroke lifecycle: `begin_stroke(layer_id)` → `stroke_to(op_type, params)` → `end_stroke()`, where begin/end map to undo transactions. The `stroke_to` call looks up the tool op in the registry and delegates to it.

Adding a new tool operation means creating one `.rs` file in `tools/`. No other file needs editing.

**TypeScript side:** Thin gesture interpreters. Each tool implements a `Tool` interface (`onPointerDown`/`Move`/`Up`) that translates pointer events + app state into WASM bridge calls. The TS tool is responsible for input interpretation (what does this drag mean?), the Rust side is responsible for pixel modification (what happens to the tiles?).

**The split:**
| Concern | Where |
|---------|-------|
| Gesture interpretation (what does the input mean?) | TypeScript `Tool` |
| Coordinate transform (screen → canvas) | TypeScript (calls `handle.screen_to_canvas()`) |
| Tool switching, hotkeys, UI options panels | TypeScript |
| Stroke lifecycle (begin transaction / end transaction) | Rust `DarklyHandle` |
| Pixel modification (painting, filling, gradient) | Rust `Document` |
| Dirty tracking | Rust `Document` |
| Undo integration | Rust (transaction at stroke boundary) |

### Layer Groups

Layer groups are added to `darkly` as a new variant `Layer::Group(LayerGroup)`. Passthrough mode (default, matching Photoshop behavior) means groups are treated as organizational containers only — the compositor flattens the tree and composites layers in display order, ignoring group boundaries.

Non-passthrough mode (normal blending group) would isolate the group's composite result and blend it into the parent as a single unit. Phase 2 implements passthrough only; the data model supports both.

---

## Project Structure (Phase 2 additions)

```
darkly/
├── crates/
│   └── darkly/
│       └── src/
│           ├── layer.rs              # + LayerGroup, LayerNode tree structure
│           ├── document.rs           # + tree traversal, group operations, reorder
│           ├── tool.rs               # ToolOp trait, ToolRegistration, ToolRegistry
│           ├── color.rs              # Color types (used by tools via WASM bridge)
│           ├── tools/                # Self-contained tool ops, one file per op
│           │   ├── mod.rs            # @generated by build.rs — auto-discovers tool ops
│           │   ├── paint_circle.rs   # PaintCircleOp: register + params + apply
│           │   ├── erase_circle.rs   # EraseCircleOp: register + params + apply
│           │   ├── flood_fill.rs     # FloodFillOp: register + params + apply
│           │   └── gradient.rs       # LinearGradientOp: register + params + apply
│           └── gpu/
│               ├── compositor.rs     # + view transform uniform, flatten groups
│               └── view.rs           # ViewTransform: pan, zoom, rotate matrix
│
├── frontend/
│   ├── src/
│   │   ├── App.svelte                # Layout shell: left sidebar + canvas + right sidebar
│   │   ├── canvas/
│   │   │   ├── CanvasView.svelte     # Canvas element + pointer handlers + navigation
│   │   │   └── navigation.svelte.ts  # Pan/zoom/rotate state machine
│   │   ├── config/
│   │   │   ├── schema.ts             # ProjectConfig + UserConfig interfaces + defaults
│   │   │   ├── presets/
│   │   │   │   ├── krita.ts          # Krita hotkey preset (default)
│   │   │   │   ├── photoshop.ts      # Photoshop hotkey preset
│   │   │   │   └── gimp.ts           # GIMP hotkey preset
│   │   │   ├── store.svelte.ts       # ProjectStore + UserStore: reactive config with persistence
│   │   │   └── hotkeys.svelte.ts     # Hotkey registration via tinykeys
│   │   ├── state/
│   │   │   └── app.svelte.ts         # AppState: colors, active tool, active layer, handle
│   │   ├── tools/
│   │   │   ├── registry.ts           # ToolRegistry + Tool interface (gesture interpreters)
│   │   │   ├── brush.svelte.ts       # BrushTool: translates drag → stroke_to(PaintCircle)
│   │   │   ├── eraser.svelte.ts      # EraserTool: translates drag → stroke_to(EraseCircle)
│   │   │   ├── fill.svelte.ts        # FillTool: translates click → stroke_to(FloodFill)
│   │   │   ├── gradient.svelte.ts    # GradientTool: captures start+end → stroke_to(Gradient)
│   │   │   ├── colorpicker.svelte.ts # ColorPickerTool: samples color (no stroke)
│   │   │   └── ToolOptions.svelte    # Dynamic options panel for active tool
│   │   ├── ui/
│   │   │   ├── LeftSidebar.svelte    # Color picker widget + tool buttons
│   │   │   ├── RightSidebar.svelte   # Layer panel container
│   │   │   ├── ColorPicker.svelte    # HSV color picker + primary/secondary swatches
│   │   │   └── layers/
│   │   │       ├── LayerPanel.svelte # Layer list with groups, drag-and-drop reorder
│   │   │       ├── LayerItem.svelte  # Single layer row (thumbnail, name, visibility, opacity)
│   │   │       └── LayerGroup.svelte # Collapsible group with indent
│   │   └── editor.ts                # + init config, register hotkeys
│   └── wasm/
│       └── src/
│           └── api.rs                # + stroke lifecycle, view transform, layer tree ops
│
└── shaders/
    └── present.wgsl                  # + view transform matrix uniform
```

---

## Krita Default Hotkeys Reference

The following is the comprehensive Krita default shortcut map, extracted from `krita/krita/data/shortcuts/krita_default.shortcuts`. Shortcuts marked with `[Phase 2]` are implemented in this phase. All others are preserved as comments for future reference.

```
# ──────────────────────────────────────────────────────────
# Canvas Navigation (input profile, not shortcut system)
# ──────────────────────────────────────────────────────────
# Space+Drag           Pan canvas                          [Phase 2]
# Shift+Space+Drag     Rotate canvas                       [Phase 2]
# Ctrl+Space+Drag      Zoom canvas                         [Phase 2]
# Ctrl+[               Rotate canvas left 15°
# Ctrl+]               Rotate canvas right 15°
# M                    Mirror view (horizontal flip)

# ──────────────────────────────────────────────────────────
# Color
# ──────────────────────────────────────────────────────────
# D                    Reset foreground/background to B/W   [Phase 2]
# X                    Swap foreground/background            [Phase 2]
# K                    Make brush color darker
# L                    Make brush color lighter
# H                    Show color history
# U                    Show common colors
# Shift+I              Show color selector
# Shift+N              Show minimal shade selector

# ──────────────────────────────────────────────────────────
# Edit / Undo
# ──────────────────────────────────────────────────────────
# Ctrl+Z               Undo                                 [Phase 2]
# Ctrl+Shift+Z         Redo                                 [Phase 2]
# Ctrl+C               Copy
# Ctrl+X               Cut
# Ctrl+V               Paste
# Ctrl+A               Select all
# Ctrl+Shift+A         Deselect
# Shift+Backspace      Fill with foreground color
# Backspace            Fill with background color

# ──────────────────────────────────────────────────────────
# Tools
# ──────────────────────────────────────────────────────────
# B                    Freehand Brush
# E                    Eraser mode toggle
# G                    Gradient Tool
# F                    Fill Tool
# P                    Color Sampler (eyedropper)
# T                    Move Tool
# Q                    Multibrush Tool
# C                    Crop Tool
# Shift+R              Rectangle Tool
# Shift+J              Ellipse Tool

# ──────────────────────────────────────────────────────────
# Brush / Size / Opacity
# ──────────────────────────────────────────────────────────
# [                    Decrease brush size
# ]                    Increase brush size
# I                    Decrease opacity
# O                    Increase opacity
# ,                    Previous favorite preset
# .                    Next favorite preset
# /                    Switch to previous preset

# ──────────────────────────────────────────────────────────
# Layers
# ──────────────────────────────────────────────────────────
# Ins                  Add new paint layer
# Shift+Ins            Add new group layer
# PgUp                 Activate next layer
# PgDown               Activate previous layer
# Ctrl+J               Duplicate layer
# Ctrl+E               Merge down
# Shift+Del            Remove layer
# Ctrl+PgUp            Move layer up
# Ctrl+PgDown          Move layer down
# F2                   Rename layer
# F3                   Layer properties

# ──────────────────────────────────────────────────────────
# View / Zoom
# ──────────────────────────────────────────────────────────
# Ctrl+=  (Ctrl++)     Zoom in
# Ctrl+-               Zoom out
# Ctrl+0               Zoom to 100%
# Tab                  Canvas-only mode (hide UI)
# Ctrl+Shift+'         Toggle grid
# Ctrl+Shift+;         Snap to grid
# Ctrl+Shift+F         Fullscreen

# ──────────────────────────────────────────────────────────
# Selection
# ──────────────────────────────────────────────────────────
# Ctrl+Shift+A         Deselect / Select none
# Ctrl+Shift+D         Reselect
# Ctrl+Shift+I         Invert selection
# Shift+F6             Feather selection
# Ctrl+H               Toggle display selection
# Ctrl+Alt+J           Copy selection to new layer
# Ctrl+Shift+J         Cut selection to new layer

# ──────────────────────────────────────────────────────────
# Filters / Adjustments
# ──────────────────────────────────────────────────────────
# Ctrl+F               Apply filter again
# Ctrl+B               Color balance
# Ctrl+U               HSV adjustment
# Ctrl+I               Invert
# Ctrl+L               Levels
# Ctrl+M               Curves (per-channel)
# Ctrl+Shift+U         Desaturate

# ──────────────────────────────────────────────────────────
# File
# ──────────────────────────────────────────────────────────
# Ctrl+N               New
# Ctrl+O               Open
# Ctrl+S               Save
# Ctrl+Shift+S         Save as
# Ctrl+W               Close
# Ctrl+Q               Quit
# Ctrl+P               Print
# Ctrl+Shift+E         Flatten image

# ──────────────────────────────────────────────────────────
# Panels / Misc
# ──────────────────────────────────────────────────────────
# F1                   Help
# F4                   Save incremental backup
# F5                   Brush Editor
# F6                   Brush Presets
# \                    Tool Options
# Ctrl+Return          Search Actions (command palette)
```

---

## Implementation Steps

### Step 1: Configuration system

Build the config schema, preset system, and reactive store before anything else — all other systems depend on it.

**`frontend/src/config/schema.ts` — Config interfaces + defaults:**

```typescript
// ─── Tool hotkey imports (each tool defines its own default hotkey) ───
import { BRUSH_HOTKEY } from '../tools/brush.svelte';
import { ERASER_HOTKEY } from '../tools/eraser.svelte';
import { FILL_HOTKEY } from '../tools/fill.svelte';
import { GRADIENT_HOTKEY } from '../tools/gradient.svelte';
import { COLORPICKER_HOTKEY } from '../tools/colorpicker.svelte';

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

export interface HotkeyMap {
    // Canvas navigation (modifier+drag combos handled by navigation state machine,
    // not tinykeys — but listed here for preset customization)
    panModifier: string;
    rotateModifier: string;
    zoomModifier: string;

    // Color
    resetColors: string;
    swapColors: string;

    // Edit
    undo: string;
    redo: string;

    // Tools — default values sourced from each tool module
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
        defaultForeground: string;  // hex
        defaultBackground: string;  // hex
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
        panModifier: 'Space',
        rotateModifier: 'Shift+Space',
        zoomModifier: 'Ctrl+Space',
        resetColors: 'KeyD',
        swapColors: 'KeyX',
        undo: '$mod+KeyZ',
        redo: '$mod+Shift+KeyZ',
        // Tool hotkeys — sourced from each tool module
        brushTool: BRUSH_HOTKEY,
        eraserTool: ERASER_HOTKEY,
        fillTool: FILL_HOTKEY,
        gradientTool: GRADIENT_HOTKEY,
        colorPickerTool: COLORPICKER_HOTKEY,
        brushSizeUp: 'BracketRight',
        brushSizeDown: 'BracketLeft',
        opacityUp: 'KeyO',
        opacityDown: 'KeyI',
    },
};
```

**`frontend/src/config/presets/krita.ts`:**

```typescript
import type { Preset } from '../schema';  // Preset.overrides is DeepPartial<UserConfig>

export const PRESET_KRITA: Preset = {
    name: 'Krita',
    description: 'Default Krita-style keybindings',
    overrides: {
        // Krita defaults match our USER_DEFAULTS, so overrides are minimal.
        // This preset exists so switching back from Photoshop/GIMP restores Krita bindings.
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

**`frontend/src/config/presets/photoshop.ts`:**

```typescript
import type { Preset } from '../schema';  // Preset.overrides is DeepPartial<UserConfig>

export const PRESET_PHOTOSHOP: Preset = {
    name: 'Photoshop',
    description: 'Adobe Photoshop-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyB',
            eraserTool: 'KeyE',       // Photoshop uses E directly
            fillTool: 'KeyG',          // Photoshop: G = paint bucket (fill group)
            gradientTool: 'Shift+KeyG', // Photoshop: Shift+G cycles fill/gradient
            colorPickerTool: 'KeyI',    // Photoshop: I = eyedropper
        },
    },
};
```

**`frontend/src/config/presets/gimp.ts`:**

```typescript
import type { Preset } from '../schema';  // Preset.overrides is DeepPartial<UserConfig>

export const PRESET_GIMP: Preset = {
    name: 'GIMP',
    description: 'GIMP-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyP',          // GIMP: P = paintbrush
            eraserTool: 'Shift+KeyE',   // GIMP: Shift+E
            fillTool: 'Shift+KeyB',     // GIMP: Shift+B = bucket fill
            gradientTool: 'KeyG',       // GIMP: G = gradient
            colorPickerTool: 'KeyO',    // GIMP: O = color picker
        },
    },
};
```

**`frontend/src/config/store.svelte.ts` — Reactive config stores:**

Two stores: one for per-document project settings, one for global user preferences. Separate storage keys, separate resolution logic. Presets only apply to user config.

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

// ─── User config (global) ───

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
```

**`frontend/src/config/hotkeys.svelte.ts` — Hotkey registration:**

```typescript
import { tinykeys } from 'tinykeys';
import { user } from './store.svelte';

let cleanup: (() => void) | null = null;

/**
 * Register all hotkeys from the resolved user config.
 * Call on init and whenever the preset changes.
 * `actions` maps HotkeyMap key names to handler functions.
 */
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

**Install deps:**
```bash
cd frontend && npm install defu tinykeys
```

**Verification:** Both stores resolve correctly. `project.resolved` returns canvas defaults. `user.resolved` merges user overrides + preset + defaults. Switching presets changes hotkey values. User values persist via localStorage. Project config is independent.

---

### Step 2: Application state

A reactive singleton that holds the global state shared across all UI components and tools.

**`frontend/src/state/app.svelte.ts`:**

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

    // Tool runtime state — working values adjusted while painting.
    // Serialized with the project so reopening a doc restores tool state.
    brushSize = $state(24);
    brushOpacity = $state(1.0);
    fillTolerance = $state(32);     // 0–255
    fillAll = $state(false);
    gradientType = $state<'linear' | 'radial'>('linear');

    // View transform (controlled by canvas navigation)
    panX = $state(0);
    panY = $state(0);
    zoom = $state(1.0);
    rotation = $state(0);   // radians

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

---

### Step 3: Canvas view transform (Rust + shader)

Add a view transform to the present pipeline so the canvas can be panned, zoomed, and rotated without affecting compositing.

**`crates/darkly/src/gpu/view.rs`:**

```rust
/// 2D view transform for canvas navigation.
/// Compositing happens in canvas-pixel space. This transform is applied
/// only in the present shader to map canvas pixels to screen pixels.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewTransform {
    /// Inverse view matrix (screen → canvas), stored as 3 vec4s for std140.
    /// The third element of rows 0 and 1 carries the unpadded canvas
    /// dimensions — the present shader needs these for its OOB check
    /// (see "tile-padded textures" note below).
    /// Row 0: [m00, m01, canvas_w, 0]
    /// Row 1: [m10, m11, canvas_h, 0]
    /// Row 2: [tx,  ty,  1,        0]
    pub matrix: [[f32; 4]; 3],
}

impl ViewTransform {
    pub fn identity() -> Self {
        ViewTransform {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        }
    }

    /// Build the inverse view matrix (screen → canvas) from pan/zoom/rotation.
    /// The forward transform is: canvas → screen
    ///   1. Translate by -canvas_center
    ///   2. Scale by zoom
    ///   3. Rotate by rotation
    ///   4. Translate by screen_center + pan
    ///
    /// The present shader needs the inverse: screen → canvas.
    pub fn from_pan_zoom_rotate(
        pan_x: f32, pan_y: f32,
        zoom: f32, rotation: f32,       // radians
        screen_w: f32, screen_h: f32,
        canvas_w: f32, canvas_h: f32,
    ) -> Self {
        let cos_r = rotation.cos();
        let sin_r = rotation.sin();
        let inv_zoom = 1.0 / zoom;

        let cx = canvas_w / 2.0;
        let cy = canvas_h / 2.0;
        let sx = screen_w / 2.0 + pan_x;
        let sy = screen_h / 2.0 + pan_y;

        // Inverse: undo translate, undo rotate, undo scale, undo center
        let m00 = cos_r * inv_zoom;
        let m01 = sin_r * inv_zoom;
        let m10 = -sin_r * inv_zoom;
        let m11 = cos_r * inv_zoom;
        let tx = cx - m00 * sx - m10 * sy;
        let ty = cy - m01 * sx - m11 * sy;

        ViewTransform {
            matrix: [
                [m00, m01, canvas_w, 0.0],
                [m10, m11, canvas_h, 0.0],
                [tx,  ty,  1.0,      0.0],
            ],
        }
    }

    /// Transform a screen point to canvas coordinates using the stored inverse matrix.
    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        let m = &self.matrix;
        let cx = m[0][0] * screen_x + m[1][0] * screen_y + m[2][0];
        let cy = m[0][1] * screen_x + m[1][1] * screen_y + m[2][1];
        (cx, cy)
    }
}
```

**Compositor changes (`compositor.rs`):**

Add a `view_uniform_buf: wgpu::Buffer` created once in `Compositor::new()`. The present bind group layout gains a third entry (binding 2, uniform buffer). Add `pub fn update_view_transform(&self, queue, transform)` that calls `queue.write_buffer()`. Presenting with a view change only needs `mark_dirty()` to trigger a re-present — compositing is skipped if only the view changed (no dirty tiles).

**Optimization — view-only changes:**

When only the view transform changes (no dirty tiles, no layer property changes), the compositor should skip offscreen compositing and only re-run the present pass. Add a `needs_present: bool` flag separate from `needs_composite`. The `update_view_transform` method sets `needs_present = true` without setting `needs_composite`. The render loop checks: if `!needs_composite && needs_present`, skip all tile upload and compositing, and only run the present pass from the existing `composite_cache`.

**Updated `shaders/present.wgsl`:**

**Tile-padded textures vs canvas dimensions:** Layer textures and accum buffers are padded to tile boundaries (e.g. a 900px-wide canvas becomes a 960px texture at TILE_SIZE=64). The present shader must handle two different "sizes": the padded texture size for UV sampling (so texels map 1:1 to canvas pixels), and the actual canvas dimensions for the OOB check (so the padding area shows workspace background, not black). The canvas dimensions are packed into the ViewTransform uniform (row0.z = canvas_w, row1.z = canvas_h). Using `textureDimensions()` alone for both would show a black bar in the padding region; using canvas dims alone for both would stretch the image.

```wgsl
struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

struct ViewTransform {
    row0: vec4f,
    row1: vec4f,
    row2: vec4f,
}

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> view: ViewTransform;

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    // Transform screen pixel → canvas pixel using the inverse view matrix
    let screen_pos = in.position.xy;
    let canvas_x = view.row0.x * screen_pos.x + view.row1.x * screen_pos.y + view.row2.x;
    let canvas_y = view.row0.y * screen_pos.x + view.row1.y * screen_pos.y + view.row2.y;

    // Sample using the padded texture size so texels map 1:1 to canvas pixels.
    let tex_dims = vec2f(textureDimensions(t_source));
    let uv = vec2f(canvas_x, canvas_y) / tex_dims;
    let clamped_uv = clamp(uv, vec2f(0.0), vec2f(1.0));
    let color = textureSample(t_source, t_sampler, clamped_uv);

    // OOB check uses actual canvas dimensions (unpadded) so the tile
    // padding area shows as workspace background, not black.
    let canvas_dims = vec2f(view.row0.z, view.row1.z);
    let oob = canvas_x < 0.0 || canvas_x > canvas_dims.x
           || canvas_y < 0.0 || canvas_y > canvas_dims.y;
    let bg = vec4f(0.11, 0.11, 0.11, 1.0);
    return select(vec4f(color.rgb, 1.0), bg, oob);
}
```

**WASM bridge additions (`api.rs`):**

**Document size is decoupled from viewport:** `DarklyHandle::create` accepts explicit `doc_width`/`doc_height` parameters for the document dimensions. The viewport size comes from the HTML canvas element (sized to CSS layout × DPR via `ResizeObserver`). The document dimensions are passed to `Document::new()` and `Compositor::new()`, while the viewport dimensions only affect the GPU surface. This means the document can be any aspect ratio (e.g. 900×1600 portrait) regardless of the browser window shape.

```rust
/// Create a new Darkly editor instance, initializing GPU and document.
/// `doc_width`/`doc_height` set the document (canvas) dimensions;
/// the viewport size comes from the HTML canvas element.
pub async fn create(canvas: web_sys::HtmlCanvasElement, doc_width: u32, doc_height: u32) -> DarklyHandle {
    let gpu = GpuContext::new(canvas).await;
    let compositor = Compositor::new(&gpu.device, &gpu.queue, gpu.surface_format(), doc_width, doc_height);
    let doc = Document::new(doc_width, doc_height);
    // ...
}

/// Update the canvas view transform (pan, zoom, rotation).
pub fn set_view_transform(
    &mut self,
    pan_x: f32, pan_y: f32,
    zoom: f32, rotation: f32,
    screen_w: f32, screen_h: f32,
) {
    let transform = ViewTransform::from_pan_zoom_rotate(
        pan_x, pan_y, zoom, rotation,
        screen_w, screen_h,
        self.doc.width as f32, self.doc.height as f32,
    );
    self.view_transform = transform;
    self.compositor.update_view_transform(&self.gpu.queue, &transform);
    self.compositor.mark_needs_present();
}

/// Transform screen coordinates to canvas coordinates for paint input.
pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
    let (cx, cy) = self.view_transform.screen_to_canvas(screen_x, screen_y);
    vec![cx, cy]
}
```

**Coordinate space note:** The canvas buffer is sized to match its CSS layout at `devicePixelRatio` (not a fixed resolution like 1920×1080). A `ResizeObserver` keeps the buffer and GPU surface in sync with the element size via `handle.resize()`. The `set_view_transform` call receives `canvas.width`/`canvas.height` (the actual buffer dimensions). Pan values stored in CSS pixels are scaled by `devicePixelRatio` before passing to the GPU. The `cssToBuffer` helper simply multiplies by DPR — no letterbox offset math needed since the buffer matches the element 1:1.

**Verification:** Call `set_view_transform` with non-identity values. Canvas appears panned/zoomed/rotated. Painting still works correctly (mouse coordinates inverse-transformed). View-only changes (no painting) skip compositing and only re-present.

---

### Step 4: Canvas navigation state machine

Handle Space+drag (pan), Shift+Space+drag (rotate), Ctrl+Space+drag (zoom) as a proper state machine in the frontend.

**`frontend/src/canvas/navigation.svelte.ts`:**

See `frontend/src/canvas/navigation.svelte.ts` for the implementation. Key design decisions:

- **Rotation is Krita-style angular**, not linear pixel mapping. On pointer down, the angle from the canvas center to the cursor is measured with `atan2()`. On move, the new angle is measured and the rotation delta is the difference. This feels like physically grabbing and spinning the canvas.
- **Rotation pivot must account for pan.** The view transform rotates around the canvas center, which appears on screen at `element_center + pan`. The angular rotation gesture must measure angles from this same point — `rect.left + rect.width/2 + panX`, not just `rect.left + rect.width/2`. Using the raw element center causes the rotation pivot to drift after panning.
- **Zoom uses vertical drag** (dy), not horizontal. Drag up = zoom out, drag down = zoom in. Exponential scaling (`Math.pow(2, -dy / 150)`).
- **Pan is straightforward** 1:1 CSS pixel mapping of dx/dy.
- **`onPointerDown` accepts the canvas element** so rotation can compute the canvas center for the angular measurement.
- **Scroll zoom is cursor-centered**: adjusts pan so the point under the cursor stays fixed after zoom.

**`frontend/src/canvas/CanvasView.svelte`:**

The current `App.svelte` canvas+mouse logic moves here. Pointer event flow:

1. Navigation state machine gets first chance (`nav.onPointerDown(e)`)
2. If navigation consumed the event, skip tool dispatch
3. Otherwise, transform screen coords → canvas coords via `handle.screen_to_canvas()`
4. Dispatch to active tool's `onPointerDown(ctx, e, canvasX, canvasY)`

View transform is updated on every `$effect` that watches `app.panX`, `app.panY`, `app.zoom`, `app.rotation`. Pan values (CSS pixels) are scaled by `devicePixelRatio` to buffer space:

```typescript
$effect(() => {
    if (app.handle && canvas) {
        const dpr = window.devicePixelRatio || 1;
        app.handle.set_view_transform(
            app.panX * dpr, app.panY * dpr,
            app.zoom, app.rotation,
            canvas.width, canvas.height,
        );
    }
});
```

`cssToBuffer` simply scales by DPR (no letterbox math — the buffer matches the element):

```typescript
function cssToBuffer(cssLocalX: number, cssLocalY: number) {
    const dpr = window.devicePixelRatio || 1;
    return { x: cssLocalX * dpr, y: cssLocalY * dpr };
}
```

**Verification:** Space+drag pans the canvas. Shift+Space+drag rotates. Ctrl+Space+drag zooms. Ctrl+scroll zooms. Painting still works correctly after transform.

---

### Step 5: Tool system

The tool system is split: Rust owns pixel modification and undo; TypeScript owns gesture interpretation and UI.

#### 5a: Rust side — self-contained tool operation modules

The tool system follows the exact same auto-discovery pattern as filters (see modularity principle in AGENTS.md):

| Filter system | Tool system | Role |
|--------------|-------------|------|
| `gpu/filter.rs` — `Filter` trait + `FilterRegistration` + `FilterRegistry` | `tool.rs` — `ToolOp` trait + `ToolRegistration` + `ToolRegistry` | Trait + registry |
| `gpu/filters/mod.rs` — @generated by build.rs | `tools/mod.rs` — @generated by build.rs | Auto-discovered modules |
| `gpu/filters/noise.rs` — `register()` + struct + `Filter` impl | `tools/paint_circle.rs` — `register()` + struct + `apply()` | One self-contained file per module |

Each tool operation is a self-contained file in `tools/` that defines its param struct, an `apply(&self, doc, layer_id)` method, and a `register()` function returning a `ToolRegistration`. The `build.rs` script auto-discovers all `.rs` files in `tools/` and generates `mod.rs`.

**Adding a new tool operation requires:** Create one `.rs` file in `tools/`. Nothing else.

**`crates/darkly/src/tool.rs` — Tool operation trait + registry:**

```rust
use crate::document::Document;
use crate::layer::LayerId;
use std::collections::HashMap;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

/// What each tool op module returns from its `register()` function.
pub struct ToolRegistration {
    pub type_id: &'static str,
    #[cfg(target_arch = "wasm32")]
    pub from_js: fn(JsValue) -> Box<dyn ToolOp>,
}

/// A discrete operation that a tool applies to the document.
/// Each tool op struct implements this trait.
pub trait ToolOp {
    fn apply(&self, doc: &mut Document, layer_id: LayerId);
}

/// Auto-discovered tool operation registry.
/// Built from the generated `tools::registrations()` at construction time.
pub struct ToolRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    #[cfg(target_arch = "wasm32")]
    from_js: fn(JsValue) -> Box<dyn ToolOp>,
}

impl ToolRegistry {
    pub fn new() -> Self; // populates from tools::registrations()

    /// Deserialize a JS params object into a tool op and apply it.
    #[cfg(target_arch = "wasm32")]
    pub fn apply(&self, type_id: &str, js: JsValue,
                 doc: &mut Document, layer_id: LayerId) {
        let entry = &self.entries[type_id];
        let op = (entry.from_js)(js);
        op.apply(doc, layer_id);
    }
}
```

**`crates/darkly/src/tools/mod.rs` — @generated by build.rs (never edit manually).**

**`crates/darkly/src/tools/paint_circle.rs` — Brush dab operation (fully self-contained):**

```rust
use crate::tool::{ToolOp, ToolRegistration};
use crate::document::Document;
use crate::layer::{Layer, LayerId};
use crate::tile::TILE_SIZE;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "paint_circle",
        #[cfg(target_arch = "wasm32")]
        from_js: |js| {
            let p: PaintCircleParams = serde_wasm_bindgen::from_value(js).unwrap();
            Box::new(PaintCircleOp {
                x: p.x, y: p.y, radius: p.radius,
                color: [p.r, p.g, p.b, p.a],
            })
        },
    }
}

pub struct PaintCircleOp {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: [u8; 4],
}

impl ToolOp for PaintCircleOp {
    fn apply(&self, doc: &mut Document, layer_id: LayerId) {
        // ... pixel math: iterate tiles touched by circle, write pixels ...
    }
}
```

**`crates/darkly/src/tools/erase_circle.rs`:**

Same pattern — `register()` + `EraseCircleOp` struct + `ToolOp` impl. Identical loop to paint_circle but writes `[0, 0, 0, 0]`.

**`crates/darkly/src/tools/flood_fill.rs`:**

`register()` + `FloodFillOp` struct + `ToolOp` impl. Scanline flood-fill algorithm.

**`crates/darkly/src/tools/gradient.rs`:**

`register()` + `LinearGradientOp` struct + `ToolOp` impl. Two-point linear gradient.

#### 5b: WASM bridge — stroke lifecycle

The WASM bridge exposes a three-method stroke lifecycle that maps to undo transactions:

```rust
impl DarklyHandle {
    /// Begin a stroke on a layer. Starts an undo transaction.
    /// Must be called before any stroke_to() calls.
    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.doc.begin_transaction(layer_id);
        self.active_stroke_layer = Some(layer_id);
    }

    /// Apply a stroke operation. Can be called once (fill, gradient)
    /// or many times (brush, eraser — once per pointer event).
    /// Delegates to the ToolRegistry — no match on op_type here.
    pub fn stroke_to(&mut self, op_type: &str, params: &JsValue) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };

        self.tool_registry.apply(op_type, params.clone(), &mut self.doc, layer_id);
    }

    /// End the current stroke. Commits the undo transaction.
    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            if let Some(step) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(step);
            }
        }
    }
}
```

The key insight from Krita: **the tool never manages undo**. `begin_stroke` opens a transaction, `end_stroke` commits it. Everything in between is one undo step. The TS tool doesn't know or care about transactions — it just calls `begin_stroke` on pointer down and `end_stroke` on pointer up.

#### 5c: TypeScript side — gesture interpreters

**`frontend/src/tools/registry.ts` — Tool interface:**

```typescript
import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import type { Component } from 'svelte';

export interface ToolContext {
    handle: DarklyHandle;
    screenToCanvas: (screenX: number, screenY: number) => { x: number; y: number };
}

export interface Tool {
    readonly id: string;
    readonly name: string;
    readonly icon: string;

    /** Key name in HotkeyMap that activates this tool (e.g. 'brushTool').
     *  Used by hotkey registration to wire up tool switching automatically. */
    readonly hotkeyAction: string;

    /** Optional Svelte component for tool-specific options panel */
    readonly optionsComponent?: Component;

    onActivate?(ctx: ToolContext): void;
    onDeactivate?(ctx: ToolContext): void;
    onPointerDown(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerMove(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerUp(ctx: ToolContext, e: PointerEvent): void;
}

class ToolRegistry {
    private tools = new Map<string, Tool>();
    private order: string[] = [];

    register(tool: Tool) {
        this.tools.set(tool.id, tool);
        this.order.push(tool.id);
    }

    get(id: string): Tool | undefined {
        return this.tools.get(id);
    }

    all(): Tool[] {
        return this.order.map(id => this.tools.get(id)!);
    }
}

export const toolRegistry = new ToolRegistry();
```

The TS `Tool` interface is deliberately simple: it's a gesture interpreter, not a pixel modifier. Its job is to translate "the user dragged from A to B while holding Shift" into the correct sequence of WASM bridge calls.

**`frontend/src/tools/brush.svelte.ts` — Brush tool:**

Each tool file is fully self-contained: constants, default hotkey, runtime state, and the gesture interpreter. Adding a new tool means creating one file like this and importing it in the registry + schema.

```typescript
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

// ─── Constants & hotkey (hotkey imported by schema.ts) ───

export const MIN_SIZE = 1;
export const MAX_SIZE = 500;
export const SIZE_STEP = 4;
export const INITIAL_SIZE = 24;
export const INITIAL_OPACITY = 1.0;

export const BRUSH_HOTKEY = 'KeyB';

// ─── Gesture interpreter ───

export const brushTool: Tool = {
    id: 'brush',
    name: 'Brush',
    icon: 'B',
    hotkeyAction: 'brushTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        // wasm-bindgen maps Rust u64 to JS BigInt — convert at the WASM boundary
        ctx.handle.begin_stroke(BigInt(layerId));

        const c = app.foreground;
        const alpha = Math.round(c.a * app.brushOpacity);
        ctx.handle.stroke_to('paint_circle', {
            x: cx, y: cy, radius: app.brushSize,
            r: c.r, g: c.g, b: c.b, a: alpha,
        });
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        const c = app.foreground;
        const alpha = Math.round(c.a * app.brushOpacity);
        ctx.handle.stroke_to('paint_circle', {
            x: cx, y: cy, radius: app.brushSize,
            r: c.r, g: c.g, b: c.b, a: alpha,
        });
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },
};
```

Note: the brush tool reads `app.foreground`, `app.brushSize`, `app.brushOpacity` at the moment of each pointer event. This is the correct behavior for Phase 2 — the user can change brush size mid-stroke and see the effect. If we later need Krita-style "freeze parameters at stroke start," that's a one-line change: capture the values in `onPointerDown` and close over them.

**`frontend/src/tools/eraser.svelte.ts`:**

Same single-file pattern. Exports `ERASER_HOTKEY`. Shares `app.brushSize` / `app.brushOpacity` with the brush tool (matching Krita behavior). Gesture interpreter: `onPointerDown` calls `begin_stroke`, each move calls `stroke_to('erase_circle', { x, y, radius })`, `onPointerUp` calls `end_stroke`.

**`frontend/src/tools/fill.svelte.ts`:**

Exports `FILL_HOTKEY`. Runtime state: `tolerance` (0–255, default 32) and `fillAll` (default false) as `$state` properties on the tool object, adjustable from the tool options panel. One-shot tool. `onPointerDown` does everything: `begin_stroke` → `stroke_to('flood_fill', { x, y, r, g, b, a, tolerance })` → `end_stroke`.

**`frontend/src/tools/gradient.svelte.ts`:**

Exports `GRADIENT_HOTKEY`. Runtime state: `gradientType` ('linear' | 'radial', default 'linear'). Two-point tool. `onPointerDown` calls `begin_stroke` and records start point. `onPointerUp` calls `stroke_to('linear_gradient', { x0, y0, x1, y1, ... })` → `end_stroke`.

**`frontend/src/tools/colorpicker.svelte.ts`:**

Exports `COLORPICKER_HOTKEY = 'KeyP'`. No config interface needed (no settings). Read-only tool — no stroke at all. `onPointerDown` calls `ctx.handle.pick_color(cx, cy)` which reads from the composite cache via GPU readback. Sets `app.foreground` to the sampled color.

```typescript
export const COLORPICKER_HOTKEY = 'KeyP';
```

**Tool registration (`tools/index.ts`):**

Each tool is imported and registered. The `hotkeyAction` on each tool allows automatic hotkey wiring in Step 9 — no need to hardcode tool IDs in the hotkey registration.

```typescript
import { toolRegistry } from './registry';
import { brushTool } from './brush.svelte';
import { eraserTool } from './eraser.svelte';
import { fillTool } from './fill.svelte';
import { gradientTool } from './gradient.svelte';
import { colorPickerTool } from './colorpicker.svelte';

toolRegistry.register(brushTool);
toolRegistry.register(eraserTool);
toolRegistry.register(fillTool);
toolRegistry.register(gradientTool);
toolRegistry.register(colorPickerTool);
```

**Verification:** Switching tools via hotkeys (B, E, F, G, P) changes active tool. Each tool responds to pointer events. Brush paints with foreground color. Eraser clears pixels. Fill flood-fills. Gradient draws between two points. Color picker samples and updates foreground. Undo reverses one complete stroke (not individual dabs).

---

### Step 6: Layer groups (`darkly`)

**`layer.rs` additions:**

```rust
pub struct LayerGroup {
    pub id: LayerId,
    pub name: String,
    pub children: Vec<LayerNode>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub passthrough: bool,  // true = passthrough (default), false = normal group
    pub collapsed: bool,    // UI state: whether the group is visually collapsed
}

/// A node in the layer tree. Either a leaf layer or a group containing children.
pub enum LayerNode {
    Layer(Layer),
    Group(LayerGroup),
}

impl LayerNode {
    pub fn id(&self) -> LayerId {
        match self {
            LayerNode::Layer(l) => l.id(),
            LayerNode::Group(g) => g.id,
        }
    }

    pub fn visible(&self) -> bool {
        match self {
            LayerNode::Layer(l) => l.visible(),
            LayerNode::Group(g) => g.visible,
        }
    }
}
```

**`document.rs` changes:**

The `Document::layers` field changes from `Vec<Layer>` to `Vec<LayerNode>` — a forest of layer trees.

```rust
pub struct Document {
    pub layers: Vec<LayerNode>,     // root-level nodes (bottom to top)
    pub width: u32,
    pub height: u32,
    pub dirty: HashMap<LayerId, DirtyRegion>,
    next_id: LayerId,
}

/// Where to move a layer in the tree.
pub enum MoveTarget {
    Before(LayerId),              // before the given sibling
    After(LayerId),               // after the given sibling
    IntoGroupTop(LayerId),        // as first child of group
    IntoGroupBottom(LayerId),     // as last child of group
}

impl Document {
    /// Add a new empty group at the top of the root level.
    pub fn add_group(&mut self) -> LayerId;

    /// Add a raster layer inside a group (or root if parent is None).
    pub fn add_raster_layer_in(&mut self, parent: Option<LayerId>) -> LayerId;

    /// Flatten the layer tree into display order (bottom-to-top) for compositing.
    /// In passthrough mode, groups are transparent — children are yielded directly
    /// in their natural order. Group visibility is respected: if a group is hidden,
    /// none of its children are yielded.
    pub fn flat_layers(&self) -> Vec<&Layer>;

    /// Recursively flatten a node list.
    fn flatten_nodes<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a Layer>);

    /// Move a layer/group to a new position in the tree.
    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget);

    /// Remove a node from the tree (detaches and returns it).
    fn detach_node(&mut self, layer_id: LayerId) -> Option<LayerNode>;

    /// Insert a node at a target position.
    fn insert_node(&mut self, node: LayerNode, target: MoveTarget);

    /// Find a node anywhere in the tree.
    pub fn find_node(&self, id: LayerId) -> Option<&LayerNode>;
    pub fn find_node_mut(&mut self, id: LayerId) -> Option<&mut LayerNode>;

    /// Get the parent group of a node (None if root level).
    pub fn parent_of(&self, id: LayerId) -> Option<LayerId>;

    /// Remove a layer or group (and all its children).
    pub fn remove_layer(&mut self, id: LayerId);
}
```

**Compositor changes:**

The compositor currently iterates `doc.layers` directly. Change it to call `doc.flat_layers()` which returns a `Vec<&Layer>` in display order, ignoring group structure in passthrough mode. All existing compositing logic (ping-pong, scissor, cache) works unchanged on this flat list.

Cache invalidation: any structural change (layer move, group add/remove, visibility toggle, reorder) → full cache invalidation.

**Verification:** Unit tests:
- Create groups, add layers inside, verify `flat_layers()` returns correct bottom-to-top order
- Move layers between groups, verify ordering
- Hide a group, verify its children are excluded from `flat_layers()`
- Compositor renders identically whether layers are in groups or at root level (passthrough mode)

---

### Step 7: UI — Left sidebar

**`frontend/src/ui/LeftSidebar.svelte`:**

A narrow (48px default, from `user.resolved.ui.leftSidebarWidth`) vertical bar on the left edge. Contents from top to bottom:

1. **Color swatches** — Two overlapping squares (20x20px) showing foreground (top-left) and background (bottom-right). Click foreground square to open the color picker popup. Click the small swap icon or press X to swap. Displays `app.foreground` and `app.background`.

2. **Tool buttons** — Vertical stack of icon buttons, one per registered tool. Active tool is highlighted with a border or background color. Click to switch tool. Title attribute shows tool name + hotkey.

3. **Tool options** — If the active tool has an `optionsComponent`, render it below the tool buttons.

**`frontend/src/ui/ColorPicker.svelte`:**

A popup that appears above the left sidebar when the foreground swatch is clicked. Contains:
- **SV plane** — A `<canvas>` element showing the Saturation/Value square at the current Hue. Click/drag to select SV.
- **Hue slider** — Vertical strip with rainbow gradient. Click/drag to select Hue.
- **Opacity slider** — Vertical strip for alpha.
- **Hex input** — Text field for direct hex color entry.
- Updates `app.foreground` in real-time as the user interacts.

The SV plane is rendered on a `<canvas>` for performance (avoids DOM overhead for 256x256 color cells). Re-renders only when Hue changes.

**Verification:** Left sidebar renders at correct width. Tool buttons switch active tool. Color picker opens, allows color selection, updates foreground. D resets to B/W. X swaps.

---

### Step 8: UI — Right sidebar (layer panel)

**`frontend/src/ui/RightSidebar.svelte`:**

A ~260px panel on the right edge containing the layer panel.

**`frontend/src/ui/layers/LayerPanel.svelte`:**

Displays the layer tree as a vertically scrollable list. Display order: topmost layer first (matching Photoshop/Krita convention — reversed from the internal bottom-to-top order). Group children are indented.

Each `LayerItem` shows:
- **Visibility toggle** — Eye icon, toggles `visible` via `handle.set_layer_visible()`
- **Layer name** — Text, editable on double-click via `handle.set_layer_name()`
- **Opacity slider** — Inline `<input type="range">`, updates via `handle.set_opacity()`
- **Blend mode** — `<select>` dropdown (Normal, Multiply, Screen, Overlay)

Each `LayerGroup` shows:
- **Collapse toggle** — Triangle icon, expands/collapses children
- **Group name** — Editable
- **Visibility toggle**
- Children rendered as indented `LayerItem`s / nested `LayerGroup`s

**Action buttons** at the bottom of the panel:
- **New Layer** (+) — `handle.add_raster_layer()` or `handle.add_raster_layer_in(activeGroupId)`
- **New Group** (folder icon) — `handle.add_group()`
- **Delete** (trash icon) — `handle.remove_layer(activeLayerId)`

**Drag-and-drop reorder:**

HTML5 Drag and Drop API:
- `draggable="true"` on each layer/group row
- `dragstart`: store dragged layer ID in `dataTransfer.setData()`
- `dragover`: compute drop position from mouse Y within target element:
  - Top 25% of target → drop before (above in display = after in stack order)
  - Bottom 25% of target → drop after (below in display = before in stack order)
  - Middle 50% of a group row → drop into group as first child
- `drop`: call `handle.move_layer(draggedId, targetType, targetId)` which maps to `doc.move_layer()`
- Visual indicator: a horizontal line between items (for before/after) or a highlight (for into-group)

**Layer tree synchronization:**

After each mutation (add, remove, move, rename, visibility, opacity, blend mode), the frontend calls `handle.layer_tree()` to get the updated tree structure as JSON. This is stored in a reactive `$state` and drives the `LayerPanel` rendering.

```typescript
// In CanvasView.svelte or a layer store
let layerTree = $state<LayerTreeNode[]>([]);

function refreshLayerTree() {
    if (app.handle) {
        layerTree = app.handle.layer_tree();
    }
}
```

**WASM bridge — `layer_tree()` return format:**

```json
[
    { "type": "raster", "id": 3, "name": "Layer 2", "visible": true,
      "opacity": 1.0, "blendMode": 0 },
    {
        "type": "group", "id": 5, "name": "Group 1",
        "visible": true, "collapsed": false, "passthrough": true,
        "children": [
            { "type": "raster", "id": 2, "name": "Layer 1", "visible": true,
              "opacity": 1.0, "blendMode": 0 }
        ]
    },
    { "type": "filter", "id": 4, "name": "Noise", "visible": true },
    { "type": "raster", "id": 1, "name": "Background", "visible": true,
      "opacity": 1.0, "blendMode": 0 }
]
```

Note: returned in top-to-bottom display order (reversed from internal order) for direct UI rendering.

**Verification:** Layer panel shows all layers and groups. Drag-and-drop reorders. Layers can be dragged into/out of groups. Visibility toggle hides layers (compositor skips them). Opacity slider adjusts smoothly. New layer/group/delete buttons work. Double-click rename works.

---

### Step 9: Hotkey registration + integration

Wire everything together in `editor.ts`. Tool hotkeys are built automatically from the registry — adding a new tool with a `hotkeyAction` field automatically gets a hotkey binding without touching this file.

```typescript
import { registerHotkeys } from './config/hotkeys.svelte';
import { app } from './state/app.svelte';
import { toolRegistry } from './tools/registry';
import { MIN_SIZE, MAX_SIZE, SIZE_STEP } from './tools/brush.svelte';

// Build tool-switching hotkey actions from the registry.
// Each tool's hotkeyAction (e.g. 'brushTool') maps to switching to that tool.
const toolActions: Record<string, () => void> = {};
for (const tool of toolRegistry.all()) {
    toolActions[tool.hotkeyAction] = () => { app.activeToolId = tool.id; };
}

// Register all hotkey actions.
// Canvas navigation (Space+drag, etc.) is handled by the navigation state machine,
// not tinykeys, because those are modifier+drag combos.
registerHotkeys({
    undo:            () => app.handle?.undo(),
    redo:            () => app.handle?.redo(),
    resetColors:     () => app.resetColors(),
    swapColors:      () => app.swapColors(),
    ...toolActions,
    brushSizeUp:     () => {
        app.brushSize = Math.min(app.brushSize + SIZE_STEP, MAX_SIZE);
    },
    brushSizeDown:   () => {
        app.brushSize = Math.max(app.brushSize - SIZE_STEP, MIN_SIZE);
    },
    opacityUp:       () => {
        app.brushOpacity = Math.min(1.0, app.brushOpacity + 0.1);
    },
    opacityDown:     () => {
        app.brushOpacity = Math.max(0.0, app.brushOpacity - 0.1);
    },
});
```

**Verification — all Phase 2 mandatory hotkeys:**
- Space+drag → pan canvas
- Shift+Space+drag → rotate canvas
- Ctrl+Space+drag → zoom canvas
- D → reset foreground/background to black/white
- X → swap foreground/background
- Ctrl+Z → undo
- Ctrl+Shift+Z → redo

---

### Step 10: Layout + App.svelte refactor

Refactor `App.svelte` from a monolithic canvas component into a three-column layout shell.

**`frontend/src/App.svelte`:**

```svelte
<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import CanvasView from './canvas/CanvasView.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
</script>

<div class="app-layout">
    <LeftSidebar />
    <CanvasView />
    <RightSidebar />
</div>

<style>
    :global(body) {
        margin: 0;
        padding: 0;
        background: #1a1a1a;
        overflow: hidden;
        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
        color: #e0e0e0;
        user-select: none;
    }

    .app-layout {
        display: flex;
        width: 100vw;
        height: 100vh;
        overflow: hidden;
    }
</style>
```

`CanvasView.svelte` inherits all canvas logic from the current `App.svelte`:
- Canvas mounting + WASM init
- rAF render loop
- Pointer event dispatch: navigation first, then active tool
- `$effect` to sync view transform when `app.panX/panY/zoom/rotation` change
- Coordinate transform for screen → canvas via `handle.screen_to_canvas()`
- Canvas fills the remaining space between sidebars (`flex: 1`)

**Verification:** Three-column layout renders. Canvas fills center. Sidebars render on left and right. All Phase 1 functionality preserved (painting, undo/redo, compositing). All Phase 2 features integrated.

---

## Build Order Summary

| Step | Deliverable | Test |
|------|-------------|------|
| 1 | Config system: project/user split, presets, reactive stores, hotkeys | Both stores resolve; presets swap user config; localStorage persists |
| 2 | AppState singleton | Colors, active tool, view state reactive |
| 3 | View transform (Rust + shader) | `set_view_transform()` pans/zooms/rotates the canvas |
| 4 | Navigation state machine | Space+drag pans, Shift+Space rotates, Ctrl+Space zooms |
| 5a | Rust: `ToolOp` trait + `tools/` self-contained modules + build.rs registry | Each tool op applies correctly; unit tests for each op |
| 5b | WASM: stroke lifecycle (begin/stroke_to/end) | Stroke → undo transaction; round-trips from TS work |
| 5c | TS: 5 tool gesture interpreters | Brush/eraser/fill/gradient/picker respond to pointer events |
| 6 | Layer groups (Rust) | `flat_layers()` correct order; passthrough compositing works |
| 7 | Left sidebar UI | Color picker + tool buttons render and function |
| 8 | Right sidebar + layer panel | Layer tree, drag-drop reorder, visibility, opacity |
| 9 | Hotkey registration | All mandatory hotkeys work with Krita preset |
| 10 | Layout refactor | Three-column layout; all Phase 1 functionality preserved |

## New Dependencies

| Package | Version | Size (min+gz) | Purpose |
|---------|---------|---------------|---------|
| `defu` | ^6 | ~1 kB | Deep defaults merging for config preset overlay |
| `tinykeys` | ^3 | ~650 B | Keyboard shortcut binding |

## WASM Bridge — New Methods

```rust
// Construction — doc dimensions are decoupled from viewport
async fn create(canvas: HtmlCanvasElement, doc_width: u32, doc_height: u32) -> DarklyHandle;

// Stroke lifecycle (replaces ad-hoc paint/snapshot/commit)
fn begin_stroke(&mut self, layer_id: u64);              // opens undo transaction
fn stroke_to(&mut self, op_type: &str, params: &JsValue); // dispatches via ToolRegistry
fn end_stroke(&mut self);                                // commits undo transaction

// Read-only canvas query (color picker — no stroke needed)
fn pick_color(&self, x: f32, y: f32) -> Vec<u8>;

// View transform
fn set_view_transform(&mut self, pan_x: f32, pan_y: f32, zoom: f32, rotation: f32, screen_w: f32, screen_h: f32);
fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32>;

// Layer tree operations
fn layer_tree(&self) -> JsValue;           // returns JSON layer tree for UI
fn add_group(&mut self) -> u64;
fn add_raster_layer_in(&mut self, group_id: u64) -> u64;
fn remove_layer(&mut self, layer_id: u64);
fn move_layer(&mut self, layer_id: u64, target_type: &str, target_id: u64);
fn set_layer_name(&mut self, layer_id: u64, name: &str);
fn set_layer_visible(&mut self, layer_id: u64, visible: bool);
fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool);
```

## Lessons Learned (Implementation Notes)

### Viewport should be dynamic, not hardcoded

The original plan hardcoded the canvas buffer and GPU surface to a fixed 1920×1080 resolution, with `object-fit: contain` CSS scaling the element to fit. This created a widescreen viewport that didn't fill the window, required letterbox offset math in coordinate transforms, and meant the GPU was rendering at a fixed resolution regardless of the actual display.

**Fix:** Size the canvas buffer to match its CSS layout at `devicePixelRatio`. Use a `ResizeObserver` to call `handle.resize()` when the element size changes. The GPU surface config in `context.rs` reads `canvas.width()`/`canvas.height()` from the element instead of hardcoded values. This eliminates all letterboxing math and makes the viewport fill its container.

**Key detail:** In `context.rs`, the canvas element is moved into `SurfaceTarget::Canvas(canvas)`, so you must read `canvas.width()`/`canvas.height()` *before* the move.

### Rotation should be angular (Krita-style), not linear

The original plan used a linear mapping: horizontal pixels dragged → rotation angle. This feels disconnected — the canvas doesn't follow your hand.

**Fix (from Krita's `kis_rotate_canvas_action.cpp`):** Use `atan2()` to measure the angle from the canvas center to the cursor at drag start and on each move. The rotation is the angular difference. This feels like physically grabbing and spinning the canvas. The center point is computed from the canvas element's bounding rect on pointer down.

### Zoom and rotate should use vertical axis

Both zoom (Ctrl+Space+drag) and rotate (Shift+Space+drag) originally used horizontal drag (dx). Vertical drag (dy) is more natural for both — it matches the "scrub up/down" convention used in most creative tools.

### Painting should use pointer capture, not pointerleave

The original plan used `onpointerleave` to end strokes when the cursor left the canvas. This means any accidental slip off the edge kills your stroke. Using `setPointerCapture(e.pointerId)` on pointer down tells the browser to keep delivering events to the canvas until the button is released, regardless of cursor position.

## Key Reference Files

- Krita tool architecture analysis: `KRITA-TOOL-ARCHITECTURE.md`
- Krita default shortcuts: `krita/krita/data/shortcuts/krita_default.shortcuts`
- Krita input profile (canvas nav): `krita/krita/data/input/kritadefault.profile`
- Krita tool actions: `krita/plugins/tools/tools.action`
- Krita main actions: `krita/krita/krita.action`
- Krita rotation implementation: `krita/libs/ui/input/kis_rotate_canvas_action.cpp`
