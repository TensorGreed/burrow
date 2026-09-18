<script lang="ts">
  // A strip of page pictures, shared by `/split-pdf` and `/rotate-pdf`.
  //
  // #57, and the ceiling it needed is ADR 0027. Everything here follows from three facts that
  // were measured rather than assumed, and each one shapes the code in a way that would look
  // arbitrary without them:
  //
  //   1. A render is INTERRUPTIBLE, a page LOAD is not. `FPDF_LoadPage` parses the content
  //      stream in one call with no checkpoint -- 757 MiB and 1.2 s on one adversarial page,
  //      1,765 MiB and 3.5 s on another. So the per-page deadline can be exceeded somewhere
  //      nothing can refuse it, and the only outcome there is the watchdog terminating the
  //      worker. THAT IS WHY THE STRIP IS A SEQUENCE OF REQUESTS rather than one: a dead
  //      worker then costs the request in flight, not the strip.
  //   2. THE ENGINE HEAP NEVER SHRINKS on this platform, so "the previous page has freed" is
  //      not observable. What makes it true is the worker being replaced, which Rust asks for
  //      through `reply.recycle` -- acted on here by starting the next request, which gets a
  //      fresh worker.
  //   3. A WINDOW SMALLER THAN THE VIEWPORT MUST DEGRADE, NOT LOOP. The first version evicted
  //      the oldest drawn tile back to `waiting` without asking whether it was still on screen,
  //      so `outstanding()` reported it again immediately and the strip rendered forever with
  //      nobody scrolling -- 500 requests and 24,064 renders in a model, on a static 1920x1080
  //      viewport. `strip-schedule.ts` holds the fix and the termination test; the short version
  //      is that a tile pushed out while still visible becomes `released` and waits for a scroll.
  //   4. THE TOOL WORKS WITHOUT THIS. `/split-pdf` and `/rotate-pdf` select by page number and
  //      shipped with no pictures at all (ADR 0020). So every failure here is a tile that says
  //      what it is, never a banner and never a blocked control: the strip is an aid, and an
  //      aid that breaks must not take the tool with it.
  //
  // NO `<img>`, AND THAT IS THE CSP RATHER THAN A PREFERENCE. `img-src 'self'` admits no
  // `data:` and no `blob:` (ADR 0014), and the refusal is silent. Pixels arrive as a
  // transferred `ArrayBuffer` and are put into a `<canvas>`; there is no URL, so there is
  // nothing to revoke and nothing that can outlive the tile.
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { ENGINE_PAUSED, LIMITS, RENDER, createToolHost, hostKind } from "./tool-host.js";
  import { evictable, outstanding as outstandingPages, releasedState } from "./strip-schedule.js";
  import type { TileState } from "./strip-schedule.js";
  import { LIVE_THUMBNAIL_WINDOW, THUMBNAIL_CSS_HEIGHT, thumbnailBox } from "./thumbnail-policy.js";

  interface Props {
    /** The document. `null` clears the strip. */
    file: File | null;
    /** Pages, once the document operation counted them. `null` or negative means no strip. */
    pageCount: number | null;
    /**
     * True while the tool is running an operation, so the strip must not hold an engine.
     *
     * **PDFIUM IS ABSENT WHILE AN OPERATION RUNS (#107)**, which is narrower than the claim
     * this comment used to make and is the one the measurement supports.
     *
     * "One engine in the tab at a time" was wrong: the DOCUMENTS worker persists after an
     * operation, so qpdf and PDFium are both resident whenever the strip draws at all -- before
     * a split and again afterwards. The withdrawal appeared to prove otherwise only because a
     * withdrawn strip never ran.
     *
     * What the pause achieves is that PDFium is not resident DURING an operation, which is the
     * window where the spike happens: four failures in 580 runs with the strip mounted, zero in
     * 580 without. The strip is the one that yields, because a thumbnail is an aid and the
     * operation is what the person came for.
     */
    paused: boolean;
  }

  const { file, pageCount, paused }: Props = $props();

  /** One tile. `data` is held only while the tile is inside the live window. */
  interface Tile {
    page: number;
    state: TileState;
    width: number;
    height: number;
    data: ImageData | null;
  }

  let tiles = $state<Tile[]>([]);
  /** Pages that killed a worker. Never asked for again -- see `advance`. */
  let refused = $state(new Set<number>());
  /** True once the breaker has latched: the strip stops and says so, and the tool goes on. */
  let unavailable = $state(false);

  const host = createToolHost(undefined, RENDER, { paused: () => paused });
  const box = thumbnailBox(typeof window === "undefined" ? 1 : window.devicePixelRatio);

  /** Which tiles the viewport has asked for, most recent last. */
  let wanted: number[] = [];
  /** Tiles drawn, oldest first -- the live window, released from the front. */
  let live: number[] = [];
  let running = false;
  /** Invalidates everything in flight when the file changes. */
  let generation = 0;
  let observer: IntersectionObserver | null = null;

  // THE STRIP IS REBUILT WHENEVER THE DOCUMENT OR ITS PAGE COUNT CHANGES, and everything in
  // flight for the previous one is invalidated by generation -- the same mechanism the tool
  // islands use for a reply about a document nobody has chosen any more.
  $effect(() => {
    const count = pageCount;
    const chosen = file;
    generation += 1;
    wanted = [];
    live = [];
    refused = new Set();
    unavailable = false;
    tiles =
      chosen && count !== null && count > 0
        ? Array.from({ length: count }, (_, i) => ({
            page: i + 1,
            state: "waiting" as const,
            width: 0,
            height: 0,
            data: null,
          }))
        : [];
  });

  /**
   * Watch a tile, so the strip renders what somebody is looking at.
   *
   * A VIEWPORT WINDOW, NOT A FEATURE WINDOW. ADR 0027 §3: every page gets a picture when you
   * look at it, which is the opposite of ADR 0020's rejected "first N pages only" -- that one
   * gave pictures to part of a document and numbers to the rest with nothing explaining the
   * boundary.
   */
  function watch(node: HTMLElement, page: number) {
    observer ??= new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const at = Number(entry.target.getAttribute("data-page"));
          if (!at) continue;
          if (entry.isIntersecting) {
            if (!wanted.includes(at)) wanted.push(at);
          } else {
            wanted = wanted.filter((p) => p !== at);
            // A TILE THE VIEWPORT HAS LEFT MAY BE ASKED FOR AGAIN. `released` is what stops a
            // tile pushed out while still on screen being re-requested forever; leaving the
            // viewport is the event that lifts it. See `strip-schedule.ts`.
            const tile = tiles[at - 1];
            if (tile?.state === "released") {
              tiles[at - 1] = { ...tile, state: "waiting" };
            }
          }
        }
        void pump();
      },
      // A MARGIN, so a tile is drawn just before it is looked at rather than just after. One
      // tile's height either way: enough to hide the latency of an ordinary page, small enough
      // that a fast scroll does not queue the whole document.
      { rootMargin: `${THUMBNAIL_CSS_HEIGHT}px 0px` },
    );
    observer.observe(node);
    return {
      destroy() {
        observer?.unobserve(node);
      },
    };
  }

  /** Pages that are wanted, not drawn, not released, and not known to be undrawable. */
  function outstanding(): number[] {
    return outstandingPages(
      wanted,
      tiles.map((t) => t.state),
      refused,
    );
  }

  /**
   * Draw what is wanted, one request at a time.
   *
   * ONE REQUEST IN FLIGHT, and it is not only about the engine: the worker is serialised, so a
   * second request would queue behind this one carrying a deadline it did not spend. ADR 0015
   * §2a; the host queues, and this simply does not ask twice.
   */
  // THE RELEASE, AND IT IS THE WHOLE OF #107's FIX ON THIS SIDE.
  //
  // `discardWorker()` terminates the render worker, so PDFium's heap goes with it rather than
  // sitting resident while qpdf does the operation. The tiles already drawn are `ImageData` on
  // the main thread and are untouched -- what is released is the engine, not the pictures, so
  // a person watching sees nothing change.
  //
  // WHAT THIS GUARANTEES, PRECISELY: the strip stops asking and its engine is released when an
  // operation starts. It does NOT guarantee that no instant exists where both are resident --
  // the discard runs in an effect after the flag changes, and a render already inside PDFium
  // ends when the worker is terminated rather than before. The measurement in #107 is what
  // says whether that residue matters; the claim here is not stronger than the mechanism.
  $effect(() => {
    if (paused) {
      host.discardWorker();
      return;
    }
    // Resuming is just asking again: the viewport's wants are still recorded, and tiles that
    // survived are still drawn.
    void pump();
  });

  async function pump(): Promise<void> {
    if (running || unavailable || paused) return;
    const chosen = file;
    if (!chosen) return;
    running = true;
    const mine = generation;
    try {
      // A LOOP, NOT A TAIL CALL. `await pump()` at the end built a promise chain one frame
      // deep per round that never unwound while the strip was active; this says the same thing
      // and retains nothing. Three ordinary things leave work here: a page was refused and the
      // ones after it are still wanted, the reply asked for the worker to be recycled, or the
      // viewport moved while a request was in flight.
      while (mine === generation && !unavailable && !paused) {
        const pages = outstanding();
        if (pages.length === 0) break;
        if (!(await drawRun(chosen, pages, mine))) break;
      }
    } catch (error) {
      // A REFUSAL FROM THE GATE IS NOT A FAILURE. `ensure()` and every acquiring call reject
      // with `ENGINE_PAUSED` while an operation is running (#107) -- by identity, never by
      // reading the value. The pages stay `waiting`; the effect below asks again when the
      // operation ends. Anything else is rethrown rather than swallowed.
      if (error !== ENGINE_PAUSED) throw error;
    } finally {
      running = false;
    }
  }

  /** One request. Returns false when the strip should stop for now. */
  async function drawRun(chosen: File, pages: number[], mine: number): Promise<boolean> {
    {
      // THE ENGINES START HERE AND NOWHERE EARLIER. A page that never scrolls a strip into
      // view never downloads PDFium -- which is ADR 0026's promise, kept at the last possible
      // moment rather than at the first convenient one.
      // ACQUISITION IS GATED, NOT JUST THE PUMP, and this line is the difference between the
      // two versions of #107's fix. `pump` refuses to start while paused; a request that was
      // ALREADY past that point still awaited `ensure()`, and `run` then spawned a fresh worker
      // -- re-fetching PDFium in the middle of the operation the pause exists to protect.
      // Measured: chromium fetched `pdfium.wasm` inside the window while firefox and webkit did
      // not, which is what a race looks like when only one engine is fast enough to lose it.
      if (paused) return false;
      const h = await host.ensure();
      if (mine !== generation || paused) return false;

      const reply = await h.run(
        {
          op: "render",
          blob: chosen,
          pages,
          boxWidth: box.width,
          boxHeight: box.height,
          password: null,
          limits: LIMITS,
        },
        {
          maxDurationMs: LIMITS.maxDurationMs,
          onPage: (page, pixels) => {
            if (mine !== generation) return;
            accept(page.number, page.width, page.height, pixels);
          },
        },
      );
      if (mine !== generation) return false;
      if (!reply.ok && reply.kind === CANCELLED) return false;

      // A WORKER THIS COMPONENT ITSELF DISCARDED IS NOT A PAGE THAT FAILED. Pausing releases
      // the render worker mid-flight (#107), which settles this request exactly as a crash
      // would -- and the inference below would then blame whichever page was in the queue and
      // mark it refused for as long as the file stayed chosen. The pages stay `waiting`; the
      // next round after the operation asks for them again.
      if (paused) return false;

      if (!reply.ok) {
        if (hostKind(reply.kind) === "EngineUnavailable") {
          unavailable = true;
          return false;
        }

        // A DEADLINE IS NOT A PAGE'S FAULT, and treating it as one marked innocent pages
        // permanently. One `Deadline` covers the whole request, so an ordinary hundred-page
        // document at a few hundred milliseconds a page runs out of budget part way -- and the
        // reply is `LimitExceeded`, not a terminated worker. Blaming the next page in order
        // then showed "Too complex to preview" on a page that is fine, for as long as the file
        // stayed chosen. Found by security review.
        //
        // So a spent budget just ends this request: the pages stay `waiting` and the next
        // round continues with a fresh one.
        if (reply.limit === "max_duration_ms") return true;

        // WHICH PAGE ENDED IT IS INFERRED, and the inference is sound for the case it is left
        // to cover -- a worker that was TERMINATED. The worker posts pages in the order they
        // were asked for, one at a time, so the first still waiting is the one it was inside.
        // Nothing in a terminated worker's reply could say so directly; it was terminated.
        //
        // IT IS MARKED, NOT RETRIED. `apps/web/CLAUDE.md`: never retry automatically. A retry
        // against the page that just killed a worker is the one request guaranteed to kill the
        // next one too, and three of those latch the breaker.
        const culprit = pages.find((p) => tiles[p - 1]?.state === "waiting");
        if (culprit !== undefined) {
          refused = new Set([...refused, culprit]);
          tiles[culprit - 1] = { ...tiles[culprit - 1]!, state: "refused" };
        }
      }
      return true;
    }
  }

  /** Take one page's pixels and put them in its tile, releasing the oldest if the window is full. */
  function accept(page: number, width: number, height: number, pixels: ArrayBuffer) {
    const tile = tiles[page - 1];
    if (!tile) return;

    // THE WINDOW. ADR 0027 §3, and the eviction rule is `strip-schedule.ts`'s rather than a
    // shift from the front: pushing out a tile that is still on screen and then re-requesting
    // it is a loop with no exit, which is what this did.
    while (live.length >= LIVE_THUMBNAIL_WINDOW) {
      const out = evictable(live, wanted, page);
      if (out === null) break;
      live = live.filter((p) => p !== out);
      const stale = tiles[out - 1];
      if (stale?.state === "drawn") {
        tiles[out - 1] = { ...stale, state: releasedState(out, wanted), data: null };
      }
    }

    tiles[page - 1] = {
      ...tile,
      state: "drawn",
      width,
      height,
      // RGBA, which is what `ImageData` takes and what `burrow-engines` produces. The swizzle
      // from PDFium's BGRA happens in Rust on both platforms so the two cannot drift.
      data: new ImageData(new Uint8ClampedArray(pixels), width, height),
    };
    live.push(page);
  }

  /** Put a tile's pixels on its canvas. Re-run whenever the tile's data changes. */
  function paint(node: HTMLCanvasElement, data: ImageData | null) {
    const draw = (image: ImageData | null) => {
      const context = node.getContext("2d");
      if (!context) return;
      if (!image) {
        context.clearRect(0, 0, node.width, node.height);
        return;
      }
      node.width = image.width;
      node.height = image.height;
      context.putImageData(image, 0, 0);
    };
    draw(data);
    return { update: draw };
  }

  onDestroy(() => {
    observer?.disconnect();
    host.dispose();
  });
</script>

{#if tiles.length > 0}
  <section class="strip" aria-label="Page pictures">
    {#if unavailable}
      <p class="strip__notice">
        Page pictures have stopped. The tool still works — choose pages by number.
      </p>
    {/if}
    <ol class="strip__list">
      {#each tiles as tile (tile.page)}
        <li class="strip__tile">
          <div
            class="strip__frame"
            data-page={tile.page}
            data-state={tile.state}
            use:watch={tile.page}
          >
            {#if tile.state === "drawn"}
              <canvas
                class="strip__canvas"
                use:paint={tile.data}
                role="img"
                aria-label={`Page ${tile.page}`}
              ></canvas>
            {:else if tile.state === "refused"}
              <p class="strip__undrawable">Too complex to preview</p>
            {/if}
          </div>
          <span class="strip__number">{tile.page}</span>
        </li>
      {/each}
    </ol>
  </section>
{/if}

<style>
  .strip {
    margin-block-start: 1.5rem;
  }

  .strip__notice {
    color: var(--ink-quiet);
    margin-block-end: 0.75rem;
  }

  .strip__list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
  }

  .strip__tile {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
  }

  /* THE FRAME IS A FIXED BOX, and the picture is fitted inside it by the core. A tile that
     resized when its picture arrived would reflow the whole strip as it filled in, which on a
     long document is the page moving under somebody's cursor. */
  .strip__frame {
    width: 120px;
    height: 160px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--rule);
    background: var(--paper);
  }

  .strip__canvas {
    max-width: 100%;
    max-height: 100%;
  }

  .strip__undrawable {
    color: var(--ink-quiet);
    font-size: var(--step--1);
    margin: 0;
    padding: 0 0.5rem;
    text-align: center;
  }

  .strip__number {
    color: var(--ink-quiet);
    font-size: var(--step--1);
  }
</style>
