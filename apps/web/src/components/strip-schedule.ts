/**
 * Which tiles a strip should draw next, and which it may release.
 *
 * **Extracted from `PageThumbnails.svelte` because it livelocked and nothing could see it.**
 * The component evicted the oldest drawn tile back to `waiting` without asking whether it was
 * still on screen — so on a viewport holding more tiles than the live window, `outstanding()`
 * immediately reported it again and the strip rendered forever with nobody scrolling. Modelled
 * at 112 wanted tiles and a window of 64: **500 requests, 24,064 renders, never converging.**
 * Found by both reviews; reproduced before it was believed.
 *
 * `CLAUDE.md` calls that class by name — "an unbounded loop or allocation is a
 * denial-of-service bug, not a missing nicety" — and this one runs on the reader's own device.
 *
 * So the scheduling is pure functions here rather than closures over component state, and the
 * termination property is a test rather than an argument. A source scan cannot see a livelock.
 */

/** What a tile is doing. */
export type TileState = "waiting" | "drawn" | "refused" | "released";

/**
 * A tile the window pushed out while it was still on screen.
 *
 * **The state that makes the loop terminate.** A released tile is NOT re-requested: it returns
 * to `waiting` only when the viewport reports it leaving, so a static viewport asks for each
 * page at most once. Without it, releasing a wanted tile and re-requesting it is a cycle with
 * no exit that does not depend on the window being larger than the viewport.
 *
 * The cost is stated rather than hidden: on a screen showing more tiles than the window, the
 * ones beyond it stay blank until you scroll. That is a strip doing less than it could. A strip
 * that pinned a core instead is a strip doing harm.
 */
export const RELEASED: TileState = "released";

/** Pages that are wanted, not drawn, not released, and not known to be undrawable. */
export function outstanding(
  wanted: readonly number[],
  states: readonly TileState[],
  refused: ReadonlySet<number>,
): number[] {
  return wanted.filter((page) => states[page - 1] === "waiting" && !refused.has(page));
}

/**
 * Which live tile to push out to make room for `incoming`, or `null` to keep them all.
 *
 * **Prefers a tile that is no longer wanted**, so an ordinary strip — where the viewport is
 * smaller than the window — never releases anything somebody is looking at. Only when every
 * live tile is on screen does it fall back to the oldest, and that one becomes `released`
 * rather than `waiting`.
 */
export function evictable(
  live: readonly number[],
  wanted: readonly number[],
  incoming: number,
): number | null {
  const offScreen = live.find((page) => page !== incoming && !wanted.includes(page));
  if (offScreen !== undefined) return offScreen;
  const oldest = live.find((page) => page !== incoming);
  return oldest ?? null;
}

/** Whether a tile pushed out should be able to come back without a scroll. */
export function releasedState(page: number, wanted: readonly number[]): TileState {
  // A TILE STILL ON SCREEN BECOMES `released`, NOT `waiting`. That is the whole terminating
  // condition; see `RELEASED`.
  return wanted.includes(page) ? "released" : "waiting";
}
