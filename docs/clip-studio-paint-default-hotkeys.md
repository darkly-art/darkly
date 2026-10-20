# Clip Studio Paint: Default Keyboard Shortcuts Reference

A reference of the default keyboard shortcuts and modifier keys shipped with **Clip Studio Paint** (CELSYS). Compiled on 2026-10-15 from the official CELSYS user guide, whose "Shortcut list" chapter describes itself as "a list of default Clip Studio Paint shortcuts" and consists of four pages:

- [Tool Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Tool_Shortcuts.htm)
- [Menu Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Menu_Shortcuts.htm) ("the default shortcuts for menu commands at initial startup")
- [Optional Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Optional_Shortcuts.htm)
- [Shortcuts usable during operation](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Shortcuts_usable_during_operation.htm)

Cross-checked against the [Japanese edition](https://help.clip-studio.com/ja-jp/manual_jp/780_shortcuts/780_shortcuts.htm) and the 2020 legacy manual. The [legacy menu page](http://www.clip-studio.com/site/gd_en/csp/userguide/csp_userguide/900_shortcut/900_shortcut_menu.htm) lists every menu command with a key column; its non-empty keys are identical to the modern Menu Shortcuts page, which is the evidence for the [No default key](#no-default-key) section.

Edition drift: the Japanese Tool page puts Zoom on `Z` (English and legacy: `/`), adds Comic (frame border) on `N`, and names the Pencil group "Sketch". The Menu and Optional pages match exactly across editions.

> **Tip**: View and rebind these in-app via **File ▸ Shortcut Settings** (`Ctrl + Shift + Alt + K`) and **File ▸ Modifier Key Settings** (`Ctrl + Shift + Alt + Y`). On macOS and tablets both live under the **Clip Studio Paint** menu.

## Conventions

- CELSYS documents Windows keys and states: "If you are using the macOS or iPad versions, replace Alt with Option and Ctrl with Command." The macOS column applies that rule.
- `+` joins keys pressed simultaneously; `,` separates alternative bindings for the same command.
- Several tools share a letter (for example Fill and Gradient on `G`). Pressing the letter again cycles through the tools that share it.
- Pressing and holding a tool's key switches to that tool only while the key is held.
- When a common modifier key and a tool's own modifier key overlap, the tool's takes precedence.
- Some keys are of Japanese-keyboard origin (`^`, `@`, `;` in zoom). They are listed as CELSYS writes them.

## Table of Contents

- [Tools](#tools)
- [Menus](#menus)
  - [Clip Studio Paint menu (macOS / tablet)](#clip-studio-paint-menu-macos--tablet)
  - [File](#file)
  - [Edit](#edit)
  - [Layer](#layer)
  - [Select](#select)
  - [View](#view)
  - [Window](#window)
  - [Help](#help)
- [Optional shortcuts (palettes and drawing color)](#optional-shortcuts-palettes-and-drawing-color)
- [Modifier keys during operation](#modifier-keys-during-operation)
  - [Common to all tools](#common-to-all-tools)
  - [Per tool](#per-tool)
- [Behaviour notes](#behaviour-notes)
- [No default key](#no-default-key)

---

## Tools

Source: [Tool Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Tool_Shortcuts.htm). Tool keys are the same on every platform.

| Tool | Key |
| --- | --- |
| Zoom | / |
| Move (Hand) | H |
| Move (Rotate) | R |
| Operation (Object) | O |
| Operation (Select layer) | D |
| Operation (Light table) | L |
| Operation (Edit timeline) | L |
| Move layer | K |
| Selection | M |
| Auto select | W |
| Eyedropper | I |
| Pen | P |
| Pencil | P |
| Brush | B |
| Airbrush | B |
| Decoration | B |
| Eraser | E |
| Blend | J |
| Distort | J |
| Fill | G |
| Gradient | G |
| Figure | U |
| Frame border | U |
| Ruler | U |
| Text | T |
| Balloon | T |
| Correct line / Remove dust | Y |

The Selection tool's sub tools are Rectangle, Ellipse, Lasso, Polyline, Selection pen, Erase selection, Shrink selection and Magnetic lasso ([Selection area tool](https://help.clip-studio.com/en-us/manual_en/330_selection/Selection_area_tool.htm)). Only the tool group has a key. The Figure tool covers lines, curves and shapes ([Figure tool](https://help.clip-studio.com/en-us/manual_en/450_figure/Figure_tool.htm)).

## Menus

Source: [Menu Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Menu_Shortcuts.htm).

### Clip Studio Paint menu (macOS / tablet)

| Command | macOS / tablet |
| --- | --- |
| Preferences | Cmd + K |
| Shortcut Settings | Cmd + Shift + Option + K |
| Modifier Key Settings | Cmd + Shift + Option + Y |
| Quit Clip Studio Paint (macOS) | Cmd + Q |

### File

| Command | Windows | macOS |
| --- | --- | --- |
| New | Ctrl + N | Cmd + N |
| Open | Ctrl + O | Cmd + O |
| Close | Ctrl + W | Cmd + W |
| Save | Ctrl + S | Cmd + S |
| Save as | Shift + Alt + S, Ctrl + Shift + S, Ctrl + Alt + S | Shift + Option + S, Cmd + Shift + S, Cmd + Option + S |
| Print | Ctrl + P | Cmd + P |
| Preferences (Windows) | Ctrl + K | _in the Clip Studio Paint menu_ |
| Shortcut Settings (Windows) | Ctrl + Shift + Alt + K | _in the Clip Studio Paint menu_ |
| Modifier Key Settings (Windows) | Ctrl + Shift + Alt + Y | _in the Clip Studio Paint menu_ |
| Quit Clip Studio Paint (Windows) | Ctrl + Q | _in the Clip Studio Paint menu_ |

### Edit

| Command | Windows | macOS |
| --- | --- | --- |
| Undo | Ctrl + Z | Cmd + Z |
| Redo | Ctrl + Y, Ctrl + Shift + Z | Cmd + Y, Cmd + Shift + Z |
| Cut | Ctrl + X, F2 | Cmd + X, F2 |
| Copy | Ctrl + C, F3 | Cmd + C, F3 |
| Paste | Ctrl + V, F4 | Cmd + V, F4 |
| Paste to shown position | Ctrl + Shift + V | Cmd + Shift + V |
| Erase | Delete, Backspace, Ctrl + Delete, Ctrl + Backspace | Delete, Backspace, Cmd + Delete, Cmd + Backspace |
| Delete Outside Selection | Shift + Delete, Shift + Backspace | Shift + Delete, Shift + Backspace |
| Fill | Alt + Delete, Alt + Backspace | Option + Delete, Option + Backspace |
| Tonal correction ▸ Hue/Saturation/Luminosity | Ctrl + U | Cmd + U |
| Tonal correction ▸ Reverse gradient (negative) | Ctrl + I | Cmd + I |
| Transform ▸ Scale/Rotate | Ctrl + T | Cmd + T |
| Transform ▸ Free Transform | Ctrl + Shift + T | Cmd + Shift + T |

### Layer

| Command | Windows | macOS |
| --- | --- | --- |
| New Raster Layer | Ctrl + Shift + N | Cmd + Shift + N |
| Create folder and insert layer | Ctrl + G | Cmd + G |
| Ungroup Layer Folder | Ctrl + Shift + G | Cmd + Shift + G |
| Layer settings ▸ Clip to Layer Below | Ctrl + Alt + G | Cmd + Option + G |
| Merge with layer below | Ctrl + E | Cmd + E |
| Merge selected layers | Shift + Alt + E | Shift + Option + E |
| Merge visible layers | Ctrl + Shift + E | Cmd + Shift + E |
| Change Selected Layer ▸ Layer Above | Alt + ] | Option + ] |
| Change Selected Layer ▸ Layer Below | Alt + [ | Option + [ |

### Select

| Command | Windows | macOS |
| --- | --- | --- |
| Select All | Ctrl + A | Cmd + A |
| Deselect | Ctrl + D | Cmd + D |
| Reselect | Ctrl + Shift + D | Cmd + Shift + D |
| Invert selected area | Ctrl + Shift + I, Shift + F7 | Cmd + Shift + I, Shift + F7 |

### View

| Command | Windows | macOS |
| --- | --- | --- |
| Rotate/Invert ▸ Rotate Left | - | - |
| Rotate/Invert ▸ Rotate right | ^ | ^ |
| Zoom in | Ctrl + Num +, Ctrl + ; | Cmd + Num +, Cmd + ; |
| Zoom out | Ctrl + Num -, Ctrl + - | Cmd + Num -, Cmd + - |
| 100% | Ctrl + Alt + 0 | Cmd + Option + 0 |
| Fit to Screen | Ctrl + 0 | Cmd + 0 |
| Reset display | Ctrl + @ | Cmd + @ |
| Ruler | Ctrl + R | Cmd + R |
| Snap to Ruler | Ctrl + 1 | Cmd + 1 |
| Snap to Special Ruler | Ctrl + 2 | Cmd + 2 |
| Snap to Grid | Ctrl + 3 | Cmd + 3 |
| Toggle special ruler snapping | Ctrl + 4 | Cmd + 4 |

### Window

| Command | Windows | macOS | iPad |
| --- | --- | --- | --- |
| Canvas ▸ Next canvas | Ctrl + Tab | Control + Tab | Shift + Control + Tab |
| Canvas ▸ Previous Canvas | Ctrl + Shift + Tab | Shift + Control + Tab | Control + Tab |
| Show/Hide All Palettes | Tab | Tab | Tab |
| Hide Title Bar / Hide Menu Bar | Shift + Tab | Shift + Tab | Shift + Tab |

### Help

| Command | Key |
| --- | --- |
| Show Menu (tablet) | T1 |
| Training and Tutorials | F1 |

## Optional shortcuts (palettes and drawing color)

Source: [Optional Shortcuts](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Optional_Shortcuts.htm). Set under the **Options** category of Shortcut Settings.

| Palette | Operation | Windows | macOS |
| --- | --- | --- | --- |
| Tool Settings ▸ Ink | Reduce opacity | Ctrl + [ | Cmd + [ |
| Tool Settings ▸ Ink | Increase opacity | Ctrl + ] | Cmd + ] |
| Tool Settings ▸ Brush tip | Reduce brush density | Ctrl + Shift + O | Cmd + Shift + O |
| Tool Settings ▸ Brush tip | Increase brush density | Ctrl + Shift + P | Cmd + Shift + P |
| Tool Settings ▸ Spraying effect | Reduce particle density | Ctrl + Shift + K | Cmd + Shift + K |
| Tool Settings ▸ Spraying effect | Increase particle density | Ctrl + Shift + L | Cmd + Shift + L |
| Tool Settings ▸ Reference | Toggle refer multiple | 0 | 0 |
| Brush Size | Select smaller preset than current | [ | [ |
| Brush Size | Select larger preset than current | ] | ] |
| Tool Group | Switch to previous tool | , | , |
| Tool Group | Switch to next tool | . | . |
| Layer Properties | Switch using layer color | Ctrl + B | Cmd + B |
| Drawing color | Switch main color and sub color | X | X |
| Drawing color | Switch drawing color and transparent color | C | C |

## Modifier keys during operation

Source: [Shortcuts usable during operation](https://help.clip-studio.com/en-us/manual_en/780_shortcuts/Shortcuts_usable_during_operation.htm). Keys are written for Windows; on macOS read Alt as Option and Ctrl as Cmd.

### Common to all tools

| Operation | Keys |
| --- | --- |
| Hand | Space + drag |
| Rotate | Shift + Space + drag, Shift + mouse wheel |
| Zoom in | Ctrl + Space + click (macOS: press Space first, then Cmd) |
| Zoom out | Alt + Space + click |
| Select layer | Ctrl + Shift + click or drag |
| Eyedropper | Right click |

### Per tool

| Tool | Operation | Keys |
| --- | --- | --- |
| Zoom | Switch between scaling up or down | Alt + click |
| Object | Add to selection | Shift + click |
| Object | Eyedropper | Alt + click |
| Select layer | Add to selection | Shift + click |
| Select layer | Deselect | Alt + click |
| Light table | Switch center point motion by clicking | Ctrl + click |
| Light table | Switch between Move and Rotate | Alt + drag |
| Edit Timeline | Switch between previous and next | Alt + click |
| Edit Timeline | Switch frame step | Shift + click |
| Move layer | Keep original image | Alt + drag |
| Move layer | Fix movement direction | Shift + drag |
| Selection area | Interlock selection and image | Ctrl + drag |
| Selection area | Delete from current selection | Alt + drag |
| Selection area | Add to current selection | Shift + drag |
| Selection area | Select from current selection | Shift + Alt + drag |
| Selection pen, Erase selection | Change brush size (plus the Selection area keys) | Ctrl + Alt + drag |
| Auto select | Interlock selection and image | Ctrl + drag |
| Auto select | Temporarily toggle multiple referring on/off | Ctrl + click |
| Auto select | Delete from current selection | Alt + click |
| Auto select | Add to current selection | Shift + click |
| Auto select | Select from current selection | Shift + Alt + click |
| Pen, Pencil, Brush, Airbrush, Decoration, Eraser | Eyedropper | Alt + click |
| Pen, Pencil, Brush, Airbrush, Decoration, Eraser | Object | Ctrl + drag |
| Pen, Pencil, Brush, Airbrush, Decoration, Eraser | Change brush size (not the Dot pen) | Ctrl + Alt + drag |
| Pen, Pencil, Brush, Airbrush, Decoration, Eraser | Draw straight line | Shift + drag |
| Blend (except Copy stamp) | Eyedropper | Alt + click |
| Blend (except Copy stamp) | Object | Ctrl + drag |
| Blend (except Copy stamp) | Change brush size | Ctrl + Alt + drag |
| Blend (except Copy stamp) | Draw straight line | Shift + drag |
| Copy stamp | Specify reference point | Alt + click |
| Copy stamp | Object | Ctrl + drag |
| Copy stamp | Change brush size | Ctrl + Alt + drag |
| Copy stamp | Draw straight line | Shift + drag |
| Liquify | Object | Ctrl + drag |
| Liquify | Flip liquify mode | Alt + drag |
| Liquify | Draw straight line | Shift + drag |
| Liquify | Change brush size | Ctrl + Alt + drag |
| Fill | Object | Ctrl + drag |
| Fill | Eyedropper | Alt + click |
| Fill | Toggle multiple referring on/off | Shift + click |
| Paint unfilled area | Change brush size (plus the Fill keys) | Ctrl + Alt + drag |
| Gradient | Object | Ctrl + drag |
| Gradient | Eyedropper | Alt + click |
| Freeform gradient group | Toggle multiple referring on/off (plus the Gradient keys) | Shift + click |
| Figure | Eyedropper | Alt + click |
| Figure | Object | Ctrl + drag |
| Figure (not Lasso fill, Speed lines, Focus lines) | Change brush size | Ctrl + Alt + drag |
| Frame border | Eyedropper (not Divide frame border) | Alt + click |
| Frame border | Object | Ctrl + drag |
| Frame border | Change brush size (not Divide frame border) | Ctrl + Alt + drag |
| Ruler | Object | Ctrl + drag |
| Ruler pen | Draw straight line (plus the Ruler keys) | Shift + drag |
| Text | Eyedropper | Alt + click |
| Text | Object | Ctrl + drag |
| Balloon (not tails, not Flash) | Eyedropper | Alt + click |
| Balloon (not tails, not Flash) | Object | Ctrl + drag |
| Balloon (not tails, not Flash) | Change brush size | Ctrl + Alt + drag |
| Balloon tail, Thought balloon tail | Eyedropper | Alt + click |
| Flash group | Eyedropper | Alt + click |
| Flash group | Object | Ctrl + drag |
| Correct line (not Control point, Redraw vector line, Remove dust) | Object | Ctrl + drag |
| Correct line (not Control point, Redraw vector line, Remove dust) | Change brush size | Ctrl + Alt + drag |
| Control point | Object | Ctrl + drag |
| Control point | Delete control point | Alt + click |
| Control point | Add control point | Shift + click |
| Control point | Switch corner | Shift + Alt + click |
| Redraw vector line | Object | Ctrl + drag |
| Remove dust, Fill leftover | Object | Ctrl + drag |
| Remove dust, Fill leftover | Delete control point | Alt + click |
| Remove dust, Fill leftover | Switch referring multiple layers | Shift + drag |
| Select dust | Interlock selection and image | Ctrl + drag |
| Select dust | Delete from current selection | Alt + drag |
| Select dust | Add to current selection | Shift + drag |
| Select dust | Select from current selection | Shift + Alt + drag |

## Behaviour notes

Behaviour that the shortcut tables alone do not describe, from the rest of the user guide.

- **Paste position** ([Cut/Copy/Paste](https://help.clip-studio.com/en-us/manual_en/270_canvas/Cut__47_Copy__47_Paste.htm)): Paste places the image at the same location it was cut from. Paste to shown position places it at the center of the canvas. Holding Alt and dragging a layer in the Layer palette duplicates it.
- **Selection from a layer** ([Selection from layer](https://help.clip-studio.com/en-us/manual_en/330_selection/Selection_from_layer.htm)): Ctrl + click on a layer in the Layer palette creates a selection from it. Shift + Ctrl + click adds several layers. Ctrl + click on a ruler icon creates a selection from the ruler.
- **Transform modes** ([Types of transformations](https://help.clip-studio.com/en-us/manual_en/360_transform/Types_of_transformations.htm)): Free Transform moves a dragged handle along both axes (Shift constrains it to one). Distort moves in a single direction, Skew slants, and Perspective moves corners in opposite directions. Enter confirms a transformation and Esc cancels it.
- **Polyline selection** ([Selection area tool](https://help.clip-studio.com/en-us/manual_en/330_selection/Selection_area_tool.htm)): Delete removes the previous corner point and Esc cancels the selection in progress.
- **Transparent color** ([Picking colors](https://help.clip-studio.com/en-us/manual_en/060_pc/Picking_colors.htm)): drawing with the transparent color makes any drawing tool behave like an eraser.
- **Eyedropper reference** ([Eyedropper tool guide](http://www.clip-studio.com/site/gd_en/csp/toolguide/csp_toolguide/100_reference/Eyedropper.htm)): the Eyedropper can refer to the current layer, the top non-transparent layer, or the image as displayed. The guide does not state which is the default.

## No default key

These commands exist in the menus but ship with an empty key column in the [legacy full menu list](http://www.clip-studio.com/site/gd_en/csp/userguide/csp_userguide/900_shortcut/900_shortcut_menu.htm). The list covers the commands an artist coming from another editor is most likely to look for.

| Menu | Commands |
| --- | --- |
| Edit | Tonal correction ▸ Brightness/Contrast, Level Correction, Tone curve, Color balance, Posterization, Binarization, Gradient map; Transform ▸ Flip Horizontal, Flip Vertical |
| Edit (canvas) | Change Image Resolution, Change Canvas Size, Crop, Rotate/Flip canvas ▸ Rotate 180, Flip Horizontal, Flip Vertical |
| Layer | New Layer ▸ Raster Layer (the menu path; New Raster Layer itself is Ctrl + Shift + N), New Layer Folder, Duplicate Layer, Delete Layer, Layer mask ▸ Mask Outside Selection, Mask selection, Delete Mask, Apply Mask to Layer, Lock layer, Show layer, Merge visible to new layer |
| Select | Expand Selected Area |
| View | Rotate/Invert ▸ Rotate 90, Rotate 180, Flip Horizontal, Flip Vertical |
