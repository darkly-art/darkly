# Krita — Default Keyboard Shortcuts & Canvas Inputs

> Reference for **Krita default** keybindings as shipped in the source tree.
> Generated from:
>
> - `krita/krita/*.action` and `krita/plugins/**/*.action` (menu/tool keyboard shortcuts)
> - `krita/krita/data/input/kritadefault.profile` (canvas pointer/key chords for paint, pan, zoom, rotate, color-pick)
> - `krita/libs/ui/input/kis_*_action.{h,cpp}` (canvas input action mode tables)
>
> Krita ships alternate profiles too — *Photoshop Compatible*, *Clip Studio Paint Compatible*,
> *Paint Tool SAI Compatible*, *Tablet Pro* — selectable under **Settings → Configure Krita →
> Canvas Input Settings**. This document covers only the default profile.

Shortcut sources you can rebind in Krita:

- **Keyboard shortcuts** — `Settings → Configure Krita → Keyboard Shortcuts`.
  Map menu items, tool activators, layer ops, blending modes, scripts to single keys / chords.
- **Canvas Input** — `Settings → Configure Krita → Canvas Input Settings`.
  Map pointer + key chords (mouse-drag with modifiers, wheel, gestures) to the canvas action modes
  (pan, zoom, rotate, pick color, alt-invoke, change brush size, etc.).

---

## Table of contents

- [Canvas input (pointer + key chords)](#canvas-input-pointer--key-chords)
- [File](#menu--file)
- [Edit](#menu--edit)
- [View](#menu--view)
- [Image](#menu--image)
- [Select](#menu--select)
- [Filter](#menu--filter)
- [Settings / Window / Help](#menu--settings--window--help)
- [Layers](#krita--layers)
- [Painting (general)](#krita--painting)
- [Blending modes](#krita--blending-modes)
- [Filters (direct hotkeys)](#krita--filters-direct-hotkeys)
- [General](#krita--general)
- [Animation](#krita--animation)
- [Color selectors (Wide-Gamut, MyPaint, etc.)](#krita--color-selectors)
- [Tools — generic switchers](#tools--tool-shortcuts)
- [Tool — Interaction (vector)](#tools--interaction-tool)
- [Tool — Path](#tools--path-tool)
- [Tool — SVG Text](#svg-tools--svg-text-tool)
- [Scripts — Ten Brushes, Ten Scripts, Mutator](#scripts)
- [S-Pen Actions](#krita--s-pen-actions)
- [Recorder](#recorder--recorder)

---

## Canvas input (pointer + key chords)

These bindings live on the canvas itself: held while pointing, they switch the canvas action mode.
Multiple bindings per mode are common (mouse + touch + keyboard each may map the same mode).

### Tool Invocation

| Mode | Binding |
|------|---------|
| Activate (paint stroke / use tool) | `Left mouse drag` |
| Confirm | `Enter (numpad)` · `Return` |
| Cancel | `Esc` |
| Line tool (temporary) | `V + Left mouse drag` |

### Pan

| Mode | Binding |
|------|---------|
| Pan (drag) | `Two-finger tap` · `Space + Left mouse drag` · `Middle mouse drag` · `Trackpad Pan` · `One-finger drag` |

### Zoom

| Mode | Binding |
|------|---------|
| Zoom (drag) | `One-finger tap` |
| Zoom in to cursor | `+` · `=` · `Wheel Up` |
| Zoom out from cursor | `-` · `Wheel Down` |
| Zoom to 100% | `1` |
| Fit to view | `2` |
| Fit to view width | `3` |
| Relative zoom (cursor-anchored) | `Ctrl + Middle mouse drag` · `Ctrl + Space + Left mouse drag` |
| Relative discrete zoom | `Ctrl + Alt + Middle mouse drag` · `Ctrl + Alt + Space + Left mouse drag` |

### Rotate

| Mode | Binding |
|------|---------|
| Rotate (drag) | `Shift + Space + Left mouse drag` · `Shift + Middle mouse drag` · `Three-finger tap` |
| Discrete rotate (snap 15°) | `Shift + Alt + Space + Left mouse drag` |
| Rotate left (step) | `4` |
| Rotate right (step) | `6` |
| Reset rotation | `5` |

### Change Primary Setting (Brush Size)

| Mode | Binding |
|------|---------|
| Change brush size (normal) | `Shift + Left mouse drag` |

### Alternate Invocation (color pick / alt stroke)

| Mode | Binding |
|------|---------|
| Primary alt-invoke | `Ctrl + Shift + Left mouse drag` |
| Secondary alt-invoke | `Alt + Shift + Left mouse drag` |
| Sample FG color from current layer | `Alt + Ctrl + Left mouse drag` |
| Sample BG color from current layer | `Ctrl + Alt + Right mouse drag` |
| Sample FG color from merged image | `Ctrl + Left mouse drag` · `One-finger hold` |
| Sample BG color from merged image | `Ctrl + Right mouse drag` |

### Popup Widget (brush palette)

| Mode | Binding |
|------|---------|
| Show current tool's popup widget | `Right mouse drag` · `One-finger tap` |

### Select Layer by Picking

| Mode | Binding |
|------|---------|
| Pick top layer (replace selection) | `R + Left mouse drag` |
| Pick top layer (add to selection) | `Shift + R + Left mouse drag` |

### Zoom + Rotate (touch)

| Mode | Binding |
|------|---------|
| Continuous rotate | `Two-finger drag` |

### Switch Animation Frame

| Mode | Binding |
|------|---------|
| Next frame | `→` |
| Previous frame | `←` |

### Exposure / Gamma (HDR view tweak)

| Mode | Binding |
|------|---------|
| Exposure (drag) | `Y + Left mouse drag` |

### Touch Gestures (multi-finger taps)

| Mode | Binding |
|------|---------|
| Undo | `Two-finger tap` |
| Redo | `Three-finger tap` |
| Toggle canvas-only mode | `Four-finger tap` |

> **Read this table as held-modifiers.** `Shift + Left mouse drag` means: hold Shift, drag with
> the left button — release Shift and the canvas returns to normal tool invocation. Touch
> gestures (tap / drag / hold) require a touch screen; trackpad pan requires a multi-touch trackpad.

---


## Menu — File

**9 default shortcuts** (21 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+Alt+S` | Save Incremental Version | `save_incremental_version` |
| `Ctrl+N` | New... | `file_new` |
| `Ctrl+O` | Open... | `file_open` |
| `Ctrl+Q` | Quit | `file_quit` |
| `Ctrl+S` | Save | `file_save` |
| `Ctrl+Shift+S` | Save As... | `file_save_as` |
| `Ctrl+Shift+W` | Close All | `file_close_all` |
| `Ctrl+W` | Close | `file_close` |
| `F4` | Save Incremental Backup | `save_incremental_backup` |

<details><summary>12 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Create Copy From Current Image | `create_copy` |
| Create Template From Image... | `create_template` |
| Document Information | `file_documentinfo` |
| Export Advanced... | `file_export_advanced` |
| Export... | `file_export_file` |
| Import animation frames... | `file_import_animation` |
| Import video animation... | `file_import_video_animation` |
| Open Recent | `file_open_recent` |
| Open existing Document as Untitled Document... | `file_import_file` |
| Render Animation Again | `render_animation_again` |
| Render Animation... | `render_animation` |
| Sessions... | `file_sessions` |

</details>


## Menu — Edit

**13 default shortcuts** (22 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Backspace` | Clear | `clear` |
| `Ctrl+Alt+V` | Paste at Cursor | `paste_at` |
| `Ctrl+C` | Copy | `edit_copy` |
| `Ctrl+Shift+C` | Copy merged | `copy_merged` |
| `Ctrl+Shift+N` | Paste into New Image | `paste_new` |
| `Ctrl+Shift+R` | Paste as Reference Image | `paste_as_reference` |
| `Ctrl+Shift+V` | Paste into Active Layer | `paste_into` |
| `Ctrl+Shift+Z` | Redo | `edit_redo` |
| `Ctrl+V` | Paste | `edit_paste` |
| `Ctrl+X` | Cut | `edit_cut` |
| `Ctrl+Z` | Undo | `edit_undo` |
| `Del` | Fill with Background Color | `fill_selection_background_color` |
| `Shift+Backspace` | Fill with Foreground Color | `fill_selection_foreground_color` |

<details><summary>9 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Copy (sharp) | `copy_sharp` |
| Copy Layer Style | `copy_layer_style` |
| Cut (sharp) | `cut_sharp` |
| Delete keyframe | `delete_keyframe` |
| Fill with Pattern | `fill_selection_pattern` |
| Paste Layer Style | `paste_layer_style` |
| Paste Shape Style | `paste_shape_style` |
| Stroke Selection... | `stroke_selection` |
| Stroke selected shapes | `stroke_shapes` |

</details>


## Menu — View

**16 default shortcuts** (46 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Alt+M` | Mirror View Around Cursor | `mirror_canvas_around_cursor` |
| `Ctrl+-` | Zoom Out | `view_zoom_out` |
| `Ctrl+0` | Zoom to 100% | `zoom_to_100pct` |
| `Ctrl+=; Ctrl++` | Zoom In | `view_zoom_in` |
| `Ctrl+Shift+'` | Show Grid | `view_grid` |
| `Ctrl+Shift+;` | Snap To Grid | `view_snap_to_grid` |
| `Ctrl+Shift+F` | Full Screen Mode | `fullscreen` |
| `Ctrl+Shift+Y` | Out of Gamut Warnings | `gamutCheck` |
| `Ctrl+Y` | Soft Proofing | `softProof` |
| `Ctrl+[` | Rotate Canvas Left | `rotate_canvas_left` |
| `Ctrl+]` | Rotate Canvas Right | `rotate_canvas_right` |
| `M` | Mirror View | `mirror_canvas` |
| `Shift+L` | Instant Preview Mode | `level_of_detail_mode` |
| `Shift+W` | Wrap Around Mode | `wrap_around_mode` |
| `Shift+s` | Show Snap Options Popup | `show_snap_options_popup` |
| `Tab` | Show Canvas Only | `view_show_canvas_only` |

<details><summary>30 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Detach Canvas | `view_detached_canvas` |
| Fit to View | `zoom_to_fit` |
| Fit to View Height | `zoom_to_fit_height` |
| Fit to View Width | `zoom_to_fit_width` |
| Lock Guides | `view_lock_guides` |
| Mirror View Around Canvas | `mirror_canvas_around_canvas` |
| Reset Canvas Rotation | `reset_canvas_rotation` |
| Reset Display | `reset_display` |
| Rulers Track Pointer | `rulers_track_mouse` |
| Show Assistant Previews | `view_toggle_assistant_previews` |
| Show Guides | `view_show_guides` |
| Show Painting Assistants | `view_toggle_painting_assistants` |
| Show Pixel Grid | `view_pixel_grid` |
| Show Reference Images | `view_toggle_reference_images` |
| Show Rulers | `view_ruler` |
| Show Status Bar | `showStatusBar` |
| Snap Bounding Box | `view_snap_bounding_box` |
| Snap Extension | `view_snap_extension` |
| Snap Image Bounds | `view_snap_image_bounds` |
| Snap Image Center | `view_snap_image_center` |
| Snap Intersection | `view_snap_intersection` |
| Snap Node | `view_snap_node` |
| Snap Orthogonal | `view_snap_orthogonal` |
| Snap Pixel | `view_snap_to_pixel` |
| Snap to Guides | `view_snap_to_guides` |
| Toggle Fit to View | `toggle_zoom_to_fit` |
| View Print Size | `view_print_size` |
| Wrap Around Both Directions | `wrap_around_hv_axis` |
| Wrap Around Horizontally | `wrap_around_h_axis` |
| Wrap Around Vertically | `wrap_around_v_axis` |

</details>


## Menu — Image

**2 default shortcuts** (19 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+Alt+C` | Resize Canvas... | `canvassize` |
| `Ctrl+Alt+I` | Scale Image To New Size... | `imagesize` |

<details><summary>17 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Convert Image Color Space... | `imagecolorspaceconversion` |
| Image Background Color and Transparency... | `image_color` |
| Image Split | `imagesplit` |
| Mirror Image Horizontally | `mirrorImageHorizontal` |
| Mirror Image Vertically | `mirrorImageVertical` |
| Offset Image... | `offsetimage` |
| Properties... | `image_properties` |
| Purge Unused Image Data | `purge_unused_image_data` |
| Rotate Image 180° | `rotateImage180` |
| Rotate Image 90° to the Left | `rotateImageCCW90` |
| Rotate Image 90° to the Right | `rotateImageCW90` |
| Rotate Image... | `rotateimage` |
| Separate Image... | `separate` |
| Shear Image... | `shearimage` |
| Trim to Current Layer | `resizeimagetolayer` |
| Trim to Image Size | `trim_to_image` |
| Trim to Selection | `resizeimagetoselection` |

</details>


## Menu — Select

**5 default shortcuts** (19 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+A` | Select All | `select_all` |
| `Ctrl+H` | Display Selection | `toggle_display_selection` |
| `Ctrl+Shift+A` | Deselect | `deselect` |
| `Ctrl+Shift+D` | Reselect | `reselect` |
| `Shift+F6` | Feather Selection... | `featherselection` |

<details><summary>14 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Border Selection... | `borderselection` |
| Convert Shapes to Vector Selection | `convert_shapes_to_vector_selection` |
| Convert to Raster Selection | `convert_to_raster_selection` |
| Convert to Vector Selection | `convert_to_vector_selection` |
| Edit Selection | `edit_selection` |
| Grow Selection... | `growselection` |
| Scale... | `selectionscale` |
| Select Opaque (Add) | `selectopaque_add` |
| Select Opaque (Intersect) | `selectopaque_intersect` |
| Select Opaque (Replace) | `selectopaque` |
| Select Opaque (Subtract) | `selectopaque_subtract` |
| Select from Color Range... | `colorrange` |
| Shrink Selection... | `shrinkselection` |
| Smooth | `smoothselection` |

</details>


## Menu — Filter

**1 default shortcut** (13 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+F` | Apply Filter Again | `filter_apply_again` |

<details><summary>12 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Adjust | `adjust_filters` |
| Apply Filter Again (Reprompt) | `filter_apply_reprompt` |
| Artistic | `artistic_filters` |
| Blur | `blur_filters` |
| Colors | `color_filters` |
| Edge Detection | `edge_filters` |
| Emboss | `emboss_filters` |
| Enhance | `enhance_filters` |
| Map | `map_filters` |
| Other | `other_filters` |
| Re-apply the last G'MIC filter | `QMicAgain` |
| Start G'MIC-Qt | `QMic` |

</details>


## Menu — Settings / Window / Help

**1 default shortcut** (34 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `F1` | Krita Handbook | `help_contents` |

<details><summary>33 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| About KDE | `help_about_kde` |
| About Krita | `help_about_app` |
| Active Author Profile | `settings_active_author` |
| Brush option slider 1 | `brushslider1` |
| Brush option slider 2 | `brushslider2` |
| Brush option slider 3 | `brushslider3` |
| Brush option slider 4 | `brushslider4` |
| Brush option slider 5 | `brushslider5` |
| Choose Background Color | `chooseBackgroundColor` |
| Choose Foreground Color | `chooseForegroundColor` |
| Color | `dual` |
| Configure Krita... | `options_configure` |
| Configure Shortcuts... | `options_configure_keybinding` |
| Configure Toolbars... | `options_configure_toolbars` |
| Dockers | `settings_dockers_menu` |
| Gradients | `gradients` |
| Layouts | `select_layout` |
| Manage Resource Libraries... | `manage_bundles` |
| Manage Resources... | `manage_resources` |
| Mirror | `mirror_actions` |
| New Window | `view_newwindow` |
| Next | `windows_next` |
| Patterns | `patterns` |
| Previous | `windows_previous` |
| Report Bug... | `help_report_bug` |
| Reset All Settings | `reset_configurations` |
| Show Docker Titlebars | `view_toggledockertitlebars` |
| Show Dockers | `view_toggledockers` |
| Styles | `style_menu` |
| Switch Application Language... | `switch_application_language` |
| Themes | `theme_menu` |
| Window | `window` |
| Workspaces | `workspaces` |

</details>


## Krita — Layers

**18 default shortcuts** (97 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `;` | Activate previously selected layer | `switchToPreviouslyActiveNode` |
| `Ctrl+Alt+G` | Quick Ungroup | `quick_ungroup` |
| `Ctrl+Alt+J` | Copy Selection to New Layer | `copy_selection_to_new_layer` |
| `Ctrl+E` | Merge with Layer Below | `merge_layer` |
| `Ctrl+G` | Quick Group | `create_quick_group` |
| `Ctrl+J` | Duplicate Layer or Mask | `duplicatelayer` |
| `Ctrl+PgDown` | Move Layer or Mask Down | `move_layer_down` |
| `Ctrl+PgUp` | Move Layer or Mask Up | `move_layer_up` |
| `Ctrl+Shift+E` | Flatten image | `flatten_image` |
| `Ctrl+Shift+G` | Quick Clipping Group | `create_quick_clipping_group` |
| `Ctrl+Shift+J` | Cut Selection to New Layer | `cut_selection_to_new_layer` |
| `F2` | Rename current layer | `RenameCurrentLayer` |
| `F3` | Properties... | `layer_properties` |
| `Insert` | Add Paint Layer | `add_new_paint_layer` |
| `PgDown` | Activate previous layer | `activatePreviousLayer` |
| `PgUp` | Activate next layer | `activateNextLayer` |
| `Shift+Delete` | Remove Layer | `remove_layer` |
| `Shift+Insert` | Add Vector Layer | `add_new_shape_layer` |

<details><summary>79 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Activate next sibling layer, skipping over groups. | `activateNextSiblingLayer` |
| Activate previous sibling layer, skipping over groups. | `activatePreviousSiblingLayer` |
| Add Clone Layer | `add_new_clone_layer` |
| Add Color Overlay Mask | `add_new_fast_color_overlay_mask` |
| Add Colorize Mask | `add_new_colorize_mask` |
| Add File Layer... | `add_new_file_layer` |
| Add Fill Layer... | `add_new_fill_layer` |
| Add Filter Layer... | `add_new_adjustment_layer` |
| Add Filter Mask... | `add_new_filter_mask` |
| Add Group Layer | `add_new_group_layer` |
| Add Local Selection | `add_new_selection_mask` |
| Add Transform Mask | `add_new_transform_mask` |
| Add Transparency Mask | `add_new_transparency_mask` |
| All Layers | `select_all_layers` |
| Alpha into Mask | `split_alpha_into_mask` |
| Clones Array... | `clones_array` |
| Convert Group to Animated Layer | `convert_group_to_animated` |
| Convert Layer Color Space... | `layercolorspaceconversion` |
| Convert to Animated Layer | `convert_to_animated` |
| Convert to File Layer... | `convert_to_file_layer` |
| Convert to Filter Mask... | `convert_to_filter_mask` |
| Convert to Paint Layer | `convert_to_paint_layer` |
| Convert to Selection Mask | `convert_to_selection_mask` |
| Convert to Transparency Mask | `convert_to_transparency_mask` |
| Copy Layer | `copy_layer_clipboard` |
| Cut Layer | `cut_layer_clipboard` |
| Edit metadata... | `EditLayerMetaData` |
| Flatten Layer | `flatten_layer` |
| Force Palette Colors | `force_palette_colors` |
| Histogram... | `histogram` |
| Import Layer... | `import_layer_from_file` |
| Invisible Layers | `select_invisible_layers` |
| Isolate Active Group | `isolate_active_group` |
| Isolate Active Layer | `isolate_active_layer` |
| Layer Style... | `layer_style` |
| Locked Layers | `select_locked_layers` |
| Mirror All Layers Horizontally | `mirrorAllNodesX` |
| Mirror All Layers Vertically | `mirrorAllNodesY` |
| Mirror Layers Horizontally | `mirrorNodeX` |
| Mirror Layers Vertically | `mirrorNodeY` |
| Move into next group | `LayerGroupSwitcher/next` |
| Move into previous group | `LayerGroupSwitcher/previous` |
| New Layer From Visible | `new_from_visible` |
| Offset Layers... | `offsetlayer` |
| Paste Layer | `paste_layer_from_clipboard` |
| Reference Image from Layer | `create_reference_image_from_active_layer` |
| Reference Image from Visible | `create_reference_image_from_visible_canvas` |
| Rotate All Layers 180° | `rotateAllLayers180` |
| Rotate All Layers 90° to the Left | `rotateAllLayersCCW90` |
| Rotate All Layers 90° to the Right | `rotateAllLayersCW90` |
| Rotate All Layers... | `rotateAllLayers` |
| Rotate Layers 180° | `rotateLayer180` |
| Rotate Layers 90° to the Left | `rotateLayerCCW90` |
| Rotate Layers 90° to the Right | `rotateLayerCW90` |
| Rotate Layers... | `rotatelayer` |
| Save Group Layers... | `save_groups_as_images` |
| Save Layer/Mask... | `save_node_as_image` |
| Save Merged... | `split_alpha_save_merged` |
| Save Vector Layer as SVG... | `save_vector_node_to_svg` |
| Scale All Layers to new Size... | `scaleAllLayers` |
| Scale Layers to new Size... | `layersize` |
| Set Copy From... | `set-copy-from` |
| Shear All Layers... | `shearAllLayers` |
| Shear Layers... | `shearlayer` |
| Split Layer... | `layersplit` |
| Toggle Layer Soloing | `toggle_layer_soloing` |
| Toggle layer alpha | `toggle_layer_alpha_lock` |
| Toggle layer alpha inheritance | `toggle_layer_inherit_alpha` |
| Toggle layer lock | `toggle_layer_lock` |
| Toggle layer visibility | `toggle_layer_visibility` |
| Unify Layers Color Space | `unifylayerscolorspace` |
| Unlocked Layers | `select_unlocked_layers` |
| Visible Layers | `select_visible_layers` |
| Wavelet Decompose ... | `waveletdecompose` |
| Write as Alpha | `split_alpha_write` |
| as Filter Mask... | `import_layer_as_filter_mask` |
| as Paint Layer... | `import_layer_as_paint_layer` |
| as Selection Mask... | `import_layer_as_selection_mask` |
| as Transparency Mask... | `import_layer_as_transparency_mask` |

</details>


## Krita — Painting

**16 default shortcuts** (63 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `,` | Next Favourite Preset | `next_favorite_preset` |
| `.` | Previous Favourite Preset | `previous_favorite_preset` |
| `/` | Switch to Previous Preset | `previous_preset` |
| `Ctrl+Backspace` | Fill with Background Color (Opacity) | `fill_selection_background_color_opacity` |
| `Ctrl+Shift+Backspace` | Fill with Foreground Color (Opacity) | `fill_selection_foreground_color_opacity` |
| `Ctrl+Shift+L` | Toggle Snap To Assistants | `toggle_assistant` |
| `D` | Set Foreground and Background Colors to Black and White | `reset_fg_bg` |
| `E` | Set eraser mode | `erase_action` |
| `I` | Decrease Opacity | `decrease_opacity` |
| `K` | Make brush color darker | `make_brush_color_darker` |
| `L` | Make brush color lighter | `make_brush_color_lighter` |
| `O` | Increase Opacity | `increase_opacity` |
| `Shift+Z` | Undo Polygon Selection Points | `undo_polygon_selection` |
| `X` | Swap Foreground and Background Colors | `toggle_fg_bg` |
| `[` | Decrease Brush Size | `decrease_brush_size` |
| `]` | Increase Brush Size | `increase_brush_size` |

<details><summary>47 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Brush Smoothing: Basic | `set_simple_brush_smoothing` |
| Brush Smoothing: Disabled | `set_no_brush_smoothing` |
| Brush Smoothing: Pixel | `set_pixel_perfect_smoothing` |
| Brush Smoothing: Stabilizer | `set_stabilizer_brush_smoothing` |
| Brush Smoothing: Weighted | `set_weighted_brush_smoothing` |
| Convert to Shape | `convert_selection_to_shape` |
| Decrease Fade | `decrease_fade` |
| Decrease Flow | `decrease_flow` |
| Decrease Scatter | `decrease_scatter` |
| Fill with Pattern (Opacity) | `fill_selection_pattern_opacity` |
| Hide Brushes and Stuff Toolbar | `BrushesAndStuff` |
| Hide Mirror X Line | `mirrorX-hideDecorations` |
| Hide Mirror Y Line | `mirrorY-hideDecorations` |
| Horizontal Mirror Tool | `hmirror_action` |
| Increase Fade | `increase_fade` |
| Increase Flow | `increase_flow` |
| Increase Scatter | `increase_scatter` |
| Lock X Line | `mirrorX-lock` |
| Lock Y Line | `mirrorY-lock` |
| Make brush color more blue | `make_brush_color_bluer` |
| Make brush color more desaturated | `make_brush_color_desaturated` |
| Make brush color more green | `make_brush_color_greener` |
| Make brush color more red | `make_brush_color_redder` |
| Make brush color more saturated | `make_brush_color_saturated` |
| Make brush color more yellow | `make_brush_color_yellower` |
| Move to Canvas Center X | `mirrorX-moveToCenter` |
| Move to Canvas Center Y | `mirrorY-moveToCenter` |
| Preserve Alpha | `preserve_alpha` |
| Reload Original Preset | `reload_preset_action` |
| Rotate brush tip clockwise | `rotate_brush_tip_clockwise` |
| Rotate brush tip clockwise (precise) | `rotate_brush_tip_clockwise_precise` |
| Rotate brush tip counter-clockwise | `rotate_brush_tip_counter_clockwise` |
| Rotate brush tip counter-clockwise (precise) | `rotate_brush_tip_counter_clockwise_precise` |
| Select brush preset | `brush_select_preset_action` |
| Select eraser preset | `eraser_select_preset_action` |
| Selection Mode: Add | `selection_tool_mode_add` |
| Selection Mode: Intersect | `selection_tool_mode_intersect` |
| Selection Mode: Replace | `selection_tool_mode_replace` |
| Selection Mode: Subtract | `selection_tool_mode_subtract` |
| Shift brush color hue clockwise | `shift_brush_color_clockwise` |
| Shift brush color hue counter-clockwise | `shift_brush_color_counter_clockwise` |
| Show Global Selection Mask | `show-global-selection-mask` |
| Toggle Brush Outline | `toggle_brush_outline` |
| Toggle Selection Display Mode | `toggle-selection-overlay-mode` |
| Toggle eraser preset | `eraser_preset_action` |
| Use Pen Pressure | `disable_pressure` |
| Vertical Mirror Tool | `vmirror_action` |

</details>


## Krita — Blending Modes

**28 default shortcuts** (28 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Alt+Shift++` | Next Blending Mode | `Next Blending Mode` |
| `Alt+Shift+-` | Previous Blending Mode | `Previous Blending Mode` |
| `Alt+Shift+A` | Select Linear Burn Blending Mode | `Select Linear Burn Blending Mode` |
| `Alt+Shift+B` | Select Color Burn Blending Mode | `Select Color Burn Blending Mode` |
| `Alt+Shift+C` | Select Color Blending Mode | `Select Color Blending Mode` |
| `Alt+Shift+D` | Select Color Dodge Blending Mode | `Select Color Dodge Blending Mode` |
| `Alt+Shift+E` | Select Difference Blending Mode | `Select Difference Blending Mode` |
| `Alt+Shift+F` | Select Soft Light Blending Mode | `Select Soft Light Blending Mode` |
| `Alt+Shift+G` | Select Lighten Blending Mode | `Select Lighten Blending Mode` |
| `Alt+Shift+H` | Select Hard Light Blending Mode | `Select Hard Light Blending Mode` |
| `Alt+Shift+I` | Select Dissolve Blending Mode | `Select Dissolve Blending Mode` |
| `Alt+Shift+J` | Select Linear Light Blending Mode | `Select Linear Light Blending Mode` |
| `Alt+Shift+K` | Select Darken Blending Mode | `Select Darken Blending Mode` |
| `Alt+Shift+L` | Select Hard Mix Blending Mode | `Select Hard Mix Blending Mode` |
| `Alt+Shift+M` | Select Multiply Blending Mode | `Select Multiply Blending Mode` |
| `Alt+Shift+N` | Select Normal Blending Mode | `Select Normal Blending Mode` |
| `Alt+Shift+O` | Select Overlay Blending Mode | `Select Overlay Blending Mode` |
| `Alt+Shift+P` | Select Hard Overlay Blending Mode | `Select Hard Overlay Blending Mode` |
| `Alt+Shift+Q` | Select Behind Blending Mode | `Select Behind Blending Mode` |
| `Alt+Shift+R` | Select Clear Blending Mode | `Select Clear Blending Mode` |
| `Alt+Shift+S` | Select Screen Blending Mode | `Select Screen Blending Mode` |
| `Alt+Shift+T` | Select Saturation Blending Mode | `Select Saturation Blending Mode` |
| `Alt+Shift+U` | Select Hue Blending Mode | `Select Hue Blending Mode` |
| `Alt+Shift+V` | Select Vivid Light Blending Mode | `Select Vivid Light Blending Mode` |
| `Alt+Shift+W` | Select Linear Dodge Blending Mode | `Select Linear Dodge Blending Mode` |
| `Alt+Shift+X` | Select Exclusion Blending Mode | `Select Exclusion Blending Mode` |
| `Alt+Shift+Y` | Select Luminosity Blending Mode | `Select Luminosity Blending Mode` |
| `Alt+Shift+Z` | Select Pin Light Blending Mode | `Select Pin Light Blending Mode` |


## Krita — Filters (direct hotkeys)

**6 default shortcuts** (54 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+B` | Color Balance... | `krita_filter_colorbalance` |
| `Ctrl+I` | Invert | `krita_filter_invert` |
| `Ctrl+L` | Levels... | `krita_filter_levels` |
| `Ctrl+M` | Color Adjustment curves... | `krita_filter_perchannel` |
| `Ctrl+Shift+U` | Desaturate | `krita_filter_desaturate` |
| `Ctrl+U` | HSV Adjustment... | `krita_filter_hsvadjustment` |

<details><summary>48 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Auto Contrast | `krita_filter_autocontrast` |
| Blur... | `krita_filter_blur` |
| Bottom Edge Detection | `krita_filter_bottom edge detections` |
| Burn | `krita_filter_burn` |
| Color Transfer... | `krita_filter_colortransfer` |
| Color to Alpha... | `krita_filter_colortoalpha` |
| Cross-channel adjustment Filter | `krita_filter_crosschannel` |
| Dodge | `krita_filter_dodge` |
| Emboss (Laplacian) | `krita_filter_emboss laplascian` |
| Emboss Horizontal & Vertical | `krita_filter_emboss horizontal and vertical` |
| Emboss Horizontal Only | `krita_filter_emboss horizontal only` |
| Emboss Vertical Only | `krita_filter_emboss vertical only` |
| Emboss in All Directions | `krita_filter_emboss all directions` |
| Emboss with Variable Depth... | `krita_filter_emboss` |
| Gaussian Blur... | `krita_filter_gaussian blur` |
| Gaussian High Pass Filter | `krita_filter_gaussianhighpass` |
| Gaussian Noise Reduction... | `krita_filter_gaussiannoisereducer` |
| Halftone Filter | `krita_filter_halftone` |
| Height to Normal Map Filter | `krita_filter_height to normal` |
| Index Colors... | `krita_filter_indexcolors` |
| Left Edge Detection | `krita_filter_left edge detections` |
| Lens Blur... | `krita_filter_lens blur` |
| Maximize Channel | `krita_filter_maximize` |
| Mean Removal | `krita_filter_mean removal` |
| Minimize Channel | `krita_filter_minimize` |
| Motion Blur... | `krita_filter_motion blur` |
| Normalize Filter | `krita_filter_normalize` |
| Oilpaint... | `krita_filter_oilpaint` |
| Palettize Filter | `krita_filter_palettize` |
| Phong Bumpmap... | `krita_filter_phongbumpmap` |
| Pixelize... | `krita_filter_pixelize` |
| Posterize... | `krita_filter_posterize` |
| Propagate Colors Filter | `krita_filter_propagatecolors` |
| Raindrops... | `krita_filter_raindrops` |
| Random Noise... | `krita_filter_noise` |
| Random Pick... | `krita_filter_randompick` |
| Right Edge Detection | `krita_filter_right edge detections` |
| Round Corners... | `krita_filter_roundcorners` |
| Sharpen | `krita_filter_sharpen` |
| Slope, Offset, Power Filter | `krita_filter_asc-cdl` |
| Small Tiles... | `krita_filter_smalltiles` |
| Sobel... | `krita_filter_sobel` |
| Threshold Filter | `krita_filter_threshold` |
| Top Edge Detection | `krita_filter_top edge detections` |
| Unsharp Mask... | `krita_filter_unsharp` |
| Wave... | `krita_filter_wave` |
| Wavelet Noise Reducer... | `krita_filter_waveletnoisereducer` |
| gradientmap Filter | `krita_filter_gradientmap` |

</details>


## Krita — General

**11 default shortcuts** (31 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+Return` | Search Actions | `command_bar_open` |
| `Ctrl+Shift+I` | Invert Selection | `invert_selection` |
| `Ctrl+Shift+T` | Toggle Tablet Debugger | `tablet_debugger` |
| `F5` | Show Brush Editor | `show_brush_editor` |
| `F6` | Show Brush Presets | `show_brush_presets` |
| `H` | Show color history | `show_color_history` |
| `Shift+I` | Show color selector | `show_color_selector` |
| `Shift+M` | Show MyPaint shade selector | `show_mypaint_shade_selector` |
| `Shift+N` | Show minimal shade selector | `show_minimal_shade_selector` |
| `U` | Show common colors | `show_common_colors` |
| `\` | Show Tool Options | `show_tool_options` |

<details><summary>20 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Cascade | `windows_cascade` |
| Create Resource Bundle... | `create_bundle` |
| Create Snapshot | `create_snapshot` |
| Lock Toolbars | `lock_toolbars` |
| Open Resources Folder | `open_resources_directory` |
| Remove Selected Snapshot | `remove_snapshot` |
| Rename Composition... | `rename_composition` |
| Sample Screen Color | `sample_screen_color` |
| Sample Screen Color (Sample Real Canvas) | `sample_screen_color_real_canvas` |
| Show Android log for bug reports. | `logcatdump` |
| Show Docker Box | `docker_box` |
| Show File Toolbar | `mainToolBar` |
| Show Krita log for bug reports. | `buginfo` |
| Show color management information | `color_management_report` |
| Show crash log for bug reports. | `crashlog` |
| Show system information for bug reports. | `sysinfo` |
| Switch to Selected Snapshot | `switchto_snapshot` |
| Tile | `windows_tile` |
| Update Composition | `update_composition` |
| Use multiple of 2 for pixel scale | `ruler_pixel_multiple2` |

</details>


## Krita — Animation

<details><summary>52 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Add scalar keyframes | `add_scalar_keyframes` |
| Auto Frame Mode | `auto_key` |
| Clear Cache | `clear_animation_cache` |
| Clone Keyframes | `copy_frames_as_clones` |
| Copy Columns | `copy_columns_to_clipboard` |
| Copy Keyframes | `copy_frames` |
| Create Blank Frame | `add_blank_frame` |
| Create Duplicate Frame | `add_duplicate_frame` |
| Cut Columns | `cut_columns_to_clipboard` |
| Cut Keyframes | `cut_frames` |
| Drop Frames | `drop_frames` |
| First Frame | `first_frame` |
| Insert Column Left | `insert_column_left` |
| Insert Column Right | `insert_column_right` |
| Insert Hold Column | `insert_hold_column` |
| Insert Hold Frame | `insert_hold_frame` |
| Insert Keyframe Left | `insert_keyframe_left` |
| Insert Keyframe Right | `insert_keyframe_right` |
| Insert Multiple Columns | `insert_multiple_columns` |
| Insert Multiple Hold Columns | `insert_multiple_hold_columns` |
| Insert Multiple Hold Frames | `insert_multiple_hold_frames` |
| Insert Multiple Keyframes | `insert_multiple_keyframes` |
| Last Frame | `last_frame` |
| Make Unique | `make_clones_unique` |
| Mirror Columns | `mirror_columns` |
| Mirror Frames | `mirror_frames` |
| Next Frame | `next_frame` |
| Next Keyframe | `next_keyframe` |
| Next Matching Keyframe | `next_matching_keyframe` |
| Next Unfiltered Keyframe | `next_unfiltered_keyframe` |
| Paste Columns | `paste_columns_from_clipboard` |
| Paste Keyframes | `paste_frames` |
| Pin to Timeline | `pin_to_timeline` |
| Play / pause animation | `toggle_playback` |
| Previous Frame | `previous_frame` |
| Previous Keyframe | `previous_keyframe` |
| Previous Matching Keyframe | `previous_matching_keyframe` |
| Previous Unfiltered Keyframe | `previous_unfiltered_keyframe` |
| Remove Column | `remove_columns` |
| Remove Column and Pull | `remove_columns_and_pull` |
| Remove Frame and Pull | `remove_frames_and_pull` |
| Remove Hold Column | `remove_hold_column` |
| Remove Hold Frame | `remove_hold_frame` |
| Remove Keyframe | `remove_frames` |
| Remove Multiple Hold Columns | `remove_multiple_hold_columns` |
| Remove Multiple Hold Frames | `remove_multiple_hold_frames` |
| Remove scalar keyframe | `remove_scalar_keyframe` |
| Set End Time | `set_end_time` |
| Set Start Time | `set_start_time` |
| Stop animation | `stop_playback` |
| Toggle onion skin | `toggle_onion_skin` |
| Update Playback Range | `update_playback_range` |

</details>


## Krita — Color Selectors

**1 default shortcut** (10 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Shift+O` | Show wide-gamut color selector | `show_wg_color_selector` |

<details><summary>9 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Decrease color saturation | `wgcs_decrease_saturation` |
| Increase color saturation | `wgcs_increase_saturation` |
| Make color darker | `wgcs_darken_color` |
| Make color lighter | `wgcs_lighten_color` |
| Shift hue clockwise | `wgcs_shift_hue_clockwise` |
| Shift hue counter-clockwise | `wgcs_shift_hue_counterclockwise` |
| Show MyPaint shade selector | `show_wg_mypaint_selector` |
| Show color history | `show_wg_color_history` |
| Show wide-gamut shade selector | `show_wg_shade_selector` |

</details>


## Krita — S-Pen Actions

**8 default shortcuts** (9 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `#` | S-Pen Button Double Click | `spen_double_click` |
| `@` | S-Pen Button Click | `spen_click` |
| `F10` | S-Pen Gesture Swipe Right | `spen_swipe_right` |
| `F11` | S-Pen Gesture Circle Clockwise | `spen_circle_cw` |
| `F12` | S-Pen Gesture Circle Counter-Clockwise | `spen_circle_ccw` |
| `F7` | S-Pen Gesture Swipe Up | `spen_swipe_up` |
| `F8` | S-Pen Gesture Swipe Down | `spen_swipe_down` |
| `F9` | S-Pen Gesture Swipe Left | `spen_swipe_left` |

<details><summary>1 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Toggle popup palette with S-Pen | `spen_show_popup_palette` |

</details>


## Krita — Settings

<details><summary>1 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Explore Resources Cache Database... | `dbexplorer` |

</details>


## Tools — Tool Shortcuts (tool switchers)

**12 default shortcuts** (46 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `B` | Freehand Brush Tool | `KritaShape/KisToolBrush` |
| `C` | Crop Tool | `KisToolCrop` |
| `Ctrl+R` | Rectangular Selection Tool | `KisToolSelectRectangular` |
| `Ctrl+T` | Transform Tool | `KisToolTransform` |
| `F` | Fill Tool | `KritaFill/KisToolFill` |
| `G` | Gradient Tool | `KritaFill/KisToolGradient` |
| `J` | Elliptical Selection Tool | `KisToolSelectElliptical` |
| `P` | Color Sampler | `KritaSelected/KisToolColorSampler` |
| `Q` | Multibrush Tool | `KritaShape/KisToolMultiBrush` |
| `Shift+J` | Ellipse Tool | `KritaShape/KisToolEllipse` |
| `Shift+R` | Rectangle Tool | `KritaShape/KisToolRectangle` |
| `T` | Move Tool | `KritaTransform/KisToolMove` |

<details><summary>34 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| KisToolSelectPolygonal | `KisToolSelectPolygonal` |
| Assistant Tool | `KisAssistantTool` |
| Bezier Curve Selection Tool | `KisToolSelectPath` |
| Bezier Curve Tool | `KisToolPath` |
| Calligraphy | `KarbonCalligraphyTool` |
| Calligraphy: decrease angle | `calligraphy_decrease_angle` |
| Calligraphy: decrease width | `calligraphy_decrease_width` |
| Calligraphy: increase angle | `calligraphy_increase_angle` |
| Calligraphy: increase width | `calligraphy_increase_width` |
| Colorize Mask Tool | `KritaShape/KisToolLazyBrush` |
| Comic Panel Editing Tool | `KritaShape/KisToolKnife` |
| Contiguous Selection Tool | `KisToolSelectContiguous` |
| Dynamic Brush Tool | `KritaShape/KisToolDyna` |
| Edit Shapes Tool | `PathTool` |
| Enclose and Fill Tool | `KisToolEncloseAndFill` |
| Freehand Path Tool | `KisToolPencil` |
| Freehand Selection Tool | `KisToolSelectOutline` |
| Line Tool | `KritaShape/KisToolLine` |
| Magnetic Selection Tool | `KisToolSelectMagnetic` |
| Measurement Tool | `KritaShape/KisToolMeasure` |
| Pan Tool | `PanTool` |
| Polygon Tool | `KisToolPolygon` |
| Polyline Tool | `KisToolPolyline` |
| Reference Images Tool | `ToolReferenceImages` |
| Select Shapes Tool | `InteractionTool` |
| Similar Color Selection Tool | `KisToolSelectSimilar` |
| Smart Patch Tool | `KritaShape/KisToolSmartPatch` |
| Transform Tool (Cage Transform) | `KisToolTransformCage` |
| Transform Tool (Free Transform) | `KisToolTransformFree` |
| Transform Tool (Liquify Transform) | `KisToolTransformLiquify` |
| Transform Tool (Mesh Transform) | `KisToolTransformMesh` |
| Transform Tool (Perspective Transform) | `KisToolTransformPerspective` |
| Transform Tool (Warp Transform) | `KisToolTransformWarp` |
| Zoom Tool | `ZoomTool` |

</details>


## Tools — Interaction Tool (default vector tool)

**4 default shortcuts** (42 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Ctrl+Alt+[` | Lower | `object_order_lower` |
| `Ctrl+Alt+]` | Raise | `object_order_raise` |
| `Ctrl+Shift+[` | Send to Back | `object_order_back` |
| `Ctrl+Shift+]` | Bring to Front | `object_order_front` |

<details><summary>38 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Align Bottom | `object_align_vertical_bottom` |
| Align Left | `object_align_horizontal_left` |
| Align Right | `object_align_horizontal_right` |
| Align Top | `object_align_vertical_top` |
| Convert Text To Inline Wrapped | `text_type_inline_wrap` |
| Convert Text To Pre-positioned | `text_type_pre_positioned` |
| Convert Text To Preformatted | `text_type_preformatted` |
| Distribute Bottom | `object_distribute_vertical_bottom` |
| Distribute Centers Horizontally | `object_distribute_horizontal_center` |
| Distribute Centers Vertically | `object_distribute_vertical_center` |
| Distribute Horizontal Gap | `object_distribute_horizontal_gaps` |
| Distribute Left | `object_distribute_horizontal_left` |
| Distribute Right | `object_distribute_horizontal_right` |
| Distribute Top | `object_distribute_vertical_top` |
| Distribute Vertical Gap | `object_distribute_vertical_gaps` |
| Flow Text in Shape | `add_shape_to_flow_area` |
| Group | `object_group` |
| Horizontally Center | `object_align_horizontal_center` |
| Intersect | `object_intersect` |
| Mirror Horizontally | `object_transform_mirror_horizontally` |
| Mirror Vertically | `object_transform_mirror_vertically` |
| Move Shape Earlier In Chain. | `flow_shape_order_earlier` |
| Move Shape Later In Chain. | `flow_shape_order_later` |
| Put Text On Path | `put_text_on_path` |
| Remove Shapes from Text Chain | `remove_shapes_from_text_flow` |
| Reset Transformations | `object_transform_reset` |
| Rotate 180° | `object_transform_rotate_180` |
| Rotate 90° CCW | `object_transform_rotate_90_ccw` |
| Rotate 90° CW | `object_transform_rotate_90_cw` |
| Set Flow Shape as Last | `flow_shape_order_back` |
| Set Shape As First In Chain | `flow_shape_order_front` |
| Split | `object_split` |
| Subtract | `object_subtract` |
| Subtract Shape from Text Chain | `subtract_shape_from_flow_area` |
| Toggle Flow Shape Type | `flow_shape_type_toggle` |
| Ungroup | `object_ungroup` |
| Unite | `object_unite` |
| Vertically Center | `object_align_vertical_center` |

</details>


## Tools — Path Tool

**16 default shortcuts** (24 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Backspace` | Remove point | `pathpoint-remove` |
| `Ctrl+Alt+Shift+C` | Show Coordinates | `movetool-show-coordinates` |
| `Ctrl+B` | Break at selection | `path-break-selection` |
| `Down` | Move down | `movetool-move-down` |
| `F` | Segment to Line | `pathsegment-line` |
| `Ins` | Insert point | `pathpoint-insert` |
| `J` | Join with segment | `pathpoint-join` |
| `Left` | Move left | `movetool-move-left` |
| `P` | To Path | `convert-to-path` |
| `Right` | movetool-move-right | `movetool-move-right` |
| `Shift+C` | Segment to Curve | `pathsegment-curve` |
| `Shift+Down` | Move down more | `movetool-move-down-more` |
| `Shift+Left` | Move left more | `movetool-move-left-more` |
| `Shift+Right` | Move right more | `movetool-move-right-more` |
| `Shift+Up` | Move up more | `movetool-move-up-more` |
| `Up` | Move up | `movetool-move-up` |

<details><summary>8 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Break at point | `path-break-point` |
| Break at segment | `path-break-segment` |
| Corner point | `pathpoint-corner` |
| Make curve point | `pathpoint-curve` |
| Make line point | `pathpoint-line` |
| Merge points | `pathpoint-merge` |
| Smooth point | `pathpoint-smooth` |
| Symmetric Point | `pathpoint-symmetric` |

</details>


## SVG Tools — SVG Text Tool

**13 default shortcuts** (28 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `Alt+Shift+G` | Glyph Palette... | `svg_insert_special_character` |
| `Ctrl+<` | Decrease Font Size | `svg_decrease_font_size` |
| `Ctrl+>` | Increase Font Size | `svg_increase_font_size` |
| `Ctrl+Alt+C` | Align Center | `svg_align_center` |
| `Ctrl+Alt+R` | Align Block | `svg_align_justified` |
| `Ctrl+Alt+R` | Align Right | `svg_align_right` |
| `Ctrl+B` | Bold | `svg_weight_bold` |
| `Ctrl+I` | Italic | `svg_format_italic` |
| `Ctrl+L` | Light | `svg_weight_light` |
| `Ctrl+Shift+B` | Subscript | `svg_format_subscript` |
| `Ctrl+Shift+P` | Superscript | `svg_format_superscript` |
| `Ctrl+U` | Underline | `svg_format_underline` |
| `edit-clear` | Clear Formatting | `svg_clear_formatting` |

<details><summary>15 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Align Left | `svg_align_left` |
| Black | `svg_weight_black` |
| Demi | `svg_weight_demi` |
| Kerning | `svg_font_kerning` |
| Move Text Selection Down By 1 Pixel | `svg_type_setting_move_selection_start_down_1_px` |
| Move Text Selection Left By 1 Pixel | `svg_type_setting_move_selection_start_left_1_px` |
| Move Text Selection Right By 1 Pixel | `svg_type_setting_move_selection_start_right_1_px` |
| Move Text Selection Up By 1 Pixel | `svg_type_setting_move_selection_start_up_1_px` |
| Normal | `svg_weight_normal` |
| Paste Plain Text | `svg_paste_plain_text` |
| Paste Rich Text | `svg_paste_rich_text` |
| Remove Character Transforms | `svg_remove_transforms_from_range` |
| Settings... | `svg_settings` |
| Strikethrough | `svg_format_strike_through` |
| Text Tool | `SvgTextTool` |

</details>


## Scripts

**21 default shortcuts** (24 actions total)

| Shortcut | Action | Action ID |
|----------|--------|-----------|
| `ctrl+alt+0` | Activate Brush Preset 10 | `activate_preset_0` |
| `ctrl+alt+1` | Activate Brush Preset 1 | `activate_preset_1` |
| `ctrl+alt+2` | Activate Brush Preset 2 | `activate_preset_2` |
| `ctrl+alt+3` | Activate Brush Preset 3 | `activate_preset_3` |
| `ctrl+alt+4` | Activate Brush Preset 4 | `activate_preset_4` |
| `ctrl+alt+5` | Activate Brush Preset 5 | `activate_preset_5` |
| `ctrl+alt+6` | Activate Brush Preset 6 | `activate_preset_6` |
| `ctrl+alt+7` | Activate Brush Preset 7 | `activate_preset_7` |
| `ctrl+alt+8` | Activate Brush Preset 8 | `activate_preset_8` |
| `ctrl+alt+9` | Activate Brush Preset 9 | `activate_preset_9` |
| `ctrl+shift+0` | Execute Script 10 | `execute_script_10` |
| `ctrl+shift+1` | Execute Script 1 | `execute_script_1` |
| `ctrl+shift+2` | Execute Script 2 | `execute_script_2` |
| `ctrl+shift+3` | Execute Script 3 | `execute_script_3` |
| `ctrl+shift+4` | Execute Script 4 | `execute_script_4` |
| `ctrl+shift+5` | Execute Script 5 | `execute_script_5` |
| `ctrl+shift+6` | Execute Script 6 | `execute_script_6` |
| `ctrl+shift+7` | Execute Script 7 | `execute_script_7` |
| `ctrl+shift+8` | Execute Script 8 | `execute_script_8` |
| `ctrl+shift+9` | Execute Script 9 | `execute_script_9` |
| `z` | Mutate | `mutate` |

<details><summary>3 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Import Python Plugin from File... | `plugin_importer_file` |
| Import Python Plugin from Web... | `plugin_importer_web` |
| Ten Brushes | `ten_brushes` |

</details>


## Recorder — Recorder

<details><summary>2 unbound actions in this group (default has no shortcut)</summary>

| Action | Action ID |
|--------|-----------|
| Export Timelapse... | `recorder_export` |
| Record Timelapse | `recorder_record_toggle` |

</details>


---

## Coverage

- **685 total actions** exposed by Krita (menu items, tool activators, layer ops, scripts, etc.)
- **201 have a default keyboard shortcut**; the remaining **484** are reachable only via menus, toolbars, or by being assigned a shortcut in `Settings → Configure Krita → Keyboard Shortcuts`.
- Canvas input bindings (modifier + pointer / wheel / gesture) live in `kritadefault.profile` — counted separately.

## Source files

```
krita/krita/data/actions/InteractionTool.action
krita/krita/data/actions/MoveTool.action
krita/krita/data/actions/PathTool.action
krita/krita/krita.action
krita/krita/kritamenu.action
krita/plugins/assistants/Assistants/KisAssistantTool.action
krita/plugins/dockers/recorder/recorder.action
krita/plugins/dockers/widegamutcolorselector/WGColorSelector.action
krita/plugins/extensions/dbexplorer/dbexplorer.action
krita/plugins/extensions/spensettings/SPenSettings.action
krita/plugins/filters/asccdl/asccdl.action
krita/plugins/filters/colorsfilters/colorsfilters.action
krita/plugins/filters/convertheightnormalmap/convertheightnormalmap.action
krita/plugins/filters/gaussianhighpass/gaussianhighpass.action
krita/plugins/filters/gradientmap/gradientmap.action
krita/plugins/filters/halftone/halftone.action
krita/plugins/filters/normalize/normalize.action
krita/plugins/filters/palettize/palettize.action
krita/plugins/filters/propagatecolors/propagatecolors.action
krita/plugins/filters/threshold/threshold.action
krita/plugins/python/mutator/mutator.action
krita/plugins/python/plugin_importer/plugin_importer.action
krita/plugins/python/tenbrushes/tenbrushes.action
krita/plugins/python/tenscripts/tenscripts.action
krita/plugins/tools/basictools/KisToolPath.action
krita/plugins/tools/basictools/KisToolPencil.action
krita/plugins/tools/karbonplugins/tools/CalligraphyTool/KarbonCalligraphyTool.action
krita/plugins/tools/selectiontools/KisToolSelectContiguous.action
krita/plugins/tools/selectiontools/KisToolSelectElliptical.action
krita/plugins/tools/selectiontools/KisToolSelectMagnetic.action
krita/plugins/tools/selectiontools/KisToolSelectOutline.action
krita/plugins/tools/selectiontools/KisToolSelectPath.action
krita/plugins/tools/selectiontools/KisToolSelectPolygonal.action
krita/plugins/tools/selectiontools/KisToolSelectRectangular.action
krita/plugins/tools/selectiontools/KisToolSelectSimilar.action
krita/plugins/tools/svgtexttool/SvgTextTool.action
krita/plugins/tools/tool_crop/KisToolCrop.action
krita/plugins/tools/tool_enclose_and_fill/KisToolEncloseAndFill.action
krita/plugins/tools/tool_polygon/KisToolPolygon.action
krita/plugins/tools/tool_polyline/KisToolPolyline.action
krita/plugins/tools/tool_transform2/KisToolTransform.action
krita/plugins/tools/tools.action
krita/krita/data/input/kritadefault.profile
```