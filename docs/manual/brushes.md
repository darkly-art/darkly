---
title: Brush Basics
description: Picking, switching and tweaking brushes.
template: doc
---

Everything you need to paint. If you want to build your own brushes from scratch, skip to [the brush builder](#building-your-own-brushes).

Every action below has a hotkey, and which key depends on the hotkey preset you picked. [Actions & hotkeys](/docs/reference/actions/) lists them all for every preset; search it for "brush". Inside Darkly, the hotkey is in every tooltip and search result.

## Picking a brush

Click the brush button at the left of the brush bar to open the brush explorer. Brushes are grouped into packs; click one and the explorer gets out of your way.

## The palette wheel

**Palette Popup** opens a wheel under your cursor. Drag toward what you want and lift to pick it:

- **Recent**: your last few brushes.
- **Library**: every pack, and every brush in it.
- **Colors**: your recent colors, plus the full color wheel.

## Switching between recent brushes

**Cycle Recent Brushes** swaps to the brush you were just using. Wait a moment, press it again, and you're back. Two brushes, one key.

Press it quickly and it steps further back through your recent brushes, looping around when it reaches the end. Overshot? **Cycle Recent Brushes Backward** steps the other way, and you can mix the two in one run. Stop for a moment and the brush you landed on becomes your most recent.

By default the cycle covers your 5 most recent brushes, counting the one you're holding. Change it in Settings, under Tools, **Recent brush cycle length**.

## Tweaking a brush

The brush bar holds the settings that brush exposes: size, opacity, softness and so on. Drag one to change it, or double-click it to reset it to the brush's saved value. **Increase Brush Size** and **Decrease Brush Size** step the size from the keyboard.

Your tweaks stay with the brush. Switch to another brush and back, and they're still there. A tweaked brush shows a **`*`** after its name, with a revert button beside it that puts it back the way it was saved. Tweaks last until you close Darkly; to keep them for good, save the brush from the brush builder.

## Erasing

**Toggle Erase Mode** turns the brush you're holding into an eraser of the same shape, and back again. Blur, liquify and watercolor brushes have no erase mode.

## Locking colors to brushes

The chain icon between the color swatches and the brush button links colors to brushes. With it on, each brush remembers the foreground and background you last used it with, and switching brushes switches the colors too: a red pencil stays red, a black ink pen stays black. The same switch is in Settings, under Colors, **Lock colors to brush**.

## Color jitter

Open the color panel (or click a color swatch) and choose the **Jitter** tab. Each slider lets every stroke drift a little from your foreground color:

- **Hue**: how many degrees either side of your color a stroke may land.
- **Saturation**: how much more or less vivid.
- **Value**: how much lighter or darker.

Each stroke picks its own color once, when you start it, so a single stroke stays one color and the next one comes out slightly different. Jitter applies to every brush. Set all three back to zero to turn it off.

## Building your own brushes

Every brush in Darkly is a small graph of nodes: pen pressure goes in, gets shaped by curves and math, and ends up as a mark on the canvas. The chevron at the right end of the brush bar opens the brush builder, where you can open any brush up, rewire it, and save the result to your library.

The [brush nodes](/docs/reference/brushes/brush-nodes/) page lists every node and what it does, and [brushes](/docs/reference/brushes/brushes/) lists the brushes Darkly ships with.
