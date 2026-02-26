# Phase 2, Session 5 — Left Sidebar UI + Hotkeys + Layout Integration

## Scope

Steps 7, 9, and 10 from the Phase 2 plan. Build the left sidebar (color picker + tool buttons), wire up all hotkey registration, and refactor App.svelte into the final three-column layout.

## Prerequisites

Session 4 complete: layer groups and layer panel working, right sidebar rendering. All tools, navigation, and config systems operational.

## Done When

- Left sidebar renders with color swatches and tool buttons
- HSV color picker popup works (SV plane + hue slider + opacity + hex input)
- Tool buttons switch active tool with visual highlight
- All Phase 2 hotkeys wired up and working
- Three-column layout (left sidebar + canvas + right sidebar)
- All Phase 1 and Phase 2 functionality integrated and working together

---

## Step 7: Left sidebar UI

### `frontend/src/ui/LeftSidebar.svelte`

Narrow vertical bar (48px, from `user.resolved.ui.leftSidebarWidth`). Contents top to bottom:

1. **Color swatches** — Two overlapping squares (20x20px):
   - Foreground (top-left) — click to open color picker popup
   - Background (bottom-right)
   - Small swap icon or press X to swap
   - Displays `app.foreground` and `app.background`
   - D resets to black/white

2. **Tool buttons** — Vertical stack of icon buttons, one per registered tool from `toolRegistry.all()`. Active tool highlighted with border/background. Click to switch. Title shows tool name + hotkey.

3. **Tool options** — If active tool has `optionsComponent`, render it below the buttons.

### `frontend/src/ui/ColorPicker.svelte`

Popup that appears above/beside the left sidebar when foreground swatch is clicked.

Components:
- **SV plane** — `<canvas>` element showing Saturation/Value square at current Hue. Click/drag to select SV. Rendered on canvas for performance (avoids DOM overhead for 256x256 cells). Re-renders only when Hue changes.
- **Hue slider** — Vertical strip with rainbow gradient. Click/drag to select Hue.
- **Opacity slider** — Vertical strip for alpha.
- **Hex input** — Text field for direct hex color entry.
- Updates `app.foreground` in real-time as user interacts.

**Color space conversions:** Need `rgbToHsv()` and `hsvToRgb()` helper functions. Keep them in the ColorPicker file or a small `color-utils.ts` — no external dependency needed.

**Popup behavior:**
- Opens on foreground swatch click
- Closes on click outside (use a backdrop or `pointerdown` listener on window)
- Positioned adjacent to the left sidebar, not overlapping it

### Verification

- Left sidebar renders at correct width
- Tool buttons switch active tool
- Active tool visually highlighted
- Color picker opens on foreground swatch click
- SV plane + hue slider allow full color selection
- Hex input accepts valid hex colors
- Color picker updates `app.foreground` in real-time
- D resets colors, X swaps colors

---

## Step 9: Hotkey registration + integration

Wire everything together. Tool hotkeys are built automatically from the registry — adding a new tool with a `hotkeyAction` field automatically gets a hotkey binding.

### `frontend/src/editor.ts` (or dedicated init file)

```typescript
import { registerHotkeys } from './config/hotkeys.svelte';
import { app } from './state/app.svelte';
import { toolRegistry } from './tools/registry';
import { MIN_SIZE, MAX_SIZE, SIZE_STEP } from './tools/brush.svelte';

// Build tool-switching actions from registry automatically
const toolActions: Record<string, () => void> = {};
for (const tool of toolRegistry.all()) {
    toolActions[tool.hotkeyAction] = () => { app.activeToolId = tool.id; };
}

registerHotkeys({
    undo:            () => app.handle?.undo(),
    redo:            () => app.handle?.redo(),
    resetColors:     () => app.resetColors(),
    swapColors:      () => app.swapColors(),
    ...toolActions,
    brushSizeUp:     () => { app.brushSize = Math.min(app.brushSize + SIZE_STEP, MAX_SIZE); },
    brushSizeDown:   () => { app.brushSize = Math.max(app.brushSize - SIZE_STEP, MIN_SIZE); },
    opacityUp:       () => { app.brushOpacity = Math.min(1.0, app.brushOpacity + 0.1); },
    opacityDown:     () => { app.brushOpacity = Math.max(0.0, app.brushOpacity - 0.1); },
});
```

### All Phase 2 mandatory hotkeys

| Hotkey | Action |
|--------|--------|
| Space+drag | Pan canvas |
| Shift+Space+drag | Rotate canvas |
| Ctrl+Space+drag | Zoom canvas |
| Ctrl+scroll | Zoom (cursor-centered) |
| D | Reset foreground/background to black/white |
| X | Swap foreground/background |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z | Redo |
| B | Brush tool |
| E | Eraser tool |
| F | Fill tool |
| G | Gradient tool |
| P | Color picker tool |
| ] | Increase brush size |
| [ | Decrease brush size |
| O | Increase opacity |
| I | Decrease opacity |

Note: Canvas navigation (Space+drag combos) is handled by the navigation state machine from Session 2, not tinykeys — those are modifier+drag combos, not keystrokes.

### Verification

- All hotkeys in the table above work
- Switching to Photoshop preset changes tool hotkeys
- Switching back to Krita preset restores defaults

---

## Step 10: Layout + App.svelte refactor

Refactor `App.svelte` from monolithic to three-column layout shell.

### `frontend/src/App.svelte`

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

`CanvasView.svelte` (already created in Session 2) fills remaining space between sidebars (`flex: 1`).

### Integration checklist

- [ ] Three-column layout renders correctly
- [ ] Canvas fills center between sidebars
- [ ] Left sidebar: color swatches + tool buttons + tool options
- [ ] Right sidebar: layer panel with groups, drag-drop, controls
- [ ] Canvas navigation: pan/zoom/rotate
- [ ] All 5 tools respond to pointer events
- [ ] Undo/redo works across all tools
- [ ] All hotkeys functional with Krita preset
- [ ] Preset switching changes hotkey bindings
- [ ] Config persists via localStorage
- [ ] Phase 1 functionality preserved (compositing, filters, dirty tracking)

---

## Files Created/Modified This Session

```
frontend/src/
├── ui/
│   ├── LeftSidebar.svelte      # NEW
│   └── ColorPicker.svelte      # NEW
├── editor.ts                   # MODIFIED: hotkey registration
└── App.svelte                  # MODIFIED: three-column layout
```
