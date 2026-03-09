# ABR Format — Reverse Engineering Analysis

## Overview

ABR (Adobe Brush) is a closed, proprietary binary format used by Adobe Photoshop to store brush presets. Adobe has **never published a specification** for versions beyond 1. The official "Adobe Photoshop 6.0 File Formats Specification" (November 2000, included in this repo at `Brush-Converter/ps6ffspecsv2.pdf`, page 50) documents only version 1 — the original format with computed and sampled brushes. Everything about modern ABR (versions 2, 6, 10) has been reverse-engineered by the community.

Despite being proprietary, ABR is the de facto standard for brush distribution. Thousands of brush packs exist in ABR format, and every major art application (Krita, GIMP, Procreate, CSP, Affinity, Substance Painter, 3DCoat) attempts to import them — with varying degrees of failure.

## File Structure

### Header (all versions)

```
Offset  Size  Field
0       2     version     (big-endian int16)
2       2     count       (v1/v2: brush count; v6/v10: subversion)
```

### Version History

| Version | Photoshop Era | count Field | Structure |
|---------|-------------|-------------|-----------|
| 1 | PS 5.x and earlier | Brush count | Flat list of brush records |
| 2 | PS 6.0-7.0 | Brush count | Flat list + UCS-2 brush names |
| 6 sub1 | PS CS (8.0) | Subversion=1 | 8BIM sections: `samp`, `patt`, `desc` |
| 6 sub2 | PS CS2-CS5 | Subversion=2 | 8BIM sections, larger header skip (301 vs 47 bytes) |
| 10 sub1 | PS CS6 | Subversion=1 | Same 8BIM structure as v6 |
| 10 sub2 | PS CC (14.0+) | Subversion=2 | Same 8BIM structure, larger header |

Versions 3, 4, 5, 7, 8, 9 do not exist. The jump from 2→6 and 6→10 corresponds to major Photoshop architecture changes.

### Version 1/2: Flat Brush List

```
[header: version=1/2, count=N]
[brush 0: type(2) + size(4) + data(size)]
[brush 1: type(2) + size(4) + data(size)]
...
[brush N-1]
```

Each brush is either:
- **Type 1: Computed brush** — procedural parameters (diameter, hardness, roundness, angle, spacing). Fixed 14 bytes of data.
- **Type 2: Sampled brush** — raster image data

#### Computed Brush (Type 1) — From Official Spec

```
Offset  Size  Field
0       4     miscellaneous   (ignored)
4       2     spacing         (0-999, 0=no spacing)
6       2     diameter        (1-999 pixels)
8       2     roundness       (0-100%)
10      2     angle           (-180 to 180 degrees)
12      2     hardness        (0-100%)
```

This is enough to reconstruct a procedural brush. **No implementation (GIMP, Krita) currently generates brushes from this data** — they all skip type 1 brushes with a "FIXME: support it!" comment.

#### Sampled Brush (Type 2)

```
Offset  Size  Field
0       4     miscellaneous   (ignored)
4       2     spacing         (0-999)
[v2 only: UCS-2 name — length-prefixed 16-bit characters]
+0      1     anti-aliasing   (0=off, 1=on)
+1      8     bounds          (4x int16: top, left, bottom, right)
+9      16    bounds_long     (4x int32: top, left, bottom, right)
+25     2     depth           (bits per pixel, always 8)
+27     var   image data      (compression byte + grayscale pixels)
```

Image data structure:
```
0       1     compression     (0=raw, 1=RLE/PackBits)
1       var   pixel data      (grayscale, scanline order)
```

If height > 16384 pixels, data is chunked into 16384-line blocks.

RLE compression uses standard PackBits (same as Macintosh ROM routine / TIFF PackBits):
- Per-scanline byte counts (int16 per row) followed by compressed data
- Negative run-length = repetition, positive = literal copy

### Version 6/10: 8BIM Section Architecture

Modern ABR files use Photoshop's "8BIM" tagged section system:

```
[header: version=6/10, subversion=1/2]
[8BIM section: "samp" — sampled brush tip images]
[8BIM section: "patt" — pattern/texture data]
[8BIM section: "desc" — ActionDescriptor brush parameters]
[optional: "phry" — additional metadata]
```

Each 8BIM section:
```
0       4     signature       "8BIM"
4       4     key             "samp", "patt", "desc", etc.
8       4     length          section data size
12      var   data
```

#### The `samp` Section — Brush Tip Images

Contains one or more sampled brush images. Each brush record:
```
0       4     item_length
[sub1: 47 bytes fixed header — contains UUID, short coords, unknown]
[sub2: 301 bytes fixed header — contains UUID, extra metadata]
+0      4     top
+4      4     left
+8      4     bottom
+12     4     right
+16     2     depth
+18     1     compression
+19     var   image data (same PackBits RLE as v1/v2)
```

The 47/301 byte fixed header contains a UUID that links this brush tip to its descriptor in the `desc` section. The UUID is a standard `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` ASCII string embedded in the header bytes.

#### The `patt` Section — Patterns/Textures

Contains embedded patterns used for texture brushes. Each pattern:
```
0       4     item_length
4       4     version
8       4     color_mode      (1=Grayscale, 2=Indexed, 3=RGB, 4=CMYK/RGBA)
12      2     height
14      2     width
16      4     name_length     (UCS-2 characters)
20      var   name            (UTF-16BE)
+0      1     id_length
+1      var   id              (ASCII UUID)
[if indexed: 768 bytes color table]
[inner header: version, length, rect, max_depth, unknown, compressed_size, bit_depth, actual_rect, depth2, compression]
[pixel data: multi-channel, RLE or raw]
```

Pattern pixel data can be multi-channel (1, 3, or 4 channels) with per-channel RLE compression. Channels may be interleaved or serial, with intermediate headers between channels.

After the pixel data, patterns may have trailing metadata including vector path blocks (for vector shape masks).

#### The `desc` Section — ActionDescriptors (Brush Parameters)

This is the most complex and most valuable section. It contains serialized Photoshop ActionDescriptors — the same binary format Photoshop uses internally for actions, scripts, and preset serialization.

```
0       4     descriptor_version
4       var   descriptor tree (recursive key-value structure)
```

**ActionDescriptor structure:**
```
ClassID:
  4     name_length     (0 = implicit 4-byte code follows)
  var   class_id        (e.g. "null", "brushPreset", "computedBrush")
  pad   alignment to 4 bytes (sometimes — heuristic-dependent)
  4     num_items       (sometimes 2 bytes — heuristic-dependent)

Per item:
  4/1   key_length      (0 = implicit 4-byte OSType code; or 1-byte compact length)
  var   key             (e.g. "Brsh", "Nm  ", "Dmtr", "flipX", "useTipDynamics")
  4     type_code       (OSType: "UntF", "bool", "long", "doub", "enum", "TEXT", "Objc", "VlLs")
  var   value           (type-dependent)
```

**Value types:**
| Type | Format | Description |
|------|--------|-------------|
| `TEXT` | length(4) + UTF-16BE chars | Unicode text (brush name, etc.) |
| `UntF` | unit(4) + double(8) | Unit float (e.g. `#Pxl` = pixels, `#Prc` = percent, `#Ang` = angle) |
| `doub` | double(8) | Raw double |
| `long` | int32(4) | Integer |
| `bool` | byte(1) + padding(0-3) | Boolean with variable padding |
| `enum` | enum_type + enum_value | Enumeration (e.g. mode, blend) |
| `Objc` | flag(4) + name? + descriptor | Nested descriptor object |
| `VlLs` | count(4) + items | List/array of values |

**Known descriptor keys (reverse-engineered by community):**

*Brush tip:*
- `Nm  ` — brush name (TEXT)
- `Dmtr` — diameter (UntF, pixels)
- `Hrdn` — hardness (UntF, percent)
- `Angl` — angle (UntF, degrees)
- `Rndn` — roundness (UntF, percent)
- `Spcn` — spacing (UntF, percent)
- `Intr` — interpreted (bool)
- `flipX` / `flipY` — flip flags (bool)

*Dynamics:*
- `useTipDynamics` — enable tip dynamics (bool)
- `szVr` — size variation / size jitter (Objc → descriptor with curve data)
- `minimumDiameter` — minimum diameter percent (UntF)
- `angleDynamics` — angle jitter (Objc)
- `roundnessDynamics` — roundness jitter (Objc)
- `minimumRoundness` — minimum roundness (UntF)
- `tiltScale` — tilt scale factor (UntF)
- `fStp` — flow step? (unknown exact meaning)
- `jitter` — generic jitter parameter

*Scatter:*
- `useScatter` — enable scatter (bool)

*Texture:*
- `useTexture` — enable texture (bool)
- Pattern UUID references link to `patt` section entries

*Dual Brush:*
- `dualBrush` — dual brush settings (Objc → nested descriptor)

*Color Dynamics:*
- `useColorDynamics` — enable color dynamics (bool)

*Paint Dynamics:*
- `usePaintDynamics` — enable paint dynamics (bool)

*Other:*
- `Wtdg` — wet edges (bool)
- `Nose` — noise (bool)
- `Rpt ` — protect texture (bool)
- `computedBrush` — marks a computed (procedural) brush
- `brushGroup` / `useBrushGroup` — brush grouping
- `bVTy` — brush variation type (enum)

*Curve/dynamics descriptors contain:*
- `inpt` — input source (enum: pressure, tilt, stylus wheel, etc.)
- `grad` — gradient curve data (Objc with color stops)
- `Cl  ` — color (Objc)
- `Ofst` — offset
- `Type` — type
- `Loc ` — location
- `Mdpn` — midpoint

### The Format's Achilles Heel

The ActionDescriptor format is deeply inconsistent:
- Key lengths are sometimes 4-byte standard, sometimes 1-byte compact — **no reliable way to tell** except heuristics and lookup tables
- Object name lengths are sometimes 4-byte, sometimes 2-byte
- ClassID padding is sometimes 4-byte aligned, sometimes compact
- Boolean values have 0-3 bytes of padding with no flag to indicate how much
- Number of items in a descriptor is sometimes 4-byte, sometimes 2-byte

The `abr_solver.py` in `Brush-Converter/` handles this through a combination of **known-key lookup tables** and **heuristic probing** — checking if upcoming bytes look like valid keys/types and choosing the interpretation that makes structural sense. This is fragile and version-dependent, explaining why ABR import breaks so often across applications.

## State of Community Reverse Engineering

### What Has Been Reversed

| Component | Coverage | Notes |
|-----------|----------|-------|
| **v1/v2 brush tips** | 100% | Fully documented in official spec |
| **v1 computed brush params** | 100% | In official spec but rarely implemented |
| **v6/v10 brush tip images** | ~95% | Well understood, minor edge cases with multi-channel depth |
| **8BIM section structure** | 100% | samp, patt, desc, phry — all identified |
| **Pattern extraction** | ~80% | Multi-channel decode works, vector masks partially understood |
| **ActionDescriptor structure** | ~70% | Core types parsed, but padding/alignment is heuristic |
| **Brush parameter keys** | ~60% | Major keys identified, many undocumented ones remain |
| **Dynamics/curve data** | ~40% | Structure known, exact semantics of many values unclear |
| **Dual brush** | ~30% | Nested descriptor detected but not fully mapped |
| **Color dynamics** | ~20% | Existence known, parameter mapping incomplete |

### What Remains Unknown

- Complete list of all descriptor keys and their exact semantics
- Precise rules for compact vs standard key length encoding (currently heuristic)
- How to **write** valid ABR files (the Brush-Converter README notes: "we still don't know the rules of new version abr totally")
- ABR versions beyond 10 sub2 (if any exist in modern Photoshop)
- Exact mapping between descriptor parameter values and Photoshop brush behavior

## Existing Implementations Compared

### GIMP (`brush/gimp/app/core/gimpbrush-load.c`)
- **Versions:** 1, 2, 6 (sub 1/2), 10 (sub 1/2)
- **What it reads:** `samp` section only — brush tip images
- **What it ignores:** `desc` (all dynamics/settings), `patt` (all textures), computed brushes (type 1)
- **Telling comment, line 911:** `spacing = 25; /* real value needs 8BIMdesc section parser */`
- **Language:** C (GLib/GIO)

### Krita (`krita/libs/brush/kis_abr_brush_collection.cpp`)
- **Versions:** 1, 2, 6 (sub 1/2) — **v10 not supported**
- **What it reads:** `samp` section only — brush tip images, converted to grayscale QImage
- **What it ignores:** Everything else. Comment: `XXX: call extra setters`
- **Limitations:** Read-only, not serializable, no individual brush loading
- **Language:** C++ (Qt)

### PSBrushExtract (`brush/PSBrushExtract/psbrushextract.py`)
- **Approach:** Scans entire file for ActionDescriptor type markers (`UntF`, `bool`, `long`, `doub`, `enum`, `TEXT`, `Objc`, `VlLs`)
- **What it reads:** All parameter values from `desc` section — key names, types, and values
- **What it doesn't do:** No structured descriptor parsing (no nesting), no image extraction (separate script), no pattern extraction
- **Output:** Flat list of key-value pairs
- **Language:** Python, AGPLv3

### Brush-Converter (`brush/Brush-Converter/abr_solver.py`)
- **The most comprehensive parser.** 1281 lines of Python.
- **Versions:** 1, 2, 6 (sub 1/2), 10 (sub 1/2+, with heuristic fallback for unknown subversions)
- **What it reads:**
  - `samp` section: full brush tip images with UUID extraction
  - `patt` section: full pattern extraction including multi-channel RLE, indexed color tables, vector path blocks
  - `desc` section: full recursive ActionDescriptor parsing with heuristic key length detection
- **ActionDescriptor parsing:** Uses 3 lookup tables (KNOWN_KEY_MODES, KNOWN_OBJ_NAME_MODES, KNOWN_CLASSID_MODES) plus heuristic probing for unknown keys
- **Output:** PNG images for tips/patterns + JSON for all descriptor data
- **Also parses:** Procreate `.brushset` and Clip Studio Paint `.sut`
- **Language:** Python (numpy, PIL), CC BY-NC 4.0
- **Limitation:** Cannot write/repack ABR files

## Relevance to Darkly

### Should We Import ABR?

**Yes, but with clear expectations.** ABR is the lingua franca of brush distribution. Artists will expect it. But:

1. **Import brush tips — this is the baseline.** Every competitor does this. The `samp` section is well-understood and straightforward to implement (grayscale bitmaps with PackBits RLE).

2. **Import patterns/textures from `patt` section** — this goes beyond what GIMP/Krita do and would be a differentiator.

3. **Parse descriptors from `desc` section** — this is the hardest part but also the most valuable. Even partial extraction of spacing, diameter, hardness, angle, dynamics enables would make our import dramatically better than competitors.

4. **Don't try to write ABR.** The format is too fragile and heuristic-dependent for reliable round-tripping. Use our own format for presets.

### Should Our Brush System Be Inspired by ABR?

**No.** ABR is a serialization format, not a brush system design. It reflects Photoshop's internal architecture — which is itself showing its age (the "snowflake engine" problem that Krita inherits).

What's worth studying from ABR is the **parameter vocabulary** — it represents what the industry considers the minimum viable feature set for a professional brush:
- Tip shape (computed or sampled)
- Spacing
- Dynamics mapped to: pressure, tilt, stylus wheel, velocity, fade, etc.
- Transfer curves (not just linear mapping)
- Scatter (count, both axes)
- Texture/pattern overlay with blend modes
- Dual brush (two tips composited)
- Color dynamics (H/S/B jitter, foreground/background jitter)
- Wet edges, noise, smoothing

These are the features artists expect. How we implement them should be our own architecture — but this parameter list is the compatibility target.
