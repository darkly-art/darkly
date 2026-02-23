# Darkly Phase 2 — UI, Canvas Navigation, Configuration & Layer Groups

## Context

Phase 1 established the core engine: tiled raster layers, GPU compositing with dirty-rect optimization, a modular filter system, and COW undo/redo. Phase 2 adds the user-facing shell: a minimal but properly engineered UI, canvas pan/zoom/rotate, a modular tool system, a preset-based configuration system, and layer groups with passthrough compositing.

**Engineering principle (evolved from Phase 1):** Every system that is implemented must be implemented properly. No hacks, no shortcuts, no hardcoding — in Rust or TypeScript. Phase 1 allowed the frontend to cut corners because it was throwaway scaffolding. Phase 2's frontend *is* the product: the UI, tools, config system, and navigation are all first-class systems that must be built correctly on the first iteration. The same standard that Phase 1 applied to the engine now applies to everything.

**Phase 2 scope:**
- Left sidebar: color picker + tool buttons
- Right sidebar: layer panel with groups, reordering via drag-and-drop
- Canvas navigation: pan (Space+drag), rotate (Shift+Space+drag), zoom (Ctrl+Space+drag)
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

**Resolution order:** `user overrides > active preset > defaults`. The `defu` utility handles recursive merging with leftmost-wins semantics: `defu(userOverrides, presetOverrides, DEFAULTS)`.

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

**Rust side (`darkly-core`):** A `StrokeOp` enum that represents the different kinds of tool operations. Each variant captures the parameters needed to execute the operation on the document. `Document` gains methods that accept these operations and handle tile modification + dirty tracking. The WASM bridge exposes a stroke lifecycle: `begin_stroke(layer_id)` → `stroke_to(op_data)` → `end_stroke()`, where begin/end map to undo transactions.

This is NOT a trait-object registry like the filter system, because the two have fundamentally different lifecycles. Filters are **state**: a `FilterLayer` holds `Box<dyn FilterParams>` as part of the document — it persists, gets cloned for undo (`clone_boxed`), gets downcast for UI access (`as_any`), and must be stored polymorphically alongside raster layers. Trait objects are the right fit for stored polymorphic state. Stroke operations are **commands**: constructed from JS params, dispatched to `Document::apply_stroke_op()`, and immediately consumed. They're never stored, never cloned, never downcast. An enum is the right fit for transient dispatched commands — it gives exhaustive matching, zero allocation, and trivial WASM-boundary serialization. New tools add a variant to `StrokeOp` and a match arm in `Document::apply_stroke_op()`.

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

Layer groups are added to `darkly-core` as a new variant `Layer::Group(LayerGroup)`. Passthrough mode (default, matching Photoshop behavior) means groups are treated as organizational containers only — the compositor flattens the tree and composites layers in display order, ignoring group boundaries.

Non-passthrough mode (normal blending group) would isolate the group's composite result and blend it into the parent as a single unit. Phase 2 implements passthrough only; the data model supports both.

---

## Project Structure (Phase 2 additions)

```
darkly/
├── crates/
│   ├── darkly-core/
│   │   └── src/
│   │       ├── layer.rs              # + LayerGroup, LayerNode tree structure
│   │       ├── document.rs           # + tree traversal, group operations, reorder
│   │       ├── stroke.rs             # StrokeOp enum + Document::apply_stroke_op()
│   │       └── color.rs              # Color types (used by tools via WASM bridge)
│   │
│   └── darkly-gpu/
│       └── src/
│           ├── compositor.rs         # + view transform uniform, flatten groups
│           └── view.rs               # ViewTransform: pan, zoom, rotate matrix
│
├── frontend/
│   ├── src/
│   │   ├── App.svelte                # Layout shell: left sidebar + canvas + right sidebar
│   │   ├── canvas/
│   │   │   ├── CanvasView.svelte     # Canvas element + pointer handlers + navigation
│   │   │   └── navigation.svelte.ts  # Pan/zoom/rotate state machine
│   │   ├── config/
│   │   │   ├── schema.ts             # DarklyConfig interface + DEFAULTS
│   │   │   ├── presets/
│   │   │   │   ├── krita.ts          # Krita hotkey preset (default)
│   │   │   │   ├── photoshop.ts      # Photoshop hotkey preset
│   │   │   │   └── gimp.ts           # GIMP hotkey preset
│   │   │   ├── store.svelte.ts       # ConfigStore: reactive config with persistence
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

**`frontend/src/config/schema.ts` — Config interface + defaults:**

```typescript
export interface HotkeyMap {
    // Canvas navigation (modifier+drag combos handled by navigation state machine,
    // not tinykeys — but listed here for preset customization)
    panModifier: string;
    rotateModifier: string;
    zoomModifier: string;

    // Color
    resetColors: string;            // "KeyD"
    swapColors: string;             // "KeyX"

    // Edit
    undo: string;                   // "$mod+KeyZ"
    redo: string;                   // "$mod+Shift+KeyZ"

    // Tools
    brushTool: string;              // "KeyB"
    eraserTool: string;             // "KeyE"
    fillTool: string;               // "KeyF"
    gradientTool: string;           // "KeyG"
    colorPickerTool: string;        // "KeyP"

    // Brush size
    brushSizeUp: string;            // "BracketRight"
    brushSizeDown: string;          // "BracketLeft"
    opacityUp: string;              // "KeyO"
    opacityDown: string;            // "KeyI"
}

export interface DarklyConfig {
    canvas: {
        defaultWidth: number;
        defaultHeight: number;
        backgroundColor: string;
    };
    tools: {
        brush: {
            defaultSize: number;
            minSize: number;
            maxSize: number;
            defaultOpacity: number;
            sizeStep: number;       // increment for [ and ] keys
        };
        eraser: {
            defaultSize: number;
            defaultOpacity: number;
        };
        fill: {
            tolerance: number;      // 0–255, flood fill threshold
            fillAll: boolean;       // fill all similar, not just contiguous
        };
        gradient: {
            type: 'linear' | 'radial';
        };
    };
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
    overrides: DeepPartial<DarklyConfig>;
}

export const DEFAULTS: DarklyConfig = {
    canvas: {
        defaultWidth: 1920,
        defaultHeight: 1080,
        backgroundColor: '#1a1a1a',
    },
    tools: {
        brush: {
            defaultSize: 24,
            minSize: 1,
            maxSize: 500,
            defaultOpacity: 1.0,
            sizeStep: 4,
        },
        eraser: { defaultSize: 24, defaultOpacity: 1.0 },
        fill: { tolerance: 32, fillAll: false },
        gradient: { type: 'linear' },
    },
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

**`frontend/src/config/presets/krita.ts`:**

```typescript
import type { Preset } from '../schema';

export const PRESET_KRITA: Preset = {
    name: 'Krita',
    description: 'Default Krita-style keybindings',
    overrides: {
        // Krita defaults match our DEFAULTS, so overrides are minimal.
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
import type { Preset } from '../schema';

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
import type { Preset } from '../schema';

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

**`frontend/src/config/store.svelte.ts` — Reactive config store:**

```typescript
import { defu } from 'defu';
import { DEFAULTS, type DarklyConfig, type DeepPartial, type Preset } from './schema';
import { PRESET_KRITA } from './presets/krita';
import { PRESET_PHOTOSHOP } from './presets/photoshop';
import { PRESET_GIMP } from './presets/gimp';

const STORAGE_KEY = 'darkly-config';
const PRESET_KEY = 'darkly-preset';

const PRESETS: Record<string, Preset> = {
    'Krita': PRESET_KRITA,
    'Photoshop': PRESET_PHOTOSHOP,
    'GIMP': PRESET_GIMP,
};

function loadFromStorage(): DeepPartial<DarklyConfig> {
    try {
        const raw = localStorage.getItem(STORAGE_KEY);
        return raw ? JSON.parse(raw) : {};
    } catch { return {}; }
}

function loadPreset(): Preset {
    const name = localStorage.getItem(PRESET_KEY);
    return (name && PRESETS[name]) || PRESET_KRITA;
}

class ConfigStore {
    userOverrides = $state<DeepPartial<DarklyConfig>>(loadFromStorage());
    activePreset = $state<Preset>(loadPreset());

    /** Resolved config: user > preset > defaults */
    get resolved(): DarklyConfig {
        return defu(
            this.userOverrides,
            this.activePreset.overrides,
            DEFAULTS
        ) as DarklyConfig;
    }

    get availablePresets(): Preset[] {
        return Object.values(PRESETS);
    }

    applyPreset(preset: Preset) {
        this.activePreset = preset;
        localStorage.setItem(PRESET_KEY, preset.name);
    }

    setUserOverride(path: string, value: any) {
        const parts = path.split('.');
        let obj: any = this.userOverrides;
        for (let i = 0; i < parts.length - 1; i++) {
            if (!obj[parts[i]]) obj[parts[i]] = {};
            obj = obj[parts[i]];
        }
        obj[parts[parts.length - 1]] = value;
        localStorage.setItem(STORAGE_KEY, JSON.stringify(this.userOverrides));
    }

    reset() {
        this.userOverrides = {};
        this.activePreset = PRESET_KRITA;
        localStorage.removeItem(STORAGE_KEY);
        localStorage.removeItem(PRESET_KEY);
    }
}

export const config = new ConfigStore();
```

**`frontend/src/config/hotkeys.svelte.ts` — Hotkey registration:**

```typescript
import tinykeys from 'tinykeys';
import { config } from './store.svelte';

let cleanup: (() => void) | null = null;

/**
 * Register all hotkeys from the resolved config.
 * Call on init and whenever the preset changes.
 * `actions` maps HotkeyMap key names to handler functions.
 */
export function registerHotkeys(actions: Record<string, () => void>) {
    cleanup?.();

    const hotkeys = config.resolved.hotkeys;
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

**Verification:** Config store resolves correctly. Switching presets changes hotkey values. Values persist across page reloads via localStorage.

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
    activeLayerId = $state<bigint | null>(null);

    // Brush state (shared across tools that paint)
    brushSize = $state(24);
    brushOpacity = $state(1.0);

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

**`crates/darkly-gpu/src/view.rs`:**

```rust
/// 2D view transform for canvas navigation.
/// Compositing happens in canvas-pixel space. This transform is applied
/// only in the present shader to map canvas pixels to screen pixels.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewTransform {
    /// Inverse view matrix (screen → canvas), stored as 3 vec4s for std140.
    /// Row 0: [m00, m01, 0, 0]
    /// Row 1: [m10, m11, 0, 0]
    /// Row 2: [tx,  ty,  1, 0]
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
                [m00, m01, 0.0, 0.0],
                [m10, m11, 0.0, 0.0],
                [tx,  ty,  1.0, 0.0],
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

    let dims = vec2f(textureDimensions(t_source));
    let uv = vec2f(canvas_x, canvas_y) / dims;

    // Out-of-bounds → workspace background color
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4f(0.11, 0.11, 0.11, 1.0);
    }

    let color = textureSample(t_source, t_sampler, uv);
    return vec4f(color.rgb, 1.0);
}
```

**WASM bridge additions (`api.rs`):**

```rust
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

**Verification:** Call `set_view_transform` with non-identity values. Canvas appears panned/zoomed/rotated. Painting still works correctly (mouse coordinates inverse-transformed). View-only changes (no painting) skip compositing and only re-present.

---

### Step 4: Canvas navigation state machine

Handle Space+drag (pan), Shift+Space+drag (rotate), Ctrl+Space+drag (zoom) as a proper state machine in the frontend.

**`frontend/src/canvas/navigation.svelte.ts`:**

```typescript
import { app } from '../state/app.svelte';

type NavMode = 'none' | 'pan' | 'rotate' | 'zoom';

class NavigationState {
    mode = $state<NavMode>('none');
    private startX = 0;
    private startY = 0;
    private startPanX = 0;
    private startPanY = 0;
    private startRotation = 0;
    private startZoom = 0;

    /** Track which modifier keys are currently held */
    spaceHeld = $state(false);

    get isNavigating(): boolean {
        return this.mode !== 'none';
    }

    onKeyDown(e: KeyboardEvent) {
        if (e.code === 'Space' && !e.repeat) {
            e.preventDefault();     // prevent page scroll
            this.spaceHeld = true;
        }
    }

    onKeyUp(e: KeyboardEvent) {
        if (e.code === 'Space') {
            this.spaceHeld = false;
            this.mode = 'none';
        }
    }

    onPointerDown(e: PointerEvent): boolean {
        if (!this.spaceHeld) return false;

        if (e.ctrlKey) {
            this.mode = 'zoom';
        } else if (e.shiftKey) {
            this.mode = 'rotate';
        } else {
            this.mode = 'pan';
        }

        this.startX = e.clientX;
        this.startY = e.clientY;
        this.startPanX = app.panX;
        this.startPanY = app.panY;
        this.startRotation = app.rotation;
        this.startZoom = app.zoom;
        return true; // consumed the event — don't dispatch to tool
    }

    onPointerMove(e: PointerEvent) {
        if (this.mode === 'none') return;

        const dx = e.clientX - this.startX;
        const dy = e.clientY - this.startY;

        switch (this.mode) {
            case 'pan':
                app.panX = this.startPanX + dx;
                app.panY = this.startPanY + dy;
                break;
            case 'rotate':
                // Horizontal drag = rotation. 400px drag = full 360°.
                app.rotation = this.startRotation + (dx / 400) * Math.PI * 2;
                break;
            case 'zoom':
                // Drag right = zoom in, drag left = zoom out. Exponential scaling.
                app.zoom = this.startZoom * Math.pow(2, dx / 300);
                break;
        }
    }

    onPointerUp() {
        this.mode = 'none';
    }

    onWheel(e: WheelEvent) {
        // Pinch-to-zoom / Ctrl+scroll = zoom
        if (e.ctrlKey) {
            e.preventDefault();
            const factor = Math.pow(1.001, -e.deltaY);
            app.zoom = Math.max(0.01, Math.min(100, app.zoom * factor));
        }
    }
}

export const nav = new NavigationState();
```

**`frontend/src/canvas/CanvasView.svelte`:**

The current `App.svelte` canvas+mouse logic moves here. Pointer event flow:

1. Navigation state machine gets first chance (`nav.onPointerDown(e)`)
2. If navigation consumed the event, skip tool dispatch
3. Otherwise, transform screen coords → canvas coords via `handle.screen_to_canvas()`
4. Dispatch to active tool's `onPointerDown(ctx, e, canvasX, canvasY)`

View transform is updated on every `$effect` that watches `app.panX`, `app.panY`, `app.zoom`, `app.rotation`:

```typescript
$effect(() => {
    if (app.handle) {
        app.handle.set_view_transform(
            app.panX, app.panY, app.zoom, app.rotation,
            canvas.width, canvas.height
        );
    }
});
```

**Verification:** Space+drag pans the canvas. Shift+Space+drag rotates. Ctrl+Space+drag zooms. Ctrl+scroll zooms. Painting still works correctly after transform.

---

### Step 5: Tool system

The tool system is split: Rust owns pixel modification and undo; TypeScript owns gesture interpretation and UI.

#### 5a: Rust side — StrokeOp + Document operations

**`crates/darkly-core/src/stroke.rs` — Stroke operation enum:**

```rust
/// A discrete operation that a tool applies to the document.
/// Each variant captures the parameters needed to execute the operation.
///
/// This is an enum, not a trait — tool operations are a closed set known
/// at compile time. Adding a new tool means adding a variant here and a
/// match arm in Document::apply_stroke_op(). Same pattern as BlendMode.
pub enum StrokeOp {
    /// Paint a filled circle (brush dab). Called per pointer event.
    PaintCircle {
        x: f32,
        y: f32,
        radius: f32,
        color: [u8; 4],
    },

    /// Erase a filled circle (set pixels to transparent). Called per pointer event.
    EraseCircle {
        x: f32,
        y: f32,
        radius: f32,
    },

    /// Flood fill from a seed point. One-shot (single call per stroke).
    FloodFill {
        x: f32,
        y: f32,
        color: [u8; 4],
        tolerance: u8,
    },

    /// Linear gradient between two points. One-shot.
    LinearGradient {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color0: [u8; 4],
        color1: [u8; 4],
    },
}
```

**`document.rs` — apply method:**

```rust
impl Document {
    /// Apply a stroke operation to a raster layer.
    /// The caller is responsible for calling begin_transaction before
    /// the first op and commit_transaction after the last op.
    pub fn apply_stroke_op(&mut self, layer_id: LayerId, op: &StrokeOp) {
        match op {
            StrokeOp::PaintCircle { x, y, radius, color } => {
                self.paint_circle(layer_id, *x, *y, *radius, *color);
            }
            StrokeOp::EraseCircle { x, y, radius } => {
                self.erase_circle(layer_id, *x, *y, *radius);
            }
            StrokeOp::FloodFill { x, y, color, tolerance } => {
                self.flood_fill(layer_id, *x, *y, *color, *tolerance);
            }
            StrokeOp::LinearGradient { x0, y0, x1, y1, color0, color1 } => {
                self.fill_linear_gradient(layer_id, *x0, *y0, *x1, *y1, *color0, *color1);
            }
        }
    }
}
```

The existing `Document::paint_circle()` stays where it is — it's the implementation of `StrokeOp::PaintCircle`. New operations (`erase_circle`, `flood_fill`, `fill_linear_gradient`) are added as methods on `Document` in the same style: they take coordinates and parameters, modify tiles, and mark dirty regions. `apply_stroke_op` is the dispatcher that routes to them.

**Why an enum and not a trait?** The filter system uses trait objects (`Box<dyn FilterParams>`) because filters are open-ended — users will define custom filters, and the set grows unboundedly. Tool operations are a closed set: we know all variants at compile time, they share no common behavior worth abstracting, and they have wildly different parameter shapes. An enum gives exhaustive matching (compiler catches missing arms), zero allocation, and trivial serialization. If the set ever truly becomes open (third-party tool plugins), we can migrate to traits then.

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
    pub fn stroke_to(&mut self, op_type: &str, params: &JsValue) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };

        // Deserialize op_type + params into StrokeOp
        let op = match op_type {
            "paint_circle" => {
                // params: { x, y, radius, r, g, b, a }
                let p: PaintCircleParams = serde_wasm_bindgen::from_value(params.clone()).unwrap();
                StrokeOp::PaintCircle {
                    x: p.x, y: p.y, radius: p.radius,
                    color: [p.r, p.g, p.b, p.a],
                }
            }
            "erase_circle" => {
                let p: EraseCircleParams = serde_wasm_bindgen::from_value(params.clone()).unwrap();
                StrokeOp::EraseCircle { x: p.x, y: p.y, radius: p.radius }
            }
            "flood_fill" => {
                let p: FloodFillParams = serde_wasm_bindgen::from_value(params.clone()).unwrap();
                StrokeOp::FloodFill {
                    x: p.x, y: p.y,
                    color: [p.r, p.g, p.b, p.a],
                    tolerance: p.tolerance,
                }
            }
            "linear_gradient" => {
                let p: GradientParams = serde_wasm_bindgen::from_value(params.clone()).unwrap();
                StrokeOp::LinearGradient {
                    x0: p.x0, y0: p.y0, x1: p.x1, y1: p.y1,
                    color0: [p.r0, p.g0, p.b0, p.a0],
                    color1: [p.r1, p.g1, p.b1, p.a1],
                }
            }
            _ => return,
        };

        self.doc.apply_stroke_op(layer_id, &op);
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

```typescript
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

export const brushTool: Tool = {
    id: 'brush',
    name: 'Brush',
    icon: 'B',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        // begin_stroke opens an undo transaction — one stroke = one undo step
        ctx.handle.begin_stroke(layerId);

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
        // end_stroke commits the undo transaction
        ctx.handle.end_stroke();
    },
};
```

Note: the brush tool reads `app.foreground`, `app.brushSize`, `app.brushOpacity` at the moment of each pointer event. This is the correct behavior for Phase 2 — the user can change brush size mid-stroke and see the effect. If we later need Krita-style "freeze parameters at stroke start," that's a one-line change: capture the values in `onPointerDown` and close over them.

**`frontend/src/tools/eraser.svelte.ts`:**

Identical structure to brush. `onPointerDown` calls `begin_stroke`, each move calls `stroke_to('erase_circle', { x, y, radius })`, `onPointerUp` calls `end_stroke`.

**`frontend/src/tools/fill.svelte.ts`:**

One-shot tool. `onPointerDown` does everything: `begin_stroke` → `stroke_to('flood_fill', { x, y, r, g, b, a, tolerance })` → `end_stroke`. No `onPointerMove` or `onPointerUp` work needed (the entire fill is one undo step triggered by a single click).

**`frontend/src/tools/gradient.svelte.ts`:**

Two-point tool. `onPointerDown` calls `begin_stroke` and records start point. `onPointerUp` calls `stroke_to('linear_gradient', { x0, y0, x1, y1, ... })` → `end_stroke`. The gradient parameters are computed from the start and end points, with colors from `app.foreground` and `app.background`.

**`frontend/src/tools/colorpicker.svelte.ts`:**

Read-only tool — no stroke at all. `onPointerDown` calls `ctx.handle.pick_color(cx, cy)` which reads from the composite cache via GPU readback. Sets `app.foreground` to the sampled color.

**Tool registration (in `editor.ts` or `tools/index.ts`):**

```typescript
import { toolRegistry } from './tools/registry';
import { brushTool } from './tools/brush.svelte';
import { eraserTool } from './tools/eraser.svelte';
import { fillTool } from './tools/fill.svelte';
import { gradientTool } from './tools/gradient.svelte';
import { colorPickerTool } from './tools/colorpicker.svelte';

toolRegistry.register(brushTool);
toolRegistry.register(eraserTool);
toolRegistry.register(fillTool);
toolRegistry.register(gradientTool);
toolRegistry.register(colorPickerTool);
```

**Verification:** Switching tools via hotkeys (B, E, F, G, P) changes active tool. Each tool responds to pointer events. Brush paints with foreground color. Eraser clears pixels. Fill flood-fills. Gradient draws between two points. Color picker samples and updates foreground. Undo reverses one complete stroke (not individual dabs).

---

### Step 6: Layer groups (`darkly-core`)

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

A narrow (48px default, from `config.resolved.ui.leftSidebarWidth`) vertical bar on the left edge. Contents from top to bottom:

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

Wire everything together in `editor.ts`:

```typescript
import { registerHotkeys } from './config/hotkeys.svelte';
import { app } from './state/app.svelte';
import { config } from './config/store.svelte';

// Register all hotkey actions.
// Canvas navigation (Space+drag, etc.) is handled by the navigation state machine,
// not tinykeys, because those are modifier+drag combos.
registerHotkeys({
    undo:            () => app.handle?.undo(),
    redo:            () => app.handle?.redo(),
    resetColors:     () => app.resetColors(),
    swapColors:      () => app.swapColors(),
    brushTool:       () => { app.activeToolId = 'brush'; },
    eraserTool:      () => { app.activeToolId = 'eraser'; },
    fillTool:        () => { app.activeToolId = 'fill'; },
    gradientTool:    () => { app.activeToolId = 'gradient'; },
    colorPickerTool: () => { app.activeToolId = 'colorpicker'; },
    brushSizeUp:     () => {
        const cfg = config.resolved.tools.brush;
        app.brushSize = Math.min(app.brushSize + cfg.sizeStep, cfg.maxSize);
    },
    brushSizeDown:   () => {
        const cfg = config.resolved.tools.brush;
        app.brushSize = Math.max(app.brushSize - cfg.sizeStep, cfg.minSize);
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
| 1 | Config system: schema, presets, reactive store, hotkeys | Config resolves; presets swap; localStorage persists |
| 2 | AppState singleton | Colors, active tool, view state reactive |
| 3 | View transform (Rust + shader) | `set_view_transform()` pans/zooms/rotates the canvas |
| 4 | Navigation state machine | Space+drag pans, Shift+Space rotates, Ctrl+Space zooms |
| 5a | Rust: StrokeOp enum + Document operations | `apply_stroke_op` dispatches correctly; unit tests for each op |
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
// Stroke lifecycle (replaces ad-hoc paint/snapshot/commit)
fn begin_stroke(&mut self, layer_id: u64);              // opens undo transaction
fn stroke_to(&mut self, op_type: &str, params: &JsValue); // dispatches to Document::apply_stroke_op
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

## Key Reference Files

- Krita tool architecture analysis: `KRITA-TOOL-ARCHITECTURE.md`
- Krita default shortcuts: `krita/krita/data/shortcuts/krita_default.shortcuts`
- Krita input profile (canvas nav): `krita/krita/data/input/kritadefault.profile`
- Krita tool actions: `krita/plugins/tools/tools.action`
- Krita main actions: `krita/krita/krita.action`
