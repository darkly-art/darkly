# Krita Tool System Architecture

An analysis of how Krita abstracts, structures, and keeps DRY its tool system. Based on direct examination of the Krita codebase at `krita/`.

---

## Class Hierarchy

```
KoToolBase                                    [libs/flake/KoToolBase.h]
  │  Framework-generic base. Pure virtual mouse events, option widgets,
  │  activate/deactivate lifecycle. Part of the Flake canvas library
  │  (originally from KOffice, predates Krita-specific code).
  │
  └─ KisTool                                  [libs/ui/tool/kis_tool.h]
       │  Krita-specific base. Overrides raw mouse events and routes them
       │  to semantic "primary action" methods. Adds a mode state machine,
       │  coordinate conversion helpers, and resource accessors.
       │
       └─ KisToolPaint                         [libs/ui/tool/kis_tool_paint.h]
            │  Adds brush outline rendering, composite op handling,
            │  foreground/background color access.
            │
            ├─ KisToolFreehand                  [libs/ui/tool/kis_tool_freehand.h]
            │    │  Delegates entire stroke lifecycle to KisToolFreehandHelper.
            │    │  Defines three virtual methods: initStroke, doStroke, endStroke.
            │    │
            │    ├─ KisToolBrush                [plugins/tools/basictools/kis_tool_brush.h]
            │    │    Nearly empty — just adds smoothing UI widgets (stabilizer
            │    │    distance, tail aggressiveness, etc). All painting logic
            │    │    lives in KisToolFreehand and KisToolFreehandHelper.
            │    │
            │    └─ KisToolMultihand            [plugins/tools/tool_multihand/]
            │         Symmetry/mirror painting. Multiple KisPainter instances.
            │
            ├─ KisToolFill                      [plugins/tools/basictools/kis_tool_fill.h]
            │    One-shot flood fill. Does NOT use KisToolFreehand or PaintOps.
            │    Uses KisStrokeStrategyUndoCommandBased with KisFillCommand.
            │
            ├─ KisToolGradient                  [plugins/tools/basictools/kis_tool_gradient.h]
            │    Start+end point tool. Does NOT use strokes directly.
            │    Uses KisProcessingApplicator with KisGradientPainter.
            │
            └─ KisToolShape                     [libs/ui/tool/kis_tool_shape.h]
                 Base for shape-drawing tools (rectangle, ellipse, polyline).

KoInteractionTool                              [libs/flake/tools/KoInteractionTool.h]
  └─ KoToolBase
       Strategy pattern: each mouse press creates a KoInteractionStrategy
       subclass that handles handleMouseMove() / finishInteraction().
       Used for vector tools (move, resize, path editing).

KisDelegatedTool<Base, Delegate>               [libs/ui/tool/kis_delegated_tool.h]
       Template class that composes two tool objects, forwarding events
       and merging option widgets. Used to embed a Flake vector tool
       inside a Krita tool for mixed raster/vector editing.
```

---

## Event Routing: From Qt to Tool

### The Proxy Layer

Tools never receive Qt events directly. All input flows through `KoToolProxy`:

```
Qt mouse/tablet event
  → KoToolProxy::mousePressEvent()             [libs/flake/KoToolProxy.h]
      → routes to active KoToolBase::mousePressEvent()
```

`KoToolProxy` is owned by the canvas. It holds a pointer to the currently active tool (set by `KoToolManager`). The canvas calls proxy methods; the proxy forwards to the tool. This means the canvas has zero knowledge of which tool is active or what it does.

### The Template Method in KisTool

`KisTool` overrides the raw `KoToolBase` mouse events and routes them to semantic methods:

```cpp
// libs/ui/tool/kis_tool.h
class KisTool : public KoToolBase {
public:
    // Subclasses implement THESE, not raw mouse events:
    virtual void beginPrimaryAction(KoPointerEvent *event);
    virtual void continuePrimaryAction(KoPointerEvent *event);
    virtual void endPrimaryAction(KoPointerEvent *event);
    virtual void beginPrimaryDoubleClickAction(KoPointerEvent *event);

    // Alternate actions (Ctrl+click, Shift+click, etc):
    virtual void activateAlternateAction(AlternateAction action);
    virtual void deactivateAlternateAction(AlternateAction action);
    virtual void beginAlternateAction(KoPointerEvent *event, AlternateAction action);
    virtual void continueAlternateAction(KoPointerEvent *event, AlternateAction action);
    virtual void endAlternateAction(KoPointerEvent *event, AlternateAction action);
};
```

The routing logic inside `KisTool::mousePressEvent()` checks modifier keys and delegates to either `beginPrimaryAction()` or `beginAlternateAction()`. Subclasses never override `mousePressEvent` — they override `beginPrimaryAction`.

This is classic **Template Method**: the base class defines the algorithm skeleton (check modifiers → route to correct action method), subclasses fill in the specific behavior.

### Mode State Machine

`KisTool` maintains a modal state:

```cpp
enum ToolMode : int {
    HOVER_MODE,
    PAINT_MODE,
    SECONDARY_PAINT_MODE,
    GESTURE_MODE,          // for brush resize drag
    // ...
};
```

Guards like `CHECK_MODE_SANITY_OR_RETURN(KisTool::PAINT_MODE)` are scattered throughout to enforce correct state transitions. A tool sets `PAINT_MODE` in `beginPrimaryAction` and returns to `HOVER_MODE` in `endPrimaryAction`.

### High-Resolution Tablet Events

Painting tools override `primaryActionSupportsHiResEvents()` to return `true`, which causes the event routing layer to deliver tighter streams of tablet input events — essential for smooth brush strokes at high speeds.

---

## Coordinate Conversion

`KisTool` provides helpers that tools use to convert pointer coordinates:

```cpp
QPointF convertToPixelCoord(KoPointerEvent *e);
QPointF convertToPixelCoordAndSnap(KoPointerEvent *e, ...);
```

These apply the inverse view transform (zoom, pan, rotation) to map screen coordinates to canvas-pixel coordinates. Tools work exclusively in canvas-pixel space — they never deal with screen coordinates.

---

## Resource Access

`KisTool` provides accessors to global state:

```cpp
KisImageWSP currentImage();
KoColor currentFgColor();
KoColor currentBgColor();
KisPaintOpPresetSP currentPaintOpPreset();
KisNodeSP currentNode();    // active layer
```

These read from `KoCanvasResourceProvider`, a central reactive store. The tool subscribes to changes via a signal:

```cpp
// KoToolBase_p.h — wired automatically in constructor
q->connect(canvasResourceProvider,
    SIGNAL(canvasResourceChanged(int, const QVariant &)),
    SLOT(canvasResourceChanged(int, const QVariant &)));
```

When the user changes foreground color from the color picker, the active tool's `canvasResourceChanged()` fires automatically. This is how tools stay in sync with global state without polling.

---

## The Freehand Painting Pipeline

This is the most important pipeline — it handles brush, pencil, and all incremental painting tools.

### KisToolFreehand → KisToolFreehandHelper

`KisToolFreehand` is thin. It defines three virtual methods and delegates all of them to `KisToolFreehandHelper`:

```cpp
// libs/ui/tool/kis_tool_freehand.cc
void KisToolFreehand::beginPrimaryAction(KoPointerEvent *event) {
    setMode(KisTool::PAINT_MODE);
    canvas()->viewManager()->disableControls();
    initStroke(event);
}

void KisToolFreehand::continuePrimaryAction(KoPointerEvent *event) {
    CHECK_MODE_SANITY_OR_RETURN(KisTool::PAINT_MODE);
    doStroke(event);
}

void KisToolFreehand::endPrimaryAction(KoPointerEvent *event) {
    CHECK_MODE_SANITY_OR_RETURN(KisTool::PAINT_MODE);
    endStroke();
    setMode(KisTool::HOVER_MODE);
}

// These immediately delegate:
void KisToolFreehand::initStroke(KoPointerEvent *event) {
    m_helper->initPaint(event, convertToPixelCoord(event),
                        image(), currentNode(), image().data());
}
void KisToolFreehand::doStroke(KoPointerEvent *event) {
    m_helper->paintEvent(event);
}
void KisToolFreehand::endStroke() {
    m_helper->endPaint();
}
```

The helper is a standalone `QObject` (`libs/ui/tool/kis_tool_freehand_helper.h`) that encapsulates the entire stroke lifecycle. This separation means the tool class handles input routing while the helper handles painting mechanics — a clean split.

### KisToolFreehandHelper Internals

The helper's private state during a stroke:

```cpp
struct KisToolFreehandHelper::Private {
    KisStrokesFacade *strokesFacade;        // the image (stroke job queue)
    KisStrokeId strokeId;                    // handle to current stroke
    KisResourcesSnapshotSP resources;        // frozen snapshot of brush/color/etc
    KisPaintInformation previousPaintInformation;
    KisPaintInformation olderPaintInformation;
    QList<KisPaintInformation> history;      // for weighted smoothing
    QList<qreal> distanceHistory;
    KisSmoothingOptionsSP smoothingOptions;
    QTimer airbrushingTimer;                 // for airbrush mode (paint while stationary)
    QTimer stabilizerPollTimer;              // for stabilizer smoothing
    QQueue<KisPaintInformation> stabilizerDeque;
    bool hasPaintAtLeastOnce;
    QVector<KisFreehandStrokeInfo*> strokeInfos;  // one per painter (multihand uses many)
};
```

#### Resource Snapshot

At stroke start (`initPaint`), the helper captures a `KisResourcesSnapshot`:

```cpp
// libs/ui/tool/kis_resources_snapshot.h
class KisResourcesSnapshot {
    KoColor currentFgColor;
    KoColor currentBgColor;
    KisPaintOpPresetSP currentPaintOpPreset;
    KoPatternSP currentPattern;
    qreal opacity;
    qreal flow;
    // ... many more
};
```

This snapshot freezes all current settings at stroke start. If the user changes the brush size or color mid-stroke, the running stroke continues with the original values. This decouples the long-running paint operation from the live UI.

#### Smoothing Pipeline

`paintEvent()` receives raw pointer events and runs them through one of several smoothing algorithms before dispatching paint commands:

```
Raw input event
  → KisPaintingInformationBuilder::continueStroke(event)
      → creates KisPaintInformation (position, pressure, tilt, speed, time)
  → smoothing algorithm (one of):
      NO_SMOOTH:     pass through directly
      SIMPLE:        moving average
      WEIGHTED:      weighted average with configurable factors
      STABILIZER:    delay-based stabilization (pen trails behind cursor)
      PIXEL_PERFECT: pixel art mode (snap to pixel grid, eliminate jaggies)
  → paintLine(pi1, pi2)  or  paintBezierCurve(pi1, cp1, cp2, pi2)
```

The smoothing algorithm selection is per-tool configuration (KisToolBrush exposes UI sliders for stabilizer settings).

#### Stroke Job Dispatch

After smoothing, the helper submits paint jobs to the image's stroke queue:

```cpp
// Submit a point dab:
m_d->strokesFacade->addJob(m_d->strokeId,
    new FreehandStrokeStrategy::Data(strokeInfoId, pi));

// Submit a line segment:
m_d->strokesFacade->addJob(m_d->strokeId,
    new FreehandStrokeStrategy::Data(strokeInfoId, pi1, pi2));
```

These jobs are **asynchronous** — they return immediately to the GUI thread. The image's worker thread dequeues and executes them. This keeps the UI responsive during heavy painting.

---

## The Stroke System

### KisStrokesFacade

The minimal interface tools talk to for async execution:

```cpp
// libs/image/kis_image_interfaces.h
class KisStrokesFacade {
    virtual KisStrokeId startStroke(KisStrokeStrategy *strokeStrategy) = 0;
    virtual void addJob(KisStrokeId id, KisStrokeJobData *data) = 0;
    virtual void endStroke(KisStrokeId id) = 0;
    virtual bool cancelStroke(KisStrokeId id) = 0;
};
```

`KisImage` implements this interface. Tools never hold a `KisImage*` directly — they access it through this facade. This is a four-method surface area for the entire tool→engine boundary.

### Stroke Strategies

Each tool creates a stroke strategy that defines how jobs are executed:

```cpp
// libs/image/kis_stroke_strategy.h
class KisStrokeStrategy {
    virtual void initStrokeCallback();        // called once at stroke start
    virtual void doStrokeCallback(KisStrokeJobData *data);  // called per job
    virtual void finishStrokeCallback();      // called at stroke end
    virtual void cancelStrokeCallback();      // called on cancel
};
```

Different tools use different strategies:

| Tool | Strategy | Purpose |
|------|----------|---------|
| Brush/Freehand | `FreehandStrokeStrategy` | Dispatches paint dabs/lines to KisPainter via KisPaintOp |
| Fill | `KisStrokeStrategyUndoCommandBased` | Wraps KisFillCommand as an undo-command job |
| Gradient | `KisProcessingApplicator` (creates its own strategy) | Lambda-based one-shot operation |

### FreehandStrokeStrategy

The strategy used by all incremental painting tools:

```cpp
// libs/ui/tool/strokes/freehand_stroke.h
class FreehandStrokeStrategy : public KisPainterBasedStrokeStrategy {
    void doStrokeCallback(KisStrokeJobData *data) override {
        // data->type == POINT → maskedPainter->paintAt(data->pi)
        // data->type == LINE  → maskedPainter->paintLine(data->pi1, data->pi2)
    }
};
```

It inherits from `KisPainterBasedStrokeStrategy`, which handles:
- Creating `KisPainter` instances at stroke init
- Managing `KisTransaction` for undo
- Merging indirect painting results at stroke finish
- Issuing dirty rect signals for screen updates

---

## The PaintOp System

PaintOps are the pluggable brush engines — the actual pixel-modifying code.

### KisPaintOp Base

```cpp
// libs/image/brushengine/kis_paintop.h
class KisPaintOp : public KisShared {
public:
    // High-level interface (handles spacing):
    void paintAt(const KisPaintInformation& info, KisDistanceInformation *currentDistance);
    virtual void paintLine(const KisPaintInformation &pi1, const KisPaintInformation &pi2,
                           KisDistanceInformation *currentDistance);
    virtual void paintBezierCurve(...);

protected:
    // Subclasses implement this — paint a single dab:
    virtual KisSpacingInformation paintAt(const KisPaintInformation& info) = 0;
    virtual KisSpacingInformation updateSpacingImpl(const KisPaintInformation &info) const = 0;

    KisPainter* painter() const;   // pixel painter
    KisPaintDeviceSP source() const;
};
```

The public `paintAt(info, distance)` handles dab spacing — it calls the protected `paintAt(info)` at correct intervals based on the distance traveled and the spacing returned by the previous dab. The subclass only needs to implement "paint one dab at this point" — spacing iteration is handled by the base.

### KisBrushOp (Standard Pixel Brush)

```cpp
// plugins/paintops/defaultpaintops/brush/kis_brushop.cpp
KisSpacingInformation KisBrushOp::paintAt(const KisPaintInformation& info) {
    qreal scale = m_sizeOption.apply(info);        // pressure → size
    qreal rotation = m_rotationOption.apply(info);  // pressure/tilt → rotation
    qreal ratio = m_ratioOption.apply(info);        // aspect ratio
    KisDabShape shape(scale, ratio, rotation);

    // Apply scatter, opacity, flow from pressure curves...

    KisDabCacheUtils::DabRequestInfo request(
        painter()->paintColor(), cursorPos, shape, info, ...);
    m_dabExecutor->addDab(request, dabOpacity, dabFlow);

    return effectiveSpacing(scale, rotation, ...);
}
```

The `KisDabRenderingExecutor` composites dab bitmaps onto the target `KisPaintDevice` via `painter()->bltFixed(rect, dabsQueue)`.

### PaintOp Plugins

Each brush engine is a separate plugin:

```
plugins/paintops/
  ├── defaultpaintops/brush/    KisBrushOp — standard pixel brush
  ├── colorsmudge/              KisColorSmudgeOp — smudge/blend
  ├── hairy/                    KisHairyPaintOp — bristle brush
  ├── chalk/                    KisChalkPaintOp — chalk texture
  ├── sketch/                   KisSketchPaintOp — line sketch
  ├── spray/                    KisSprayPaintOp — particle spray
  └── ...
```

Each plugin provides a `KisPaintOpFactory` that creates `KisPaintOp` instances. PaintOps are created fresh for each stroke from the current preset settings.

---

## Full Call Chain: Mouse Event to Pixel

```
Input event (tablet/mouse)
  │
  ▼
KoToolProxy::mousePressEvent()
  │  routes to active tool
  ▼
KisTool::mousePressEvent()
  │  checks modifiers, sets mode
  ▼
KisToolFreehand::beginPrimaryAction()
  │  setMode(PAINT_MODE), disable UI controls
  ▼
KisToolFreehand::initStroke()
  │  → m_helper->initPaint(event, pixelCoord, image, node, strokesFacade)
  ▼
KisToolFreehandHelper::initPaint()
  │  1. Create KisResourcesSnapshot (freeze colors, brush, opacity)
  │  2. Create FreehandStrokeStrategy
  │  3. strokesFacade->startStroke(strategy)  → get KisStrokeId
  │  4. Record first KisPaintInformation
  ▼
─── per pointer move event ───
  │
KisToolFreehand::continuePrimaryAction()
  │  → m_helper->paintEvent(event)
  ▼
KisToolFreehandHelper::paintEvent()
  │  1. Build KisPaintInformation (position, pressure, tilt, speed)
  │  2. Run smoothing algorithm
  │  3. Call paintLine(pi1, pi2) or paintBezierCurve(...)
  ▼
KisToolFreehandHelper::paintLine()
  │  → strokesFacade->addJob(strokeId, new FreehandStrokeStrategy::Data(LINE, pi1, pi2))
  │    [returns immediately — job is queued]
  ▼
─── on worker thread ───
  │
FreehandStrokeStrategy::doStrokeCallback(data)
  │  → maskedPainter->paintLine(data->pi1, data->pi2)
  ▼
KisPainter::paintLine()
  │  → paintOp->paintLine(pi1, pi2, currentDistance)
  ▼
KisPaintOp::paintLine()
  │  Iterates with spacing, calls paintAt() per dab
  ▼
KisBrushOp::paintAt(info)
  │  Apply size, rotation, scatter, opacity from pressure curves
  │  → m_dabExecutor->addDab(request, opacity, flow)
  ▼
KisDabRenderingExecutor
  │  → painter()->bltFixed(rect, dabsQueue)
  │    Composites dab bitmap onto KisPaintDevice
  ▼
KisPaintDevice / KisDataManager
  │  Tile-based pixel storage. Dirty rects tracked.
  ▼
FreehandStrokeStrategy::issueSetDirtySignals()
  │  → targetNode()->setDirty(dirtyRects)
  │    Triggers projection update → screen repaint
  ▼
─── on pointer up ───
  │
KisToolFreehand::endPrimaryAction()
  │  → m_helper->endPaint()
  ▼
KisToolFreehandHelper::endPaint()
  │  1. Paint final dab if hasPaintAtLeastOnce == false
  │  2. strokesFacade->endStroke(strokeId)
  ▼
FreehandStrokeStrategy::finishStrokeCallback()
  │  (inherited from KisPainterBasedStrokeStrategy)
  │  1. If indirect painting: merge temp device to layer
  │  2. End KisTransaction → produces KUndo2Command
  │  3. undoAdapter->addCommand(command) → pushed onto undo stack
```

---

## Undo Integration

Undo is entirely managed by the stroke strategy — tools never call begin/commit transaction explicitly.

### Per-Stroke Undo

At stroke init (`KisPainterBasedStrokeStrategy::initStrokeCallback()`):
- Creates a `KisTransaction` wrapping the target `KisPaintDevice`
- The transaction records the "before" state of affected tiles

During the stroke:
- All painting goes directly into the target device (or a temporary indirect device)
- Affected tiles are COW-snapshotted by the transaction

At stroke finish (`finishStrokeCallback()`):
```cpp
// libs/ui/tool/strokes/kis_painter_based_stroke_strategy.cpp
void KisPainterBasedStrokeStrategy::finishStrokeCallback() {
    KisPostExecutionUndoAdapter *undoAdapter = m_resources->postExecutionUndoAdapter();

    QSharedPointer<KUndo2Command> parentCommand;
    if (!m_useMergeID) {
        parentCommand.reset(new KUndo2Command());
    } else {
        // Timed merge: rapid consecutive strokes can merge in the undo stack
        parentCommand.reset(new MergeableStrokeUndoCommand(m_resources));
        parentCommand->setTimedID(timedID(this->id()));
    }

    if (indirect && indirect->hasTemporaryTarget()) {
        // Merge temp device to layer, wrap in undo command
        indirect->mergeToLayerThreaded(node, parentCommand.data(), ...);
    } else {
        wrapper->addCommand(m_transaction->endAndTake());
    }

    parentCommand->redo();
    undoAdapter->addCommand(parentCommand);  // → onto undo stack
}
```

### Indirect Painting

When composite op requires it (e.g., opacity < 100%), painting goes to a temporary device. On finish, the temporary device is composited onto the real layer in one operation. This ensures partial-opacity strokes don't double-blend where dabs overlap.

### Timed Merge

Rapid consecutive strokes with the same preset/node can be merged via `MergeableStrokeUndoCommand::timedMergeWith()`. This reduces undo history clutter — multiple quick brush dabs become one undo step instead of ten.

---

## Non-Freehand Tools

### Fill Tool

`KisToolFill` inherits from `KisToolPaint`, NOT `KisToolFreehand`. It uses the stroke infrastructure but with a different strategy:

```cpp
// plugins/tools/basictools/kis_tool_fill.cc
void KisToolFill::beginPrimaryAction(KoPointerEvent *event) {
    KisStrokeStrategyUndoCommandBased *strategy =
        new KisStrokeStrategyUndoCommandBased(
            kundo2_i18n("Flood Fill"), false, image().data());

    m_fillStrokeId = image()->startStroke(strategy);

    image()->addJob(m_fillStrokeId,
        new KisStrokeStrategyUndoCommandBased::Data(
            KUndo2CommandSP(new KisFillCommand(...)),
            KisStrokeJobData::SEQUENTIAL,
            KisStrokeJobData::EXCLUSIVE));

    image()->endStroke(m_fillStrokeId);
}
```

No `KisPaintOp` is involved. The fill algorithm is a `KUndo2Command` that runs as a stroke job. The stroke system handles undo integration.

### Gradient Tool

`KisToolGradient` inherits from `KisToolPaint`. It doesn't even use the stroke queue directly — it uses `KisProcessingApplicator`, a higher-level wrapper:

```cpp
// plugins/tools/basictools/kis_tool_gradient.cc
void KisToolGradient::endPrimaryAction(KoPointerEvent *event) {
    KisProcessingApplicator applicator(image, resources->currentNode(), ...);

    applicator.applyCommand(
        new KisCommandUtils::LambdaCommand([resources, startPos, endPos, ...] () mutable {
            KisGradientPainter painter(device, resources->activeSelection());
            painter.beginTransaction();
            painter.paintGradient(startPos, endPos, ...);
            return painter.endAndTakeTransaction();
        }));

    applicator.end();
}
```

`KisProcessingApplicator` creates its own stroke + undo commands internally. `KisGradientPainter` directly modifies pixels without any PaintOp.

### Summary: Tool Execution Mechanisms

| Tool | Inherits From | PaintOp? | Execution Mechanism |
|------|--------------|----------|---------------------|
| Brush | KisToolFreehand | Yes | FreehandStrokeStrategy via helper, async dab jobs |
| Multihand | KisToolFreehand | Yes | FreehandStrokeStrategy with multiple painters |
| Line | KisToolPaint (via polyline) | Yes | FreehandStrokeStrategy (paints single line segment) |
| Fill | KisToolPaint | No | KisStrokeStrategyUndoCommandBased, flood-fill command |
| Gradient | KisToolPaint | No | KisProcessingApplicator, KisGradientPainter lambda |
| Move | KoInteractionTool | No | Interaction strategy pattern |

---

## Tool Registration & Discovery

### Factory + Registry Pattern

```cpp
// libs/flake/KoToolFactoryBase.h
class KoToolFactoryBase : public QObject {
    virtual KoToolBase *createTool(KoCanvasBase *canvas) = 0;

    // Metadata:
    QString id();
    QString toolTip();
    QString iconName();
    QString section();
    int priority();
    QKeySequence shortcut();
    QString activationShapeId();   // "flake/always" = always available
};
```

```cpp
// libs/flake/KoToolRegistry.h
class KoToolRegistry : public KoGenericRegistry<KoToolFactoryBase*> {
public:
    static KoToolRegistry *instance();    // singleton
    // Inherits: add(), value(), remove()
};
```

### Plugin Registration

Tools are loaded as KDE plugins via `KoPluginLoader`:

```cpp
// plugins/tools/basictools/default_tools.cc
K_PLUGIN_FACTORY_WITH_JSON(DefaultToolsFactory, "kritadefaulttools.json",
    registerPlugin<DefaultTools>();)

DefaultTools::DefaultTools(QObject *parent, const QVariantList &)
    : QObject(parent)
{
    KoToolRegistry::instance()->add(new KisToolBrushFactory());
    KoToolRegistry::instance()->add(new KisToolFillFactory());
    KoToolRegistry::instance()->add(new KisToolMoveFactory());
    // ...
}
```

### Concrete Factory Example

```cpp
// plugins/tools/basictools/kis_tool_brush.h
class KisToolBrushFactory : public KisToolPaintFactoryBase {
public:
    KisToolBrushFactory()
        : KisToolPaintFactoryBase("KritaShape/KisToolBrush")
    {
        setToolTip(i18n("Freehand Brush Tool"));
        setSection(ToolBoxSection::Shape);
        setIconName(koIconNameCStr("krita_tool_freehand"));
        setShortcut(QKeySequence(Qt::Key_B));
        setPriority(0);
        setActivationShapeId(KRITA_TOOL_ACTIVATION_ID);  // "flake/always"
    }

    KoToolBase *createTool(KoCanvasBase *canvas) override {
        return new KisToolBrush(canvas);
    }
};
```

`activationShapeId` is a Flake/KOffice mechanism for conditional tool availability: in an office suite, clicking a text frame activates text-editing tools, clicking a vector path activates path-editing tools, clicking a spreadsheet embed activates spreadsheet tools. Tools appear and disappear from the toolbox based on which shape/region you clicked on. In Krita, this mechanism is dead — every tool (including Krita's own vector tools) sets `activationShapeId = KRITA_TOOL_ACTIVATION_ID` ("flake/always") and the filtering logic never fires. The concept of "disable this tool when the wrong layer type is selected" is valid, but Krita's shape-activation system is not how you'd implement it — a simple check against the active layer type is sufficient.

---

## Tool Options Widgets

Each tool creates its own options UI:

```cpp
// KoToolBase.h
virtual QWidget *createOptionWidget();
virtual QList<QPointer<QWidget>> createOptionWidgets();
```

`KoToolManager` calls `activeTool->optionWidgets()` when a tool is activated and emits `toolOptionWidgetsChanged(controller, widgets)`. The ToolOptions docker receives and displays them.

Widget creation is lazy — only called the first time the tool is activated. Each tool builds its own Qt widgets internally. `KisToolBrush::createOptionWidget()` creates sliders for smoothing distance, tail aggressiveness, delay distance, stabilizer settings, etc.

---

## Design Patterns Summary

| Pattern | Where | Purpose |
|---------|-------|---------|
| **Template Method** | `KisTool` | Base defines event routing skeleton; subclasses fill in `beginPrimaryAction` etc |
| **Proxy** | `KoToolProxy` | Routes all canvas events to active tool without canvas knowing which tool |
| **Facade** | `KisStrokesFacade` | Four-method interface hides the entire async stroke/undo/worker-thread system |
| **Factory + Registry** | `KoToolFactoryBase` + `KoToolRegistry` | Singleton map from ID → factory; tools created on demand |
| **Strategy** | `KoInteractionTool` | Each drag creates a swappable interaction strategy |
| **Command** | `KUndo2Command` | All operations produce undo commands; managed by stroke strategies |
| **Snapshot** | `KisResourcesSnapshot` | Freezes current state at stroke start; decouples stroke from live UI |
| **Observer** | `KoCanvasResourceProvider` | Broadcasts resource changes; tools subscribe via `canvasResourceChanged()` |
| **Delegation** | `KisToolFreehandHelper` | Freehand tool delegates entire stroke lifecycle to standalone helper object |

---

## Key Source Files

| File | Role |
|------|------|
| `libs/flake/KoToolBase.h` | Framework-generic tool base class |
| `libs/flake/KoToolProxy.h` | Event routing proxy between canvas and tool |
| `libs/flake/KoToolRegistry.h` | Singleton tool factory registry |
| `libs/flake/KoToolFactoryBase.h` | Abstract tool factory |
| `libs/flake/KoCanvasResourceProvider.h` | Reactive global resource store (colors, brush, etc) |
| `libs/flake/tools/KoInteractionTool.h` | Strategy-pattern base for interaction tools |
| `libs/ui/tool/kis_tool.h` | Krita tool base: mode state machine, coordinate conversion |
| `libs/ui/tool/kis_tool_paint.h` | Paint tool base: outline, composite op, color access |
| `libs/ui/tool/kis_tool_freehand.h` | Freehand base: delegates to helper |
| `libs/ui/tool/kis_tool_freehand_helper.h` | Stroke lifecycle, smoothing, job dispatch |
| `libs/ui/tool/kis_resources_snapshot.h` | Frozen resource state at stroke start |
| `libs/ui/tool/kis_painting_information_builder.h` | Builds KisPaintInformation from pointer events |
| `libs/ui/tool/strokes/freehand_stroke.h` | FreehandStrokeStrategy: dispatches dabs/lines |
| `libs/ui/tool/strokes/kis_painter_based_stroke_strategy.h` | Transaction + undo for painter-based strokes |
| `libs/ui/tool/strokes/KisFreehandStrokeInfo.h` | Pairs KisPainter + KisDistanceInformation |
| `libs/image/kis_image_interfaces.h` | KisStrokesFacade: 4-method tool→engine interface |
| `libs/image/kis_stroke.h` | Stroke object holding job queue |
| `libs/image/kis_stroke_strategy.h` | Abstract strategy: init/dab/finish/cancel |
| `libs/image/kis_strokes_queue.h` | Queue of strokes, drives worker thread |
| `libs/image/brushengine/kis_paintop.h` | Abstract PaintOp: spacing, dab iteration |
| `plugins/paintops/defaultpaintops/brush/kis_brushop.h` | Standard pixel brush PaintOp |
| `plugins/tools/basictools/kis_tool_brush.h` | Concrete brush tool (mostly UI) |
| `plugins/tools/basictools/kis_tool_fill.h` | Fill tool: one-shot command-based stroke |
| `plugins/tools/basictools/kis_tool_gradient.cc` | Gradient tool: applicator + gradient painter |
| `plugins/tools/basictools/default_tools.cc` | Plugin entry: registers all basic tool factories |
