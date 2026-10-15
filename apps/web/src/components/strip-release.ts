// When the page-picture strip lets go of a tile's pixels, and that it zeroes them first (#199, D4).
//
// Rendered pixels are decoded content (D4). A tile holds its page's picture as an `ImageData`
// until it lets go of it, and it lets go for one of four reasons, each a call site in
// `PageThumbnails.svelte` that `page-thumbnails.test.ts` pins:
//
// - `evict`: the tile left the live window (`LIVE_THUMBNAIL_WINDOW`);
// - `document`: the document changed, so every tile goes;
// - `pagehide`: the page was left, so every drawn tile goes back to waiting;
// - `unmount`: the strip is destroyed.
//
// The pixels are zeroed before the reference is dropped, whatever the reason: a dropped `ImageData`
// is collected when the engine chooses, and holds the picture until then.

import type { TileState } from "./strip-schedule.js";

/** What this module needs of a tile: its state, and the pixels it holds, if any. */
export interface ReleasableTile {
  state: TileState;
  data: { data: Uint8ClampedArray } | null;
}

export type ReleaseReason = "evict" | "document" | "pagehide" | "unmount";

/**
 * Zero every listed tile's pixels and return the tiles without them.
 *
 * `evict` gives each tile `evictedState` (the caller's `releasedState`); `pagehide` puts a drawn
 * tile back to `waiting`, so a page restored from the back-forward cache draws it again; `document`
 * and `unmount` keep each state, since the caller drops the tiles themselves.
 */
export function releaseTiles<T extends ReleasableTile>(
  tiles: readonly T[],
  reason: ReleaseReason,
  evictedState?: TileState,
): T[] {
  for (const tile of tiles) tile.data?.data.fill(0);
  return tiles.map((tile) => {
    let state = tile.state;
    if (reason === "evict" && evictedState !== undefined) state = evictedState;
    if (reason === "pagehide" && tile.state === "drawn") state = "waiting";
    return { ...tile, state, data: null };
  });
}

/** Zero pixels that arrived with nowhere to go: no tile for them, or a tile being redrawn. */
export function zeroPixels(pixels: ArrayBuffer | Uint8ClampedArray | null | undefined): void {
  if (!pixels) return;
  (pixels instanceof ArrayBuffer ? new Uint8Array(pixels) : pixels).fill(0);
}
