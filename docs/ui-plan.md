# Darkly UI Plan

## Philosophy

The UI exists to stay out of the way. Every pixel not occupied by the canvas is a cost. Controls should be immediately legible, never decorative. The visual language is: flat, monochrome, dense, quiet.

No gradients. No shadows on controls. No rounded-everything. No color except where color carries meaning (accent for active state, danger for destructive actions, and the user's own canvas content).

## Look and Feel

The default dark theme is built on pure black (`#000`). The default light theme is built on pure white (`#fff`). Raised surfaces, hover states, and borders are subtle shifts from the base — never more than a few steps away. Text is muted by default; only active/focused elements get full contrast.

The palette for each theme is intentionally small:

| Token         | Purpose                              |
|---------------|--------------------------------------|
| `--bg`        | App background, toolbar, sidebar     |
| `--bg-raised` | Slightly elevated surfaces           |
| `--bg-hover`  | Hover state, subtle borders          |
| `--bg-active` | Active/pressed state                 |
| `--text`      | Primary text                         |
| `--text-muted`| Labels, secondary text               |
| `--text-dim`  | Disabled, decorative text            |
| `--accent`    | Active tool, selection, slider thumb |
| `--danger`    | Delete, destructive actions          |
| `--thumb-bg`  | Layer/preview thumbnails             |
| `--canvas-bg` | Area behind the canvas               |

That's the entire color system. A theme is a single file that sets these variables. Nothing else.

## Theming

Themes are CSS files that define custom properties on a class (`.dark`, `.light`, or any user-defined name). The active theme is a class on `<body>`. Switching themes = swapping that class.

User-created themes are a future goal. The architecture supports it by default: a theme is just a CSS file with ~11 variables. No registration, no code changes.

## Styling Rules

**Scoped styles only.** Each Svelte component owns its styles in a `<style>` block. No global stylesheets beyond the reset, shared tokens, and theme definitions.

**Variables for all visual tokens.** Components never use raw color values. Every color, and any spacing/radius/timing value that appears in more than one component, is a CSS custom property.

**No CSS libraries.** No Tailwind, no CSS-in-JS, no utility frameworks. The app's UI is bespoke — toolbar buttons, layer panels, node editors, scrub controls — none of which map to generic component libraries. CSS custom properties plus Svelte scoping is the entire system.

**DRY through tokens, not abstractions.** When two components share a visual pattern (e.g., icon buttons, slider thumbs), the shared parts are expressed as shared CSS variables or a small shared class. Don't build wrapper components for styling alone — a `<button>` with the right variables is fine.

**Extract only when repetition is real.** If a multi-property pattern (like the icon-button reset) appears in three or more components, extract it to a shared class in `tokens.css`. Not before.

## File Structure

```
frontend/src/
  themes/
    dark.css          # .dark { --bg: #000; ... }
    light.css         # .light { --bg: #fff; ... }
  styles/
    reset.css         # *, body — box-sizing, margin, overflow, font
    tokens.css        # shared non-color tokens (radii, spacing, transitions)
                      # and any extracted multi-component patterns
  ui/
    *.svelte          # components with scoped <style> blocks
```

## Typography

One font stack: `-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif`. No web fonts, no icon font (icons TBD — likely inline SVG or a lean icon set).

Text sizes are small and functional:
- Labels, panel titles: 9-11px, uppercase, tracked
- Values, layer names: 11-12px
- Nothing larger inside the chrome; the canvas speaks for itself

## Interaction

Controls are dense but not cramped. Hit targets are at least 24x24px even when visually smaller. Hover states are instant (0.1s transitions). Active states are visually distinct — accent-colored background, white text.

Scrub controls (drag-to-adjust values) are preferred over traditional sliders for tool options. They're compact and don't require precise click targeting.

Panels collapse and expand. The sidebar is resizable. The bottom bar (node editor) slides up and supports fullscreen. All layout dimensions are flexible, never hardcoded to specific viewport sizes.

## Principles

1. **Canvas first.** The UI is a thin frame around the canvas. Minimize chrome.
2. **No decoration.** If a visual element doesn't convey state or afford interaction, remove it.
3. **Monochrome until meaningful.** Color in the UI means something happened — selection, activation, danger. Never ornamental.
4. **Fast to scan.** A user glancing at the sidebar should instantly read the layer structure, active tool, and current values. Dense, not cluttered.
5. **Themeable by default.** Because the entire visual system is ~11 variables, theming is free. No special infrastructure needed.
