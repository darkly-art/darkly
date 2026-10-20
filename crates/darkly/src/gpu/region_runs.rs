//! A region of a layer partitioned into copyable blocks, for undo storage.
//!
//! An undo entry stores the pre-operation pixels of the region an operation
//! changed. Storing the single bounding rect of every changed pixel wastes
//! most of its bytes on thin or curved strokes (a diagonal across a 4096²
//! layer has a 64 MB bounding rect but touches about 3 MB of 64x64 tiles), so
//! the stroke-end diff also reports which tiles changed, and
//! [`RegionRuns::from_tile_flags`] packs them into horizontal runs of tiles
//! clipped to the tight bounding rect. Whichever of the runs or the bounding
//! rect costs fewer bytes is kept, so a region is never larger than the
//! single-rect form.
//!
//! The tile grid and the buffer layout live together because they are two
//! views of one thing: the diff pass reads [`TileGrid`] to size its flag
//! buffer, and the region store reads [`RegionRuns::layout`] to place each
//! run in an entry's buffer. Runs are canvas-space rects (the Storage Frame
//! Rule), so a region stays valid across layer growth.
//!
//! The granularity follows Krita's undo, which saves each 64x64 tile on its
//! first write within a transaction (`krita/libs/image/tiles3/kis_tile.cc`
//! `KisTile::lockForWrite`, `kis_memento_manager.cc`
//! `KisMementoManager::registerTileChange`). Darkly's layers are flat
//! textures, so the changed tiles are derived by a diff rather than observed.

use crate::coord::CanvasRect;

/// Alignment required by wgpu for `bytes_per_row` in buffer/texture copies.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Tile height in texels, shared by every format.
const TILE_HEIGHT: u32 = 64;

/// Most runs a region may hold before neighbouring runs are merged. Each run
/// is one copy command at commit and at restore. Simulated stroke shapes
/// (thin diagonal, circle, S curve, scribble, spray), runs before the cap:
///
/// | Shape                  | Canvas | Runs | Tile MB | Bbox MB |
/// |------------------------|--------|------|---------|---------|
/// | thin diagonal          | 4096   | 58   | 2.7     | 52      |
/// | circle                 | 4096   | 88   | 5.1     | 33      |
/// | spray, 300 dots        | 4096   | 385  | 13      | 64      |
/// | spray, 1000 dots       | 8192   | 1372 | 43      | 256     |
/// | every other tile       | 4096   | 2048 | 32      | 64      |
///
/// 2048 leaves every realistic stroke uncoarsened up to 8K and bounds a
/// commit at 2048 copies (a few ms of command encoding, once per stroke).
/// Above it, the smallest gaps are filled first, so bytes grow gradually
/// instead of jumping to the bounding rect.
pub const MAX_RUNS: usize = 2048;

/// Bytes per texel of `format`'s copyable aspect.
pub fn bytes_per_pixel(format: wgpu::TextureFormat) -> u32 {
    format.block_copy_size(None).unwrap_or(1)
}

/// Row byte count padded to wgpu's copy alignment.
pub fn padded_row(width: u32, bytes_per_pixel: u32) -> u32 {
    (width * bytes_per_pixel).div_ceil(COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT
}

/// The tile grid a format is diffed and stored on, anchored at the texture's
/// texel origin. One tile row is exactly `COPY_BYTES_PER_ROW_ALIGNMENT` bytes
/// (64x64 for RGBA8, 256x64 for R8), so a run of tiles has no row padding and
/// every tile costs the same 16 KB.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TileGrid {
    pub tile_w: u32,
    pub tile_h: u32,
}

impl TileGrid {
    pub fn for_format(format: wgpu::TextureFormat) -> Self {
        TileGrid {
            tile_w: COPY_ROW_ALIGNMENT / bytes_per_pixel(format),
            tile_h: TILE_HEIGHT,
        }
    }

    /// Tiles covering `width × height` texels, as `(columns, rows)`.
    pub fn tiles(&self, width: u32, height: u32) -> (u32, u32) {
        (width.div_ceil(self.tile_w), height.div_ceil(self.tile_h))
    }

    /// `u32` words holding one changed-flag bit per tile of `width × height`.
    pub fn flag_words(&self, width: u32, height: u32) -> usize {
        let (tx, ty) = self.tiles(width, height);
        (tx as usize * ty as usize).div_ceil(32)
    }
}

/// One run's place in an entry's packed buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RunLayout {
    pub rect: CanvasRect,
    /// Byte offset of the run's first row; a multiple of 256.
    pub offset: u64,
    /// Padded bytes per row of this run.
    pub bytes_per_row: u32,
}

/// A non-empty set of pairwise-disjoint canvas rects, stored back to back
/// (each as padded rows) in one buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionRuns(Vec<CanvasRect>);

impl From<CanvasRect> for RegionRuns {
    fn from(rect: CanvasRect) -> Self {
        RegionRuns(vec![rect])
    }
}

impl RegionRuns {
    /// Pack the tiles flagged in `flags` (bit `i` of the flat tile index, row
    /// major over `layer_extent` on `grid`) into horizontal runs, clip each to
    /// `layer_extent` and `tight_bbox`, merge across the smallest gaps while
    /// there are more than [`MAX_RUNS`], and return the runs if they cost
    /// fewer bytes than `tight_bbox` alone, else `tight_bbox` as one run.
    pub fn from_tile_flags(
        flags: &[u32],
        grid: TileGrid,
        layer_extent: CanvasRect,
        tight_bbox: CanvasRect,
        bpp: u32,
    ) -> Self {
        let (tiles_x, tiles_y) = grid.tiles(layer_extent.width, layer_extent.height);
        let flagged = |i: u32, j: u32| {
            let idx = (j * tiles_x + i) as usize;
            flags
                .get(idx / 32)
                .is_some_and(|w| w & (1 << (idx % 32)) != 0)
        };

        // Maximal runs of flagged tiles per tile row, as `[i0, i1)` columns.
        let mut rows: Vec<Vec<(u32, u32)>> = (0..tiles_y)
            .map(|j| {
                let mut runs = Vec::new();
                let mut i = 0;
                while i < tiles_x {
                    if flagged(i, j) {
                        let start = i;
                        while i < tiles_x && flagged(i, j) {
                            i += 1;
                        }
                        runs.push((start, i));
                    }
                    i += 1;
                }
                runs
            })
            .collect();

        coarsen(&mut rows, MAX_RUNS);

        let runs: Vec<CanvasRect> = rows
            .iter()
            .enumerate()
            .flat_map(|(j, row)| {
                row.iter().filter_map(move |&(i0, i1)| {
                    CanvasRect::from_xywh(
                        layer_extent.origin.x + (i0 * grid.tile_w) as i32,
                        layer_extent.origin.y + j as i32 * grid.tile_h as i32,
                        (i1 - i0) * grid.tile_w,
                        grid.tile_h,
                    )
                    .intersect(layer_extent)?
                    .intersect(tight_bbox)
                })
            })
            .collect();

        let bbox = RegionRuns::from(tight_bbox);
        if !runs.is_empty() && byte_size_of(&runs, bpp) < bbox.byte_size(bpp) {
            RegionRuns(runs)
        } else {
            bbox
        }
    }

    /// The tiles on `grid` that any of `rects` overlaps, packed and costed as
    /// [`Self::from_tile_flags`] packs a diff's, with the rects' union as
    /// the tight bbox: the region of an operation that knows where it wrote
    /// (a brush stroke's dab footprints) without diffing. `None` when no
    /// rect overlaps `layer_extent`.
    pub fn covering(
        rects: impl IntoIterator<Item = CanvasRect>,
        grid: TileGrid,
        layer_extent: CanvasRect,
        bpp: u32,
    ) -> Option<Self> {
        let (tiles_x, _) = grid.tiles(layer_extent.width, layer_extent.height);
        let mut flags = vec![0u32; grid.flag_words(layer_extent.width, layer_extent.height)];
        let mut bbox: Option<CanvasRect> = None;
        for rect in rects {
            let Some(rect) = rect.intersect(layer_extent) else {
                continue;
            };
            bbox = Some(bbox.map_or(rect, |b| b.union(rect)));
            let x0 = (rect.origin.x - layer_extent.origin.x) as u32;
            let y0 = (rect.origin.y - layer_extent.origin.y) as u32;
            for j in y0 / grid.tile_h..=(y0 + rect.height - 1) / grid.tile_h {
                for i in x0 / grid.tile_w..=(x0 + rect.width - 1) / grid.tile_w {
                    let idx = (j * tiles_x + i) as usize;
                    flags[idx / 32] |= 1 << (idx % 32);
                }
            }
        }
        Some(Self::from_tile_flags(
            &flags,
            grid,
            layer_extent,
            bbox?,
            bpp,
        ))
    }

    pub fn rects(&self) -> &[CanvasRect] {
        &self.0
    }

    /// Bytes the packed buffer holds: each run's padded rows.
    pub fn byte_size(&self, bpp: u32) -> u64 {
        byte_size_of(&self.0, bpp)
    }

    /// Each run with its offset and row pitch in the packed buffer.
    pub fn layout(&self, bpp: u32) -> impl Iterator<Item = RunLayout> + '_ {
        self.0.iter().scan(0u64, move |offset, &rect| {
            let bytes_per_row = padded_row(rect.width, bpp);
            let run = RunLayout {
                rect,
                offset: *offset,
                bytes_per_row,
            };
            *offset += bytes_per_row as u64 * rect.height as u64;
            Some(run)
        })
    }
}

fn byte_size_of(rects: &[CanvasRect], bpp: u32) -> u64 {
    rects
        .iter()
        .map(|r| padded_row(r.width, bpp) as u64 * r.height as u64)
        .sum()
}

/// Merge adjacent runs on the same row across the smallest gaps until at most
/// `cap` runs remain. Filling one gap leaves every other gap's width
/// unchanged, so sorting the gaps once and filling the smallest is optimal
/// for the bytes it adds.
fn coarsen(rows: &mut [Vec<(u32, u32)>], cap: usize) {
    let total: usize = rows.iter().map(Vec::len).sum();
    if total <= cap {
        return;
    }
    // (gap width, row, index of the run before the gap)
    let mut gaps: Vec<(u32, usize, usize)> = rows
        .iter()
        .enumerate()
        .flat_map(|(j, row)| {
            row.windows(2)
                .enumerate()
                .map(move |(k, w)| (w[1].0 - w[0].1, j, k))
        })
        .collect();
    gaps.sort_unstable();
    let mut joins: Vec<Vec<bool>> = rows.iter().map(|r| vec![false; r.len()]).collect();
    for &(_, j, k) in gaps.iter().take(total - cap) {
        joins[j][k + 1] = true;
    }
    for (row, joins) in rows.iter_mut().zip(joins) {
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(row.len());
        for (&run, join) in row.iter().zip(joins) {
            match merged.last_mut() {
                Some(last) if join => last.1 = run.1,
                _ => merged.push(run),
            }
        }
        *row = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGBA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    fn flags_for(grid: TileGrid, extent: CanvasRect, tiles: &[(u32, u32)]) -> Vec<u32> {
        let (tx, _) = grid.tiles(extent.width, extent.height);
        let mut flags = vec![0u32; grid.flag_words(extent.width, extent.height)];
        for &(i, j) in tiles {
            let idx = (j * tx + i) as usize;
            flags[idx / 32] |= 1 << (idx % 32);
        }
        flags
    }

    #[test]
    fn padded_row_alignment() {
        assert_eq!(padded_row(128, 4), 512);
        assert_eq!(padded_row(100, 4), 512);
        assert_eq!(padded_row(64, 1), 256);
        assert_eq!(padded_row(256, 1), 256);
        assert_eq!(padded_row(1, 4), 256);
    }

    #[test]
    fn tile_rows_are_one_copy_alignment_wide() {
        assert_eq!(
            TileGrid::for_format(RGBA),
            TileGrid {
                tile_w: 64,
                tile_h: 64
            }
        );
        assert_eq!(
            TileGrid::for_format(wgpu::TextureFormat::R8Unorm),
            TileGrid {
                tile_w: 256,
                tile_h: 64
            }
        );
    }

    #[test]
    fn consecutive_tiles_merge_and_gaps_split() {
        let grid = TileGrid::for_format(RGBA);
        let extent = CanvasRect::from_xywh(0, 0, 1024, 1024);
        let flags = flags_for(grid, extent, &[(1, 2), (2, 2), (5, 2), (9, 9)]);
        let bbox = CanvasRect::from_xywh(64, 128, 576, 512);
        let runs = RegionRuns::from_tile_flags(&flags, grid, extent, bbox, 4);
        assert_eq!(
            runs.rects(),
            &[
                CanvasRect::from_xywh(64, 128, 128, 64),
                CanvasRect::from_xywh(320, 128, 64, 64),
                CanvasRect::from_xywh(576, 576, 64, 64),
            ]
        );
    }

    #[test]
    fn runs_clip_to_the_tight_bbox_and_layer_extent() {
        let grid = TileGrid::for_format(RGBA);
        // Not a multiple of 64, and offset in canvas space.
        let extent = CanvasRect::from_xywh(-100, -50, 1000, 1000);
        // Last column and last row are partial tiles.
        let flags = flags_for(grid, extent, &[(0, 0), (15, 15)]);
        let bbox = CanvasRect::from_xywh(-90, -40, 985, 985);
        let runs = RegionRuns::from_tile_flags(&flags, grid, extent, bbox, 4);
        assert_eq!(
            runs.rects(),
            &[
                CanvasRect::from_xywh(-90, -40, 54, 54),
                CanvasRect::from_xywh(860, 910, 35, 35),
            ]
        );
    }

    #[test]
    fn a_thin_horizontal_stroke_keeps_its_bbox() {
        let grid = TileGrid::for_format(RGBA);
        let extent = CanvasRect::from_xywh(0, 0, 1024, 1024);
        let row: Vec<(u32, u32)> = (0..16).map(|i| (i, 3)).collect();
        let flags = flags_for(grid, extent, &row);
        let bbox = CanvasRect::from_xywh(0, 200, 1024, 8);
        let runs = RegionRuns::from_tile_flags(&flags, grid, extent, bbox, 4);
        // The one run clipped to the bbox costs exactly the bbox; a tie keeps
        // the bbox.
        assert_eq!(runs, RegionRuns::from(bbox));
    }

    #[test]
    fn scattered_tiles_beat_their_bbox() {
        let grid = TileGrid::for_format(RGBA);
        let extent = CanvasRect::from_xywh(0, 0, 1024, 1024);
        let diagonal: Vec<(u32, u32)> = (0..16).map(|i| (i, i)).collect();
        let flags = flags_for(grid, extent, &diagonal);
        let runs = RegionRuns::from_tile_flags(&flags, grid, extent, extent, 4);
        assert_eq!(runs.rects().len(), 16);
        assert_eq!(runs.byte_size(4), 16 * 64 * 256);
    }

    #[test]
    fn covering_flags_every_tile_a_rect_touches() {
        let grid = TileGrid::for_format(RGBA);
        let extent = CanvasRect::from_xywh(-64, 0, 1024, 1024);
        // Straddles two tile columns; the second sits two tile rows down and
        // pokes past the extent's left edge.
        let rects = [
            CanvasRect::from_xywh(40, 10, 40, 20),
            CanvasRect::from_xywh(-80, 140, 30, 10),
            CanvasRect::from_xywh(5000, 5000, 8, 8),
        ];
        let runs = RegionRuns::covering(rects, grid, extent, 4).expect("two rects overlap");
        // Tiles (1..3, 0) and (0, 2), clipped to the rects' union
        // x -64..80, y 10..150.
        assert_eq!(
            runs.rects(),
            &[
                CanvasRect::from_xywh(0, 10, 80, 54),
                CanvasRect::from_xywh(-64, 128, 64, 22),
            ]
        );
        assert!(RegionRuns::covering([rects[2]], grid, extent, 4).is_none());
    }

    #[test]
    fn coarsening_fills_the_smallest_gaps_first_and_stops_at_the_cap() {
        let mut rows = vec![vec![(0, 1), (2, 3), (6, 7)], vec![(0, 1), (3, 4)]];
        coarsen(&mut rows, 3);
        // Gaps: row 0 has 1 and 3, row 1 has 2. Two fills, smallest first.
        assert_eq!(rows, vec![vec![(0, 3), (6, 7)], vec![(0, 4)]]);

        let mut untouched = vec![vec![(0, 1), (2, 3)]];
        coarsen(&mut untouched, 2);
        assert_eq!(untouched, vec![vec![(0, 1), (2, 3)]]);
    }

    #[test]
    fn every_other_tile_past_the_cap_is_coarsened_to_it() {
        let grid = TileGrid::for_format(RGBA);
        // 64 columns x 80 rows, every other tile: 2560 runs before coarsening.
        let extent = CanvasRect::from_xywh(0, 0, 4096, 80 * 64);
        let tiles: Vec<(u32, u32)> = (0..80)
            .flat_map(|j| (0..32).map(move |k| (2 * k, j)))
            .collect();
        let flags = flags_for(grid, extent, &tiles);
        let runs = RegionRuns::from_tile_flags(&flags, grid, extent, extent, 4);
        assert_eq!(runs.rects().len(), MAX_RUNS);
        assert!(runs.byte_size(4) < RegionRuns::from(extent).byte_size(4));
    }

    #[test]
    fn layout_offsets_are_aligned_and_sum_to_the_byte_size() {
        let runs = RegionRuns(vec![
            CanvasRect::from_xywh(0, 0, 100, 3),
            CanvasRect::from_xywh(500, 0, 64, 64),
            CanvasRect::from_xywh(0, 300, 7, 9),
        ]);
        let layout: Vec<RunLayout> = runs.layout(4).collect();
        assert_eq!(
            layout
                .iter()
                .map(|l| (l.offset, l.bytes_per_row))
                .collect::<Vec<_>>(),
            vec![(0, 512), (1536, 256), (1536 + 16384, 256)]
        );
        assert!(layout.iter().all(|l| l.offset % 256 == 0));
        let last = layout.last().unwrap();
        assert_eq!(
            last.offset + last.bytes_per_row as u64 * last.rect.height as u64,
            runs.byte_size(4)
        );
    }
}
