---
name: m1-web-tool-page-staleness
description: The `resultRequest = request` mid-flight drift shared by RotateTool and ReorderTool, and why MergeTool is the only island immune — plus what the reorder island's guards do and do not close.
metadata:
  type: project
---

Measured 2026-09-14 on branch `m1-reorder-page` (uncommitted), the `/reorder-pdf` island.

**The `resultRequest` staleness guard is defeated by its own capture point, in two of three
islands.** `resultRequest = request` is assigned *after* both awaits
(`ReorderTool.svelte:266`, `RotateTool.svelte:260`), and `request` is a `$derived` over the
live controls. Nothing bumps `currentRun` when a control changes, so editing the box while
`phase === "working"` makes `resultRequest` record the *edited* request against the *old*
bytes: `stale` reads false and the link is shown under a preview of a different order. It also
inverts the other way — typing the original order back hides a link whose bytes are correct.

**MergeTool is immune for a structural reason, not a different capture point.** It assigns
`resultRequest` after the await too, but `MergeTool.svelte:400/406/412` disable every control
on `phase === "working"`, so the request cannot drift. Either fix works; only merge has one.

**`choose()` DOES close the cross-file disclosure.** The generation bump at
`ReorderTool.svelte:76` precedes `clearResult()`, and `name`/`order` are captured at :217-219
before `await host.ensure()`. Re-walked: a reply for the previous file cannot reach `result`.
Rotate's two review findings have no analogue here. `wanted` is *not* reset by `choose()`,
which is a separate (low) carry-over, not a disclosure.

**`resolveOrder` holds under adversarial input, measured.** 200,000 random strings over
`{digits, ',', '-', ' ', 1e20-digit runs, 'ـ1e3', '０', '\n', '4294967296'}` against page
counts 1..12: every `ok` result was an exact permutation of `1..pageCount`. Worst case is
bounded not by `MOST_CHARACTERS` (20,000) but by the duplicate check *inside* the expansion
loop — `"1-10000,"` repeated to 20 kB against a 10,000-page doc refuses on the second range's
first element in 1 ms. `named.length <= pageCount` always.

**size-budget arithmetic:** the `+462` total is 449 `page-js` + 13 `page`, not "entirely
page-js" as the `why` field says.

See [[m1-web-rotate-bridge]], [[m1-web-reorder-bridge]], [[m1-web-worker-lifecycle]].
