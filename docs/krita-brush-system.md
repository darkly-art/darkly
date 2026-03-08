# Krita Brush System — Deep Dive

## Overview

Krita's brush system is a layered architecture with four main subsystems:

1. **Brush Tips** — the stamp image or procedural mask applied at each dab
2. **Brush Engines (Paintops)** — the algorithms that decide *how* dabs are placed and what they do
3. **Dynamics / Sensors** — curves that map tablet input (pressure, tilt, speed…) to brush parameters
4. **Presets** — serialized bundles (`.kpp` files) that package an engine + its settings + embedded resources

The stroke pipeline connects these: input events flow through sensors, produce `KisPaintInformation`, the engine decides where to stamp, the brush tip generates a mask, and `KisPainter` composites the dab onto the canvas.

---

## 1. Brush Tips

### Class Hierarchy

```
KoResource
  └── KisBrush                       (base — mask generation API)
       ├── KisAutoBrush               (procedural: circle, rect, gaussian, curve)
       └── KisScalingSizeBrush        (adds user-effective-size via scale)
            ├── KisSvgBrush           (SVG vector tips)
            ├── KisAbrBrush           (Adobe .abr — read-only, grayscale only)
            └── KisColorfulBrush      (adds brightness/contrast adjustment)
                 ├── KisGbrBrush      (GIMP .gbr — grayscale or RGBA)
                 │    └── KisImagePipeBrush  (animated .gih — multi-frame)
                 └── KisPngBrush      (PNG images as brush tips)
```

`KisBrushSP` = `QSharedPointer<KisBrush>`.

### How a Dab Is Generated

The central method is `KisBrush::generateMaskAndApplyMaskOrCreateDab()`. Two paths:

**Image-based brushes** (GBR, PNG, ABR, pipe):
1. The brush tip image is scaled/rotated via `KisQImagePyramid` (mip-mapped cache) using the current `KisDabShape` (scale, ratio, rotation) and subpixel offsets
2. For single-color painting: the color space's optimized `fillGrayBrushWithColor()` tints the grayscale mask
3. For lightness-preserving mode: `fillGrayBrushWithColorAndLightnessWithStrength()` preserves the tip's luminance variation
4. For gradient mode: each mask pixel's grayscale value indexes into a cached gradient
5. For source-device coloring: per-pixel color is copied from a source, then the mask alpha is applied

**Procedural auto-brushes** (circle, square, gaussian, curve):
1. Mask dimensions are computed from `KisDabShape`
2. Center is calculated with subpixel offset
3. `KisMaskGenerator` is configured with softness, scale
4. A SIMD-optimized `KisBrushMaskApplicatorBase` stamps the mask generator's output directly — no intermediate bitmap

### Mask Generators

`KisMaskGenerator` is the base. Concrete implementations:

| Type | Circle | Rectangle |
|------|--------|-----------|
| **Default** (hard/soft) | `KisCircleMaskGenerator` | `KisRectangleMaskGenerator` |
| **Gaussian** | `KisGaussCircleMaskGenerator` | `KisGaussRectMaskGenerator` |
| **Curve** (user-defined falloff) | `KisCurveCircleMaskGenerator` | `KisCurveRectMaskGenerator` |

Parameters: radius, ratio (aspect), horizontal/vertical fade, spikes (star/polygon shapes when > 2), antialiasEdges, softness, custom curve string.

### Brush Tip Formats

**GBR (GIMP Brush):** Binary format with a header (version 1 = Latin-1 names, no spacing; version 2 = UTF-8, `GIMP` magic, spacing). 1 byte/pixel = grayscale mask (inverted), 4 bytes/pixel = RGBA image stamp.

**GIH (Image Pipe):** Multiple GBR frames in one file. A `KisPipeBrushParasite` defines up to 4 selection dimensions, each with a mode: Constant, Incremental, Angular, Velocity, Random, Pressure, TiltX, TiltY. The pipe selects a frame per-dab based on these modes.

**PNG:** Standard PNG loaded as QImage. Supports color with brightness/contrast adjustment.

**SVG:** Vector brush tips. Stored as raw SVG data, rasterized on use.

**ABR (Adobe Brush):** Read-only import. See the ABR section below.

---

## 2. Brush Engines (Paintops)

Krita has **16 brush engines**, each a `KisPaintOp` subclass. They live in `plugins/paintops/`.

### The KisPaintOp Base Class

Every engine must implement:
```cpp
virtual KisSpacingInformation paintAt(const KisPaintInformation& info) = 0;
virtual KisSpacingInformation updateSpacingImpl(const KisPaintInformation& info) const = 0;
```

Optional overrides:
- `paintLine()` — for engines that draw continuous lines rather than discrete dabs (curve, hairy, sketch, particle)
- `paintBezierCurve()` — for Bezier path segments
- `doAsynchronousUpdate()` — for engines with multithreaded dab rendering (pixel brush)
- `updateTimingImpl()` — for airbrush-style timed repeat

### Engine Catalog

#### Dab-Stamping Engines (inherit `KisBrushBasedPaintOp`)

These stamp a brush tip at each dab position:

| Engine | Class | What It Does |
|--------|-------|-------------|
| **Pixel Brush** | `KisBrushOp` | The main workhorse. Async dab rendering via `KisDabRenderingExecutor` for multithreaded painting. Supports all standard options. |
| **Color Smudge** | `KisColorSmudgeOp` | Blends paint color with existing canvas pixels. Multiple internal strategies (Lightness, Mask, Stamp, MaskLegacy). Smudge length, dulling vs smear mode, paint thickness. |
| **Clone / Duplicate** | `KisDuplicateOp` | Clones pixels from a source location. Supports seamless cloning via `minimizeEnergy()`. |
| **Filter Brush** | `KisFilterOp` | Applies a Krita filter (blur, sharpen, etc.) through the brush mask area. |
| **Hatching** | `KisHatchingPaintOp` | Generates hatching/crosshatching line patterns within the dab area. Angle, separation, thickness are all dynamic. |
| **Tangent Normal** | `KisTangentNormalPaintOp` | Converts pen tilt to RGB normal-map colors. For 3D texture painting. |

#### Line-Drawing Engines (override `paintLine()`)

These draw continuous paths rather than stamping:

| Engine | Class | What It Does |
|--------|-------|-------------|
| **Curve** | `KisCurvePaintOp` | Smooth curves between input points. |
| **Hairy** | `KisHairyPaintOp` | Natural media simulation with individual bristle physics. Each bristle tracks its own position and ink level. Ink depletion, soaking, Wu-particle anti-aliasing. |
| **Sketch** | `KisSketchPaintOp` | Pencil-like strokes by connecting nearby random points within the brush mask radius. |
| **Particle** | `KisParticlePaintOp` | Physics-based particle trajectory painting. |

#### Special Engines

| Engine | Class | What It Does |
|--------|-------|-------------|
| **Spray** | `KisSprayPaintOp` | Sprays multiple particles (ellipses, rectangles, images, Wu particles) within a radius. Configurable angular/radial distributions. |
| **Grid** | `KisGridPaintOp` | Paints in a grid of cells, each cell a configurable shape. |
| **Deform** | `KisDeformPaintOp` | Pixel deformation: grow, shrink, swirl CW/CCW, push, lens in/out, color displacement. |
| **Experiment** | `KisExperimentPaintOp` | Builds up a QPainterPath as you paint, then fills it. Non-incremental. Supports displacement and speed. |
| **Round Marker** | `KisRoundMarkerOp` | Perfectly smooth round markers with position/radius interpolation. |
| **MyPaint** | `KisMyPaintPaintOp` | Integration with the `libmypaint` brush engine. Uses MyPaint's own settings/dynamics system. |

---

## 3. Dynamics / Sensors

### Architecture

The dynamics system connects tablet input signals to brush parameters through transfer curves:

```
Tablet Input → KisPaintInformation → KisDynamicSensor → curve mapping → KisCurveOption → brush parameter
```

### The 16 Sensors

| Sensor ID | Name | Source | Mode |
|-----------|------|--------|------|
| `pressure` | Pressure | `info.pressure()` (0–1) | Multiplicative |
| `pressurein` | PressureIn | `info.maxPressure()` (max so far in stroke) | Multiplicative |
| `speed` | Speed | `info.drawingSpeed()` | Multiplicative |
| `distance` | Distance | `info.drawingDistance()` | Multiplicative (has length + periodic) |
| `fade` | Fade | Dab count | Multiplicative (has length + periodic) |
| `time` | Time | `info.currentTime()` | Multiplicative (has length + periodic) |
| `rotation` | Rotation | `info.rotation() / 180.0` (tablet barrel) | Additive |
| `drawingangle` | Drawing Angle | `info.drawingAngle()` | Absolute rotation (has locked angle, fan corners, angle offset) |
| `xtilt` | X-Tilt | `1.0 - abs(info.xTilt()) / 60.0` | Multiplicative |
| `ytilt` | Y-Tilt | `1.0 - abs(info.yTilt()) / 60.0` | Multiplicative |
| `ascension` | Tilt Direction | derived tilt direction | Additive |
| `declination` | Tilt Elevation | derived tilt elevation | Multiplicative |
| `perspective` | Perspective | `info.perspective()` (perspective grid) | Multiplicative |
| `tangentialpressure` | Tangential Pressure | `info.tangentialPressure()` (airbrush wheel) | Multiplicative |
| `fuzzy` | Fuzzy Dab | Random per dab | Additive |
| `fuzzystroke` | Fuzzy Stroke | Random per stroke | Additive |

### How Sensor Values Become Brush Parameters

`KisCurveOption` is the runtime bridge. For each brush parameter (size, opacity, rotation, etc.), there's a `KisCurveOption` instance holding the active sensors for that parameter.

Evaluation flow:
1. `computeValueComponents(info)` iterates active sensors
2. Each sensor calls its `value(info)` to get a raw 0–1 input
3. If a curve is configured (non-identity), the value is mapped through a 256-point `KisCubicCurve` transfer function
4. Results accumulate into `ValueComponents`:
   - `scaling` — product of all multiplicative sensor outputs
   - `additive` — sum of all additive sensor outputs (fuzzy, rotation, tilt direction)
   - `absoluteOffset` — for drawing angle (absolute rotation)
   - `constant` — the base strength/value
5. Final parameter value:
   - **Size-like:** `constant * scaling + constant * additive`, clamped to min/max
   - **Rotation-like:** complex formula combining base angle, axis flipping, scaling, and additive parts

### Sensor Data Serialization

Each sensor stores:
- `KoID id` — sensor identifier
- `QString curve` — serialized cubic curve control points
- `bool isActive`

Extended data for specific sensors:
- **Distance/Fade/Time:** `int length` (range), `bool isPeriodic` (cycles)
- **Drawing Angle:** `bool fanCornersEnabled`, `int fanCornersStep`, `qreal angleOffset`, `bool lockedAngleMode`

The `KisKritaSensorPack` bundles all 16 sensor data structs for serialization within a preset.

---

## 4. Stroke Rendering Pipeline

### From Pen Down to Pixels

```
1. Input Event
   ↓
2. KisPaintInformation constructed (pos, pressure, tilt, rotation, time, speed…)
   ↓
3. KisPainter::paintLine(pi1, pi2, currentDistance)
   ↓
4. KisPaintOp::paintLine() — default: iterate dab positions via spacing
   ↓
5. KisPaintOpUtils::paintLine() — interpolates between pi1 and pi2:
   │  a. currentDistance.getNextPointPosition() → find next dab position
   │  b. KisPaintInformation::mix() → interpolate tablet data at that position
   │  c. Optionally paintFan() for smooth rotation corners
   │  d. Call paintAt() at each dab position
   ↓
6. KisPaintOp::paintAt(info) — engine-specific: generates the dab
   │  a. Evaluate dynamics/sensors for this dab's parameters
   │  b. Generate mask via KisBrush::generateMaskAndApplyMaskOrCreateDab()
   │  c. Composite via KisPainter::bltFixed() or similar
   ↓
7. KisPainter composites onto canvas using composite op, opacity, flow, selection
```

For Bezier curves: `paintBezierCurve()` subdivides using midpoint subdivision (threshold 0.5) until flat, then calls `paintLine()`.

### Spacing

`KisSpacingInformation` defines how far apart dabs are placed:
- `m_distanceSpacing` — QPointF for anisotropic (x/y can differ)
- `m_rotation` — rotation of the spacing ellipse
- Auto-spacing formula: `coeff * (value < 1.0 ? value : sqrt(value))`
- Supports isotropic and anisotropic modes

`KisDistanceInformation` tracks accumulation between dabs:
- Last dab position and drawing angle
- Dab sequence number
- Max pressure during stroke
- `getNextPointPosition()` finds the next dab location

### KisPainter — The Compositing Layer

`KisPainter` is the rendering context:
- `bitBlt()` / `bltFixed()` — composite source onto target with composite op + opacity + selection
- `renderMirrorMask()` — handles symmetry painting by mirroring dabs across axes
- `renderDabWithMirroringNonIncremental()` — for non-incremental engines (Experiment)
- Properties: compositeOpId, opacity, flow, averageOpacity, paintColor, pattern, gradient, selection, channelFlags, mirror axes

### Async Dab Rendering (Pixel Brush)

The pixel brush engine (`KisBrushOp`) uses `KisDabRenderingExecutor` to queue dab requests as `KisDabCacheUtils::DabRequestInfo` objects. These are rendered in background threads. `doAsynchronousUpdate()` batches completed dabs and composites them. Rolling averages of spacing, dab count, and update time optimize the async paint period.

---

## 5. Presets (.kpp Format)

### File Format

A `.kpp` file is literally a **PNG image** with metadata in PNG text chunks:

- **Image data:** A thumbnail/preview of the brush
- **`version` text chunk:** `"2.2"` (legacy) or `"5.0"` (current)
- **`preset` text chunk:** An XML document containing all settings

This means `.kpp` files can be previewed as regular images by any PNG viewer.

### XML Structure

```xml
<Preset paintopid="paintbrush" name="My Brush" embedded_resources="2">
  <resources>
    <resource type="brushes" md5sum="abc123" name="tip.gbr" filename="tip.gbr">
      <![CDATA[base64-encoded-resource-data]]>
    </resource>
  </resources>
  <param name="paintop" type="string">paintbrush</param>
  <param name="PaintOpSettings/isAirbrushing" type="bool">false</param>
  <param name="OpacityValue" type="string">curve-data</param>
  <!-- ... all settings as param elements ... -->
</Preset>
```

### Version Differences

**Version 2.2 (legacy):**
- Resources are referenced by filename but NOT embedded
- External brush tips/patterns must exist in the user's resource folder
- Resource filenames tracked via `"dependent_resources_filenames"` metadata

**Version 5.0 (current):**
- All linked resources (brushes, patterns, gradients) are **embedded as base64** in `<resources>`
- On import, embedded resources are "side-loaded" into the resource database if they don't already exist (matched by MD5 + filename + name)
- Fully self-contained — no external dependencies

### Loading Flow

1. Open file as PNG with `QImageReader`
2. Extract `version` and `preset` text chunks
3. Read the PNG image as thumbnail
4. Apply workarounds (broken presets: nested CDATA, non-base64 pattern MD5)
5. Parse XML → `KisPaintOpSettings` (a `KisPropertiesConfiguration` key-value store)

### Saving Flow

1. Build XML from `KisPaintOpSettings::toXML()` — all settings as `<param>` elements
2. Serialize linked resources as base64 in `<resources>`
3. Sanitize (remove texture settings if texture disabled)
4. Write as PNG with text metadata, always version `"5.0"`

---

## 6. ABR (Adobe Brush) Import

### Key Files

| File | Purpose |
|------|---------|
| `libs/brush/kis_abr_brush_collection.h/.cpp` | Binary format parser — the main ABR loader |
| `libs/brush/kis_abr_brush.h/.cpp` | Individual ABR brush wrapper |
| `libs/brush/KisAbrStorage.h/.cpp` | Resource storage plugin for .abr files |

### Supported ABR Versions

```cpp
struct AbrInfo { short version; short subversion; short count; };
```

| Version | Subversion | Status |
|---------|-----------|--------|
| 1 | — | Supported (sampled brushes, no names) |
| 2 | — | Supported (sampled brushes, UCS-2 names) |
| 6 | 1 | Supported (8BIM sections) |
| 6 | 2 | Supported (8BIM sections, larger skip) |
| 3, 4, 5, 7+ | — | **Unsupported** |

### Binary Format Parsing

**Version 1/2 (`abr_brush_load_v12`):**
1. Read `brush_type`: type 1 = computed (UNSUPPORTED, skipped), type 2 = sampled
2. Read `brush_size` (for skip-on-error)
3. Skip 6 bytes (misc + spacing)
4. If v2: read UCS-2 brush name (length-prefixed 16-bit chars)
5. Skip 9 bytes (antialiasing + bounds shorts)
6. Read bounding box: top, left, bottom, right
7. Read depth + compression flag
8. If `compression == 0`: raw read. Otherwise: **PackBits RLE decode**
9. Height limit: 16384 pixels

**Version 6 (`abr_brush_load_v6`):**
1. Navigate to `"samp"` section by scanning for `8BIM` tags
2. Count brushes by iterating the sample section (4-byte aligned records)
3. Per brush: read size, skip key data (37 bytes + version-specific extra: 10 for sub1, 264 for sub2)
4. Read bounding box, depth, compression — same pipeline as v1/2

**RLE decompression:** Standard PackBits/Photoshop RLE. Per-scanline compressed lengths, then decode where negative run-length = repetition, positive = literal copy.

### Conversion to Krita Brushes

```cpp
value = 255 - buffer[pos];  // ABR black → Krita white (opaque)
pixel[x] = qRgb(value, value, value);
```

The raw grayscale buffer is inverted and converted to a QImage. Each brush becomes a `KisAbrBrush` (extends `KisScalingSizeBrush`), type `MASK`, default spacing 0.25.

### Limitations

- **Computed brushes (type 1) are not supported** — only sampled (type 2)
- **Grayscale only** — no color/RGBA extraction
- **Read-only** — `save()` returns false, `isSerializable()` returns false
- **No individual loading** — must go through `KisAbrBrushCollection`
- **No dynamics extracted** — Photoshop spacing, angle, etc. are ignored (there's a `XXX: call extra setters` comment in the code)
- **Height limit:** 16384 pixels for v1/v2
- **Only versions 1, 2, and 6 (sub 1/2)** — everything else is rejected

### Resource System Integration

`KisAbrStorage` registers as a `KisStoragePlugin` for the `AdobeBrushLibrary` storage type:
- `AbrIterator` lazy-loads the brush collection on first access
- `AbrTagIterator` creates one tag per `.abr` file containing all its brushes
- No versioning support

---

## 7. Resource Management

### Architecture

```
KisResourceLocator (singleton)
  ├── KisResourceStorage (Folder)     — default resources directory
  ├── KisResourceStorage (Bundle)     — .bundle archives
  ├── KisResourceStorage (ABR)        — .abr files via KisAbrStorage
  ├── KisResourceStorage (ASL)        — .asl files
  ├── KisResourceStorage (Memory)     — temporary/in-memory
  └── KisResourceStorage (Font)       — system fonts
         ↓
   SQLite cache database (KisResourceCacheDb)
         ↓
   KisAllResourcesModel (QAbstractTableModel)
         ↓
   KisResourceModel (QSortFilterProxyModel — filters active/inactive)
```

### Discovery Flow

1. `KisResourceLocator::initialize()` scans the resource location at startup
2. `findStorages()` discovers all storage backends (folders, bundles, .abr files, etc.)
3. Each storage's `KisStoragePlugin` provides `ResourceIterator`s that enumerate resources by type
4. Resources are inserted into the SQLite cache database
5. `KisAllResourcesModel` queries this database, loads actual resource objects on demand
6. `KisQImagePyramid` in `KisBrush` lazily generates mip-mapped versions on first use

### Tagging

`KisTag` objects are loaded from `.tag` files. Two special pseudo-tags: `All` (id=-2) and `AllUntagged` (id=-1). `KisTagResourceModel` manages the many-to-many tag-to-resource relationship. ABR collections auto-generate one tag per `.abr` file.
