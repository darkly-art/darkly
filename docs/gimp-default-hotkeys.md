# GIMP — Default Keyboard Shortcuts Reference

A comprehensive reference of the default keyboard shortcuts shipped with **GIMP 3.0+** (the GNU Image Manipulation Program). Compiled directly from the GIMP source tree — the `GimpActionEntry` arrays in `app/actions/*.c` and the tool `_register()` calls in `app/tools/gimp*tool.c` — so every binding listed here is the one GIMP actually wires into its action map at startup.

> **Tip** — View, search, or rebind these in-app via **Edit ▸ Keyboard Shortcuts…**, or hit the **Search and Run a Command** dialog (`/`) to invoke any action by name.

## Conventions

- Modifier keys are written out: `Ctrl`, `Alt`, `Shift`, `Cmd`, `Option`, `Control`.
- `+` joins keys pressed simultaneously; `,` joins keys pressed in sequence.
- Source uses GTK's `<primary>` modifier, which resolves to **Ctrl** on Linux/Windows and **Cmd** on macOS. `<alt>` resolves to **Alt** on Linux/Windows and **Option** on macOS. Both forms are shown in the tables.
- When a single action lists more than one binding, alternates are separated by ` · ` (any of them triggers the action). Some bindings reference dedicated keyboard hardware keys (`Cut`, `Copy`, `Paste`, `Forward`, `Back`, `ZoomIn`, `ZoomOut`) that exist on multimedia keyboards.
- Tool shortcuts are case-sensitive in GIMP's parser. `<shift>B` and `B` are different bindings; a bare lowercase entry like `b` (Paths tool) means the unshifted key.
- All bindings assume the default keymap and the **English** menu language. Localised builds keep the same accelerators but may show different menu labels.

## Table of Contents

- **Tools**
  - [Selecting tools (the Toolbox)](#selecting-tools-the-toolbox)
  - [Tool size & opacity nudges](#tool-size--opacity-nudges)
  - [On-canvas modifier behaviour (mouse + modifier conventions)](#on-canvas-modifier-behaviour-mouse--modifier-conventions)
  - [Inside the Text tool](#inside-the-text-tool)
- **File**
  - [Opening, saving, exporting](#opening-saving-exporting)
- **Edit**
  - [Undo, redo, clipboard, fill](#undo-redo-clipboard-fill)
- **Select**
  - [Selection commands](#selection-commands)
  - [Quick Mask](#quick-mask)
- **Image & Layers**
  - [Image commands](#image-commands)
  - [Layer commands](#layer-commands)
  - [Paths](#paths)
- **View & Window**
  - [View, zoom & display](#view-zoom--display)
  - [Explicit zoom levels](#explicit-zoom-levels)
  - [Window navigation](#window-navigation)
- **Filters & Help**
  - [Filters](#filters)
  - [Help](#help)
- **Dialogs & Dockable Panels**
  - [Opening dialogs](#opening-dialogs)
- **Colors & Context**
  - [Foreground / background colour](#foreground--background-colour)
- **Appendix**
  - [Source-of-truth files](#source-of-truth-files)

---

## Selecting tools (the Toolbox)

Each tool's binding is declared in its `_register()` call in `app/tools/gimp<name>tool.c`. Pressing the key activates the tool; pressing the previously-active tool's key gets you back. A history-of-one is available via `Shift + X` ("Activate Last Tool").

### Selection tools

| Tool | Linux / Windows | macOS |
| --- | --- | --- |
| Rectangle Select | R | R |
| Ellipse Select | E | E |
| Free Select (Lasso) | F | F |
| Fuzzy Select (Magic Wand) | U | U |
| Select by Color | Shift + O | Shift + O |
| Scissors Select | I | I |
| Foreground Select | _none — accessible from Tools ▸ Selection Tools_ | _same_ |

### Paint tools

| Tool | Linux / Windows | macOS |
| --- | --- | --- |
| Paintbrush | P | P |
| Pencil | N | N |
| Airbrush | A | A |
| Ink | K | K |
| MyPaint Brush | Y | Y |
| Eraser | Shift + E | Shift + E |
| Smudge | S | S |
| Clone | C | C |
| Heal | H | H |
| Perspective Clone | _none_ | _none_ |
| Blur / Sharpen | Shift + U | Shift + U |
| Dodge / Burn | Shift + D | Shift + D |
| Bucket Fill | Shift + B | Shift + B |
| Gradient | G | G |

### Transform tools

| Tool | Linux / Windows | macOS |
| --- | --- | --- |
| Move | M | M |
| Align and Distribute | Q | Q |
| Crop | Shift + C | Shift + C |
| Unified Transform | Shift + T | Shift + T |
| Rotate | Shift + R | Shift + R |
| Scale | Shift + S | Shift + S |
| Shear | Shift + H | Shift + H |
| Perspective | Shift + P | Shift + P |
| 3D Transform | Shift + W | Shift + W |
| Flip | Shift + F | Shift + F |
| Cage Transform | Shift + G | Shift + G |
| Warp Transform | W | W |
| Handle Transform | Shift + L | Shift + L |
| N-Point Deformation | Shift + N | Shift + N |

### Other tools

| Tool | Linux / Windows | macOS |
| --- | --- | --- |
| Color Picker (Eyedropper) | O | O |
| Zoom | Z | Z |
| Measure | Shift + M | Shift + M |
| Text | T | T |
| Paths | B (lowercase `b`) | B (lowercase `b`) |

### Tool switching helpers

| Action | Linux / Windows | macOS |
| --- | --- | --- |
| Activate Last Tool (swap with previous) | Shift + X | Shift + X |
| Show / hide the Toolbox dock | Ctrl + B | Cmd + B |

---

## Tool size & opacity nudges

These adjust the **active tool's** brush size and opacity. They work for any tool that uses a brush (Paintbrush, Eraser, Smudge, Clone, Blur/Sharpen, Dodge/Burn, …).

| Action | Linux / Windows | macOS |
| --- | --- | --- |
| Brush size: decrease by 1 | `[` | `[` |
| Brush size: increase by 1 | `]` | `]` |
| Brush size: decrease by 10 | `{` (Shift + `[`) | `{` (Shift + `[`) |
| Brush size: increase by 10 | `}` (Shift + `]`) | `}` (Shift + `]`) |
| Brush size: reset to default | `\` | `\` |
| Tool opacity: decrease by 1 | `<` | `<` |
| Tool opacity: increase by 1 | `>` | `>` |
| Tool opacity: decrease by 10 | Ctrl + `<` | Cmd + `<` |
| Tool opacity: increase by 10 | Ctrl + `>` | Cmd + `>` |

> Airbrush rate, airbrush flow, and many other tool-option scalars all have parallel `*-decrease`, `*-increase`, and `*-skip` actions that ship **without** default bindings. Assign them yourself in **Edit ▸ Keyboard Shortcuts** if you want them.

---

## On-canvas modifier behaviour (mouse + modifier conventions)

GIMP names its modifier *roles* in `app/core/` and references them everywhere a tool checks state. Roles map to physical keys as follows (the actual mapping is platform-aware, but defaults to):

| Role | Linux / Windows | macOS | Used for |
| --- | --- | --- | --- |
| `extend_selection_mask` | Shift | Shift | Add to selection; draw a straight line from last paint stamp |
| `modify_selection_mask` | Ctrl | Cmd | Subtract from selection; toggle behaviour-specific intersect |
| `constrain_behavior_mask` | Ctrl | Cmd | Constrain angles to 15°; constrain transform to axis; constrain crop to square |
| `toggle_behavior_mask` | Ctrl | Cmd | Toggle inverse behaviour (e.g. Bucket Fill — fill *similar* vs *opposite*) |
| (extra mod) | Alt | Option | Move floating selection without anchoring; pick from screen |

Selection-tool combinations across all the marquee/lasso/wand/scissors/by-color tools:

| Action while a selection tool is active | Linux / Windows | macOS |
| --- | --- | --- |
| Add to existing selection | Shift + drag | Shift + drag |
| Subtract from existing selection | Ctrl + drag | Cmd + drag |
| Intersect with existing selection | Ctrl + Shift + drag | Cmd + Shift + drag |
| Constrain marquee to square / circle | Drag, then hold Ctrl | Drag, then hold Cmd |
| Draw marquee from centre | Drag, then hold Ctrl + Alt | Drag, then hold Cmd + Option |
| Move selection marching-ants only (not contents) | Alt + drag | Option + drag |

Paint-tool combinations:

| Action while a paint tool is active | Linux / Windows | macOS |
| --- | --- | --- |
| Stroke a straight line from the last paint point | Click point A, then Shift + click point B | _same_ |
| Constrain that line to 15° increments | Shift + Ctrl + click | Shift + Cmd + click |
| Sample colour into the foreground swatch (temporary Color Picker) | Ctrl + click | Cmd + click |

Transform-tool combinations:

| Action while a transform tool is active | Linux / Windows | macOS |
| --- | --- | --- |
| Constrain to axis / aspect ratio / 15° rotation | Hold Ctrl while dragging a handle | Hold Cmd while dragging a handle |
| Transform from centre | Hold Ctrl + Alt while dragging | Hold Cmd + Option while dragging |
| Commit the transform | Return (Enter) | Return (Enter) |
| Cancel the transform | Esc | Esc |

Move-tool combinations:

| Action with the Move tool active | Linux / Windows | macOS |
| --- | --- | --- |
| Nudge active layer / selection / path by 1 px | ←, →, ↑, ↓ | ←, →, ↑, ↓ |
| Nudge by an "accelerated" step (≈25 px) | Shift + arrow | Shift + arrow |
| Pick up the layer under the cursor (instead of the active one) | (default — see **Tool Options**) | _same_ |
| Constrain motion to axis | Hold Ctrl while dragging | Hold Cmd while dragging |

> **Pan & rotate the canvas with the mouse alone** — these are GIMP-wide hard-wired bindings, *not* configurable through the action editor:
>
> | Gesture | Effect |
> | --- | --- |
> | Middle-button drag | Pan the canvas |
> | Hold Space, drag | Pan the canvas (configurable via *Preferences ▸ Image Windows ▸ Space Bar*) |
> | Ctrl + middle-drag | Rotate the canvas |
> | Mouse wheel | Scroll vertically |
> | Shift + wheel | Scroll horizontally |
> | Ctrl + wheel | Zoom in / out around the cursor |
> | Alt + wheel | Change opacity of the active layer |

---

## Inside the Text tool

These shortcuts are only live while the Text tool is editing a text layer (cursor visible inside the text box).

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Cut text | Ctrl + X | Cmd + X |
| Copy text | Ctrl + C | Cmd + C |
| Paste text | Ctrl + V | Cmd + V |
| Paste unformatted | Ctrl + Shift + V | Cmd + Shift + V |
| Toggle **Bold** on selection | Ctrl + B | Cmd + B |
| Toggle **Italic** on selection | Ctrl + I | Cmd + I |
| Toggle **Underline** on selection | Ctrl + U | Cmd + U |

> Standard text-editing keys (arrows for caret motion, Shift + arrows to extend selection, Home/End, Backspace, Delete, etc.) all behave like a normal GTK text widget.

---

## Opening, saving, exporting

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| New image | Ctrl + N | Cmd + N |
| Open… | Ctrl + O | Cmd + O |
| Open as Layers… | Ctrl + Alt + O | Cmd + Option + O |
| Open as Link Layer… | Ctrl + Alt + Shift + O | Cmd + Option + Shift + O |
| Save (always to native `.xcf`) | Ctrl + S | Cmd + S |
| Save As… (native `.xcf`) | Ctrl + Shift + S | Cmd + Shift + S |
| Export (overwrite last export target) | Ctrl + E | Cmd + E |
| Export As… (choose format) | Ctrl + Shift + E | Cmd + Shift + E |
| Close all images | Ctrl + Shift + W | Cmd + Shift + W |
| Show in File Manager | Ctrl + Alt + F | Cmd + Option + F |
| Quit | Ctrl + Q | Cmd + Q |

---

## Undo, redo, clipboard, fill

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Undo | Ctrl + Z | Cmd + Z |
| Redo | Ctrl + Y | Cmd + Y |
| Strong Undo (skip "minor" steps) | Ctrl + Shift + Z | Cmd + Shift + Z |
| Strong Redo | Ctrl + Shift + Y | Cmd + Shift + Y |
| Cut | Ctrl + X · Cut (HW key) | Cmd + X · Cut |
| Copy | Ctrl + C · Copy (HW key) | Cmd + C · Copy |
| Copy Visible (merged, all layers) | Ctrl + Shift + C · Shift + Copy | Cmd + Shift + C · Shift + Copy |
| Paste | Ctrl + V · Paste (HW key) | Cmd + V · Paste |
| Paste **In Place** (preserve original coords) | Ctrl + Alt + V · Alt + Paste | Cmd + Option + V · Option + Paste |
| Paste as New Image | Ctrl + Shift + V · Shift + Paste | Cmd + Shift + V · Shift + Paste |
| Clear (delete selected pixels) | Delete | Delete |
| Fill with **Foreground** colour | Ctrl + `,` | Cmd + `,` |
| Fill with **Background** colour | Ctrl + `.` | Cmd + `.` |
| Fill with active **Pattern** | Ctrl + `;` | Cmd + `;` |

---

## Selection commands

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Select All | Ctrl + A | Cmd + A |
| Select None (deselect) | Ctrl + Shift + A | Cmd + Shift + A |
| Invert Selection | Ctrl + I | Cmd + I |
| Cut and Float | Ctrl + Shift + L | Cmd + Shift + L |
| Selection from Path | Shift + V | Shift + V |
| Toggle Selection visibility (marching ants) | Ctrl + T | Cmd + T |

> **Float**, **Anchor**, **Grow…**, **Shrink…**, **Border…**, **Feather…**, **Sharpen**, **Remove Holes**, **Save to Channel** and the **Selection Editor** are all in the **Select** menu but ship without default bindings.

### Quick Mask

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Toggle Quick Mask mode | Shift + Q | Shift + Q |

---

## Image commands

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Duplicate Image | Ctrl + D | Cmd + D |
| Merge Visible Layers… | Ctrl + M | Cmd + M |
| Image Properties… | Alt + Return | Option + Return |

---

## Layer commands

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| New Layer… | Ctrl + Shift + N | Cmd + Shift + N |
| Duplicate Layer(s) | Ctrl + Shift + D | Cmd + Shift + D |
| Anchor Floating Layer / Mask | Ctrl + H | Cmd + H |
| Select **Top** Layer in stack | Home | Home |
| Select **Bottom** Layer in stack | End | End |
| Select **Previous** Layer (up) | Page Up | Page Up |
| Select **Next** Layer (down) | Page Down | Page Down |

> Other layer operations (raise/lower, merge down, layer-to-image-size, flatten, scale layer, …) live under the **Layer** menu but have no default keys.

---

## Paths

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Selection from Paths | Shift + V | Shift + V |

The Paths tool itself is bound to **lowercase `b`** (see Tools above).

---

## View, zoom & display

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Close View | Ctrl + W | Cmd + W |
| Fit Image in Window | Ctrl + Shift + J | Cmd + Shift + J |
| Center Image in Window | Shift + J | Shift + J |
| Shrink-Wrap window to image | Ctrl + J | Cmd + J |
| Revert Zoom (back to last user zoom) | ` (backtick / grave) | ` (backtick / grave) |
| Reset Flip & Rotate | ! (Shift + 1) | ! (Shift + 1) |
| Toggle Selection visibility | Ctrl + T | Cmd + T |
| Toggle Guides visibility | Ctrl + Shift + T | Cmd + Shift + T |
| Toggle Rulers visibility | Ctrl + Shift + R | Cmd + Shift + R |
| Fullscreen | F11 | F11 |
| Zoom In | `+` · `+` (keypad) · ZoomIn (HW key) | `+` · `+` (keypad) · ZoomIn |
| Zoom Out | `-` · `-` (keypad) · ZoomOut (HW key) | `-` · `-` (keypad) · ZoomOut |

### Explicit zoom levels

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| 1600 % (16 : 1) | 5 · 5 (keypad) | 5 · 5 (keypad) |
| 800 % (8 : 1) | 4 · 4 (keypad) | 4 · 4 (keypad) |
| 400 % (4 : 1) | 3 · 3 (keypad) | 3 · 3 (keypad) |
| 200 % (2 : 1) | 2 · 2 (keypad) | 2 · 2 (keypad) |
| 100 % (1 : 1) | 1 · 1 (keypad) | 1 · 1 (keypad) |
| 50 % (1 : 2) | Shift + 2 · Shift + 2 (keypad) | Shift + 2 · Shift + 2 (keypad) |
| 25 % (1 : 4) | Shift + 3 · Shift + 3 (keypad) | Shift + 3 · Shift + 3 (keypad) |
| 12.5 % (1 : 8) | Shift + 4 · Shift + 4 (keypad) | Shift + 4 · Shift + 4 (keypad) |
| 6.25 % (1 : 16) | Shift + 5 · Shift + 5 (keypad) | Shift + 5 · Shift + 5 (keypad) |

### Window navigation

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Next image window | Alt + Tab · Forward (HW key) | Option + Tab · Forward |
| Previous image window | Alt + Shift + Tab · Back (HW key) | Option + Shift + Tab · Back |

> macOS note: **Cmd + Tab** is owned by the OS for app switching; GIMP uses **Option + Tab** to cycle image windows on every platform.

---

## Filters

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Repeat last filter (same settings) | Ctrl + F | Cmd + F |
| Re-Show last filter (open its dialog) | Ctrl + Shift + F | Cmd + Shift + F |
| Offset… (Filters ▸ Map ▸ Offset) | Ctrl + Shift + O | Cmd + Shift + O |

---

## Help

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Help | F1 | F1 |
| Context Help (what's this?) | Shift + F1 | Shift + F1 |

---

## Opening dialogs

These open (or focus, if already docked) the named dockable dialog. See `app/actions/dialogs-actions.c` and `windows-actions.c`.

| Dialog | Linux / Windows | macOS |
| --- | --- | --- |
| Toolbox | Ctrl + B | Cmd + B |
| Layers | Ctrl + L | Cmd + L |
| Brushes | Ctrl + Shift + B | Cmd + Shift + B |
| Patterns | Ctrl + Shift + P | Cmd + Shift + P |
| Gradients | Ctrl + G | Cmd + G |
| **Search and Run a Command** (action finder) | `/` · `/` (keypad) | `/` · `/` (keypad) |

> A **lot** of other dialogs ship with no default key (Channels, Paths, Histogram, Pointer, Colors, Tool Options, Device Status, Error Console, Dashboard, …). The action search dialog (`/`) is the fastest way to reach them; you can also assign your own bindings via **Edit ▸ Keyboard Shortcuts**.

---

## Foreground / background colour

| Result | Linux / Windows | macOS |
| --- | --- | --- |
| Reset to Default (black / white) | D | D |
| Swap FG ⇄ BG | X | X |
| Foreground: previous colour from active swatch / palette | 9 | 9 |
| Foreground: next colour from active swatch / palette | 0 | 0 |

---

## Appendix

### Source-of-truth files

If a binding ever drifts from this document, these are the authoritative C arrays — they're what GIMP reads at startup to register every action and its accelerator:

| Subsystem | File |
| --- | --- |
| Tool activation keys | `app/tools/gimp<tool>tool.c` — the `_register()` call near the top |
| File menu | `app/actions/file-actions.c` |
| Edit menu | `app/actions/edit-actions.c` |
| Select menu | `app/actions/select-actions.c` |
| View menu (toggles, scroll, navigation) | `app/actions/view-actions.c` |
| View ▸ Zoom… | `app/actions/view-actions.c` (`view_zoom_actions[]`, `view_zoom_explicit_actions[]`) |
| Image menu | `app/actions/image-actions.c` |
| Layer menu | `app/actions/layers-actions.c` |
| Paths | `app/actions/paths-actions.c` |
| Filters menu | `app/actions/filters-actions.c` |
| Help | `app/actions/help-actions.c` |
| Quick Mask | `app/actions/quick-mask-actions.c` |
| Dialogs / dockables | `app/actions/dialogs-actions.c`, `app/actions/windows-actions.c` |
| Tool-options nudges (size, opacity, flow, rate) | `app/actions/tools-actions.c` |
| Text-tool editing keys | `app/actions/text-tool-actions.c` |
| FG / BG colour, palette swatches | `app/actions/context-actions.c` |
| Modifier-role definitions | `gimp_get_extend_selection_mask` / `gimp_get_modify_selection_mask` / `gimp_get_constrain_behavior_mask` / `gimp_get_toggle_behavior_mask` (libgimpwidgets) |

### What's *not* in this list

GIMP ships hundreds of actions without default bindings — alignment commands, channel/path operations, layer-stack moves (raise/lower/merge-down), most filters, every dockable that isn't in the table above, all the script-fu and python-fu scripts, and the entirety of the colour-management menu. Each is still bindable through **Edit ▸ Keyboard Shortcuts** or by editing `~/.config/GIMP/3.0/shortcutsrc`.

### Customising

- **In-app:** **Edit ▸ Keyboard Shortcuts…** — searchable, live conflict detection, per-action reset.
- **On disk:** Custom bindings are persisted to `~/.config/GIMP/3.0/shortcutsrc` (Linux/Windows portable: `%APPDATA%\GIMP\3.0\shortcutsrc`). Delete the file to revert all customisations to the defaults documented here.
- **Per-keymap themes:** **Edit ▸ Preferences ▸ Interface ▸ Keyboard Shortcuts** lets you load alternate keymaps shipped with GIMP, including a Photoshop-style preset.
