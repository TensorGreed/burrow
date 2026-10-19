<script lang="ts">
  // The redaction page's selection picture (#136, S4). HELD, like `RedactTool.svelte`, which is
  // the only thing that mounts it.
  //
  // ONE PAGE, DRAWN BY THE RENDER WORKER WITH ITS ANNOTATIONS (D1, D3), for choosing an area by
  // pointer or by keyboard. It writes the four boxes and draws its outline FROM them
  // (`region-picture.ts`), so the boxes stay what the run sends and the accessible way in. Every
  // failure here is a sentence and the numbers, never a blocked control: the page worked without
  // a picture in S3 and still does.
  //
  // IT IS NOT A PREVIEW. A render does not show what a redaction leaves (ADR 0029), and the page
  // says so beside it. An annotation it draws -- or one it cannot draw, such as a form widget or a
  // hidden one -- is refused by the run if the area reaches its `/Rect` (#372), never kept.
  //
  // #199 AND D1: ONE DOCUMENT PER RENDER WORKER. It ends when the file changes, before a run
  // (`paused`), when the page is left, and when this unmounts. A run's Stop needs nothing of its
  // own here: the worker already ended when the run began. D4: the pixels are decoded content;
  // they are zeroed when the picture lets go of them, and a page arriving for anything the
  // picture no longer shows is zeroed on arrival (`arrival`).
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { ENGINE_PAUSED, LIMITS, RENDER, createToolHost, hostKind } from "./tool-host.js";
  import { createEndGate } from "./strip-lifecycle.js";
  import { zeroPixels } from "./strip-release.js";
  import {
    PICTURE_COPY,
    arrival,
    fieldsBetween,
    fractionAt,
    nudged,
    outlineOf,
    pictureBox,
    shownPictureSize,
    type Fields,
    type Fraction,
  } from "./region-picture.js";

  interface Props {
    /** The document to draw, or `null` for none: no file, or a re-fed result (D9). */
    file: File | null;
    /** True when the chosen file is a re-fed result, which gets no picture (D9). */
    result: boolean;
    /** The page, 1-based, or `null` when the box names none. */
    page: number | null;
    /** That page's display frame from `page_frames`, or `null` when a frame rule refused it. */
    frame: { width: number; height: number } | null;
    /** True while a redaction runs: the render worker is not held through it (D1). */
    paused: boolean;
    /** The four boxes as they are. */
    fields: Fields;
    /** Write the four boxes. */
    onselect: (fields: Fields) => void;
  }

  const { file, result, page, frame, paused, fields, onselect }: Props = $props();

  type Status = "idle" | "drawing" | "drawn" | "mismatch" | "failed" | "unavailable";
  let status = $state<Status>("idle");
  /** The drawn page's pixels: held only while shown. */
  let image = $state<ImageData | null>(null);
  /** Which document and page the picture shows, or is being drawn for. */
  let shownFor: { file: File; page: number } | null = null;
  /** Moves whenever what the picture shows changes, so a late reply is known to be stale. */
  let generation = 0;
  let running = false;
  let hidden = false;
  let space = $state(0);
  let viewportHeight = $state(typeof window === "undefined" ? 0 : window.innerHeight);
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio;

  /** The box the picture is asked for: the space it has, at this screen's density. */
  const box = $derived(pictureBox(space, dpr));
  /** Where the picture goes on screen, reserved before it is drawn (`shownPictureSize`). */
  const shown = $derived(
    frame ? shownPictureSize(frame, box, dpr, Math.round(viewportHeight * 0.75)) : null,
  );

  const host = createToolHost(undefined, RENDER, { paused: () => paused });
  const ending = createEndGate({
    discard: () => host.discardWorker(),
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (handle) => clearTimeout(handle as number),
  });

  /** Let go of the picture's pixels, zeroed first (D4). */
  function release() {
    zeroPixels(image?.data);
    image = null;
  }

  /** End the render worker: now, or once the render in flight settles (`strip-lifecycle.ts`). */
  function endWorker() {
    ending.request(running);
  }

  // WHAT TO DRAW. A different file ends the worker (one document per worker); a different page
  // of the same file only invalidates what is in flight, and is drawn on the same worker.
  $effect(() => {
    const chosen = result ? null : file;
    const wanted = page;
    const shape = frame;
    // THE SAME DOCUMENT AND PAGE IS NOT ASKED FOR AGAIN, whatever became of it: a page that
    // could not be drawn is not retried (apps/web/CLAUDE.md), and one drawn stays drawn.
    const same =
      shownFor !== null && shape !== null && shownFor.file === chosen && shownFor.page === wanted;
    if (same) return;
    generation += 1;
    release();
    if (shownFor && shownFor.file !== chosen) endWorker();
    shownFor = chosen && wanted !== null && shape ? { file: chosen, page: wanted } : null;
    status = shownFor ? "drawing" : "idle";
    if (shownFor && !paused && !hidden) void draw();
  });

  // THE RUN: the worker goes before it, and nothing is drawn during it (D1). A picture already
  // drawn is pixels on this thread and stays; one still to draw is drawn when the run ends.
  $effect(() => {
    if (paused) {
      if (!running) host.discardWorker();
      return;
    }
    if (status === "drawing" && !running && !hidden) void draw();
  });

  async function draw(): Promise<void> {
    if (running || paused || hidden || !shownFor || !frame) return;
    const mine = generation;
    const target = shownFor;
    const shape = frame;
    const asked = box;
    running = true;
    try {
      const h = await host.ensure();
      if (mine !== generation || paused) return;
      const reply = await h.run(
        {
          op: "render",
          blob: target.file,
          pages: [target.page],
          boxWidth: asked.width,
          boxHeight: asked.height,
          annotations: true,
          password: null,
          limits: LIMITS,
        },
        {
          maxDurationMs: LIMITS.maxDurationMs,
          onPage: (got, pixels) => {
            ending.progress();
            const verdict = arrival(
              pixels,
              got,
              { page: target.page, current: mine === generation },
              shape,
              asked,
            );
            if (verdict.kind === "stale") return;
            if (verdict.kind === "mismatch") {
              status = "mismatch";
              return;
            }
            release();
            image = new ImageData(new Uint8ClampedArray(pixels), verdict.width, verdict.height);
            status = "drawn";
          },
        },
      );
      if (mine !== generation) return;
      if (!reply.ok && reply.kind === CANCELLED) return;
      // A WORKER ENDED FOR THE RUN IS NOT A PAGE THAT FAILED: it is drawn after the run.
      if (paused) return;
      if (!reply.ok) {
        // NEVER RETRIED (apps/web/CLAUDE.md): a page that killed a worker kills the next one.
        status = hostKind(reply.kind) === "EngineUnavailable" ? "unavailable" : "failed";
        return;
      }
      if (status === "drawing") status = "failed";
    } catch (error) {
      if (error !== ENGINE_PAUSED) throw error;
    } finally {
      running = false;
      if (paused) {
        host.discardWorker();
        ending.cancel();
      } else {
        ending.settled();
      }
      // WHAT IS WANTED MOVED WHILE THIS RAN: draw it now.
      if (mine !== generation && status === "drawing" && !paused && !hidden) void draw();
    }
  }

  /** Put the pixels on the canvas; cleared and shrunk when they go (D4). */
  function paint(node: HTMLCanvasElement, data: ImageData | null) {
    const put = (next: ImageData | null) => {
      // A CPU CANVAS, for the reason `PageThumbnails.svelte` gives (#107).
      const context = node.getContext("2d", { willReadFrequently: true });
      if (!context) return;
      if (!next) {
        context.clearRect(0, 0, node.width, node.height);
        node.width = 0;
        node.height = 0;
        return;
      }
      node.width = next.width;
      node.height = next.height;
      context.putImageData(next, 0, 0);
    };
    put(data);
    return {
      update: put,
      destroy() {
        put(null);
      },
    };
  }

  // ---- choosing the area ------------------------------------------------------------------

  let surface = $state<HTMLElement | null>(null);
  /** The canvas, whose box -- not the surface's, which has a border -- is the page's. */
  let canvas = $state<HTMLCanvasElement | null>(null);
  let anchor: Fraction | null = null;
  let dragging = $state(false);

  function at(event: PointerEvent): Fraction | null {
    if (!canvas) return null;
    return fractionAt(event.clientX, event.clientY, canvas.getBoundingClientRect());
  }

  function onPointerDown(event: PointerEvent) {
    if (event.button !== 0 || !frame || status !== "drawn") return;
    const start = at(event);
    if (!start) return;
    event.preventDefault();
    anchor = start;
    dragging = true;
    surface?.setPointerCapture(event.pointerId);
    surface?.focus();
  }

  function onPointerMove(event: PointerEvent) {
    if (!dragging || !anchor || !frame) return;
    const here = at(event);
    if (!here) return;
    const next = fieldsBetween(anchor, here, frame);
    if (next) onselect(next);
  }

  function onPointerEnd(event: PointerEvent) {
    if (!dragging) return;
    onPointerMove(event);
    dragging = false;
    anchor = null;
    if (surface?.hasPointerCapture(event.pointerId)) surface.releasePointerCapture(event.pointerId);
  }

  function onKeyDown(event: KeyboardEvent) {
    if (!frame || status !== "drawn") return;
    const next = nudged(fields, frame, {
      key: event.key,
      shiftKey: event.shiftKey,
      altKey: event.altKey,
    });
    if (!next) return;
    event.preventDefault();
    onselect(next);
  }

  const outline = $derived(frame ? outlineOf(fields, frame) : null);

  // LEAVING THE PAGE (#199, D1): the pixels zeroed and the worker ended, as the strip does; a
  // page restored from the back-forward cache draws the picture again, on a new worker.
  $effect(() => {
    const onPageHide = () => {
      hidden = true;
      generation += 1;
      release();
      if (shownFor) status = "drawing";
      endWorker();
    };
    const onPageShow = () => {
      hidden = false;
      if (status === "drawing") void draw();
    };
    globalThis.addEventListener("pagehide", onPageHide);
    globalThis.addEventListener("pageshow", onPageShow);
    return () => {
      globalThis.removeEventListener("pagehide", onPageHide);
      globalThis.removeEventListener("pageshow", onPageShow);
    };
  });

  onDestroy(() => {
    generation += 1;
    ending.cancel();
    release();
    host.dispose();
  });
</script>

<svelte:window bind:innerHeight={viewportHeight} />

<div class="picture" bind:clientWidth={space} data-testid="redact-picture" data-state={status}>
  {#if result}
    <p class="picture__note">{PICTURE_COPY.result}</p>
  {:else if (status === "drawn" || status === "drawing") && shown}
    <p class="picture__note" id="picture-hint">{PICTURE_COPY.hint}</p>
    <div
      class="picture__surface"
      style:width={`${shown.width}px`}
      style:height={`${shown.height}px`}
      class:picture__surface--dragging={dragging}
      bind:this={surface}
      role="application"
      aria-label="The page. Choose the area to remove text from."
      aria-describedby="picture-hint"
      tabindex="0"
      data-testid="redact-picture-surface"
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerEnd}
      onpointercancel={onPointerEnd}
      onkeydown={onKeyDown}
    >
      <canvas
        class="picture__canvas"
        bind:this={canvas}
        use:paint={image}
        data-testid="redact-picture-canvas"
      ></canvas>
      {#if outline}
        <div
          class="picture__outline"
          data-testid="redact-picture-outline"
          style:left={`${outline.x * 100}%`}
          style:top={`${outline.y * 100}%`}
          style:width={`${outline.width * 100}%`}
          style:height={`${outline.height * 100}%`}
        ></div>
      {/if}
    </div>
    <p class="picture__note">{PICTURE_COPY.notPreview}</p>
    {#if status === "drawing"}
      <p class="visually-hidden" role="status">{PICTURE_COPY.drawing}</p>
    {/if}
  {:else if status === "mismatch"}
    <p class="picture__note">{PICTURE_COPY.mismatch}</p>
  {:else if status === "failed"}
    <p class="picture__note">{PICTURE_COPY.failed}</p>
  {:else if status === "unavailable"}
    <p class="picture__note">{PICTURE_COPY.unavailable}</p>
  {/if}
</div>

<style>
  .picture {
    margin-block-start: var(--space-4);
    max-width: 100%;
  }

  .picture__note {
    color: var(--ink-quiet);
    margin-block: var(--space-2);
  }

  /* SHRINK-WRAPPED TO THE PICTURE, so a place in this box is a place on the page: the outline's
     percentages and the pointer's fractions are both of this box. */
  .picture__surface {
    position: relative;
    display: block;
    box-sizing: content-box;
    max-width: 100%;
    line-height: 0;
    /* A RING, NOT A BORDER: a border would make the surface two pixels wider than the space it
       was sized for, which at 390 px is a sideways scroll. */
    box-shadow: 0 0 0 1px var(--rule);
    background: var(--paper);
    cursor: crosshair;
    /* A DRAG CHOOSES AN AREA rather than scrolling the page under the pointer. */
    touch-action: none;
    user-select: none;
  }

  .picture__surface:focus-visible {
    outline: 3px solid var(--focus, currentColor);
    outline-offset: 2px;
  }

  /* SIZED BY ITS SURFACE, whose size is reserved before the pixels arrive. */
  .picture__canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  .picture__outline {
    position: absolute;
    box-sizing: border-box;
    border: 2px solid var(--refuse);
    background: color-mix(in srgb, var(--refuse) 12%, transparent);
    pointer-events: none;
  }
</style>
