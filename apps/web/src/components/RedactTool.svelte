<script lang="ts">
  // The redaction tool's island (#136, S3). HELD: mounted only by `src/held/redact-pdf.astro`,
  // which a production build never routes, so nothing here reaches a visitor until S9.
  //
  // WHAT IT DOES, AND ONLY THAT: a file, every page's frame (`page_frames`), one page and one
  // region typed in points (D2: one region per run), a run through REDACTION's own worker, a
  // cancel, and a download. Pictures and pointer selection are S4; "redact another area of this
  // result" is S6b.
  //
  // THE WORKER IS REDACTION'S, NOT THE BASE ONE. The redaction bundle is fetched on first use and
  // by nothing else (ADR 0029's 2026-09-21 amendment), through the same `createToolHost` every
  // island uses, so its integrity pin, origin guard and memoised build are the shared ones.
  // `BUNDLE_REDACT_WORKER` is null in a production build; this island is never in one.
  import { onDestroy, tick } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { BUNDLE_REDACT_WORKER } from "../generated/engines.js";
  import { ORIGINAL_MARK } from "../redact-copy.js";
  import {
    messageFor,
    messageForRule,
    resultNotes,
    type RedactMessage,
  } from "./redact-messages.js";
  import { framesOf, readRegion, shownSize } from "./redact-region.js";
  import {
    LIMITS,
    ORIGIN_MISMATCH,
    createToolHost,
    hostKind,
    originMismatchNotice,
  } from "./tool-host.js";
  import { createDelivery, type Handout } from "./tool-delivery.js";

  type Frame = { width: number; height: number; refused: string };

  let file = $state<File | null>(null);
  /** Every page's frame, once read. `null` while reading, an empty list if it could not be. */
  let frames = $state<Frame[] | null>(null);
  let unreadable = $state(false);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<RedactMessage | null>(null);
  let result = $state<Handout | null>(null);
  /** What the run reported that §7 has a paragraph for. */
  let retainedFonts = $state(0);
  let droppedCarriedText = $state(0);
  let announcement = $state("");

  /** The page, 1-based, as typed. */
  let pageText = $state("1");
  let left = $state("");
  let top = $state("");
  let width = $state("");
  let height = $state("");

  const host = BUNDLE_REDACT_WORKER ? createToolHost(undefined, BUNDLE_REDACT_WORKER) : null;
  const delivery = createDelivery();
  let engineStarted = $state(false);

  const pageCount = $derived(frames?.length ?? 0);

  /** The chosen page, 1-based, or null when the box does not name one. */
  const page = $derived.by(() => {
    const trimmed = pageText.trim();
    if (!/^\d{1,9}$/.test(trimmed)) return null;
    const n = Number(trimmed);
    return n >= 1 && n <= pageCount ? n : null;
  });

  const frame = $derived(page !== null && frames ? (frames[page - 1] ?? null) : null);

  /** A page refused by a frame rule refuses every region on it, before anything is typed. */
  const pageRefusal = $derived(frame && frame.refused ? messageForRule(frame.refused) : null);

  const region = $derived.by(() => {
    if (!frame || frame.refused) return null;
    if ([left, top, width, height].every((text) => text.trim() === "")) return null;
    return readRegion({ left, top, width, height }, frame);
  });

  const regionProblem = $derived(region !== null && !region.ok ? region.problem : "");
  const pageProblem = $derived(
    frames !== null && pageCount > 0 && page === null
      ? `Type a page number from 1 to ${pageCount}.`
      : "",
  );

  /** The request a visible result was produced from; a link for an older one is hidden. */
  const request = $derived(`${file?.name ?? ""}|${pageText}|${left}|${top}|${width}|${height}`);
  const stale = $derived(result !== null && result.signature !== request);

  const canRun = $derived(
    file !== null && phase !== "working" && region !== null && region.ok && page !== null,
  );
  const preparing = $derived(!engineStarted && file !== null && frames === null && !unreadable);

  async function choose(files: FileList | File[]) {
    const [chosen] = Array.from(files);
    if (!chosen) return;
    delivery.invalidate();
    clearResult();
    phase = "idle";
    notice = null;
    file = chosen;
    frames = null;
    unreadable = false;
    // A NEW FILE STARTS FROM PAGE 1 AND AN EMPTY AREA, as `SplitTool.svelte` resets its cuts: a
    // page number or an area typed for the last document means nothing for this one.
    pageText = "1";
    left = "";
    top = "";
    width = "";
    height = "";
    announce(`${chosen.name} chosen.`);
    await readFrames(chosen);
  }

  async function readFrames(chosen: File) {
    if (!host) {
      unreadable = true;
      notice = messageFor({ kind: "Internal" });
      return;
    }
    const watching = delivery.watch();
    try {
      const h = await host.ensure();
      const raw = await h.run(
        { op: "page_frames", blob: chosen, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      // ON THE FIRST REPLY, not from `hasWorker()`: redaction's worker recycles after every reply
      // (#199), so a live worker is never held between operations and the question means nothing.
      engineStarted = true;
      if (!watching.live()) return;
      if (!raw.ok && raw.kind === CANCELLED) return;
      if (!raw.ok) {
        unreadable = true;
        notice = messageFor({ ...raw, kind: hostKind(raw.kind) });
        announce(notice.title);
        return;
      }
      const read = framesOf(raw.frames, raw.frameRefusals);
      if (read === null || read.length === 0) {
        unreadable = true;
        notice = messageFor({ kind: "Internal" });
        return;
      }
      frames = read;
      announce(`${read.length} ${read.length === 1 ? "page" : "pages"}.`);
    } catch (error) {
      if (!watching.live()) return;
      unreadable = true;
      notice =
        error === ORIGIN_MISMATCH ? originMismatchNotice() : messageFor({ kind: "Internal" });
    }
  }

  async function run() {
    if (!canRun || !file || !host || page === null || region === null || !region.ok) return;
    const target = page;
    const area = region.region;
    const run = delivery.begin({ name: suggestedName(), signature: request });

    clearResult();
    notice = null;
    phase = "working";
    announce("Removing the text in the area.");

    try {
      const h = await host.ensure();
      if (!run.live()) {
        phase = "idle";
        return;
      }
      // ONE PAGE, AND IT IS THE ONLY ONE COVERED (D2). A font is cut only when every page using
      // it is covered, so a font another page shares is left whole and reported.
      const reply = await h.run(
        {
          op: "redact",
          blob: file,
          page: target,
          covered: [target],
          region: area,
          password: null,
          limits: LIMITS,
        },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      if (!run.live()) return;
      if (!reply.ok) {
        if (reply.kind === CANCELLED) {
          phase = "idle";
          return;
        }
        notice = messageFor({ ...reply, kind: hostKind(reply.kind) });
        phase = "idle";
        announce(notice.title);
        return;
      }
      if (!reply.output) {
        notice = messageFor({ kind: "Internal" });
        phase = "idle";
        return;
      }
      const handout = run.hand(reply.output);
      if (!handout) {
        phase = "idle";
        return;
      }
      retainedFonts = reply.retainedFonts ?? 0;
      droppedCarriedText = reply.droppedCarriedText ?? 0;
      result = handout;
      phase = "done";
      announce("Done. The new file is ready to download.");
    } catch (error) {
      if (!run.live()) return;
      notice =
        error === ORIGIN_MISMATCH ? originMismatchNotice() : messageFor({ kind: "Internal" });
      phase = "idle";
    } finally {
      await tick();
    }
  }

  function suggestedName(): string {
    const base = file?.name.replace(/\.pdf$/i, "") ?? "document";
    return `${base}-redacted.pdf`;
  }

  /** Stop the run in flight: only terminating the worker stops an engine call (ADR 0007). */
  function cancel() {
    delivery.invalidate();
    host?.discardWorker();
    phase = "idle";
    notice = null;
    announce("Stopped.");
  }

  function startAgain() {
    host?.reset();
    notice = null;
    announce("Ready to try again.");
  }

  function clearResult() {
    delivery.release(result);
    result = null;
    retainedFonts = 0;
    droppedCarriedText = 0;
  }

  function announce(text: string) {
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  onDestroy(() => {
    // INVALIDATE FIRST, so a reply landing during teardown cannot hand out a URL after the
    // release below (L4).
    delivery.invalidate();
    clearResult();
    host?.dispose();
  });

  let dragging = $state(false);

  function onDrop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    const dropped = event.dataTransfer?.files;
    if (dropped && dropped.length > 0) void choose(dropped);
  }
</script>

<section class="tool" aria-labelledby="tool-heading">
  <h2 id="tool-heading" class="visually-hidden">Remove text from an area of a page</h2>

  <label
    class="drop"
    class:drop--over={dragging}
    ondragover={(e) => {
      e.preventDefault();
      dragging = true;
    }}
    ondragleave={() => (dragging = false)}
    ondrop={onDrop}
  >
    <input
      type="file"
      accept="application/pdf,.pdf"
      class="visually-hidden"
      data-testid="redact-file"
      onchange={(e) => {
        const input = e.currentTarget;
        if (input.files) void choose(input.files);
        input.value = "";
      }}
    />
    <span class="drop__text">
      {file ? "Choose a different PDF" : "Drop a PDF here, or choose one"}
    </span>
  </label>

  {#if file}
    <p class="chosen">
      <span class="chosen__name">{file.name}</span>
      <span class="chosen__pages">
        {#if unreadable}
          could not be read
        {:else if frames === null}
          reading…
        {:else}
          {pageCount}
          {pageCount === 1 ? "page" : "pages"}
        {/if}
      </span>
    </p>
  {/if}

  {#if preparing}
    <p class="preparing" role="status">
      Starting the PDF engine — about 585 KB. Nothing has been sent anywhere.
    </p>
  {/if}

  {#if frames !== null && pageCount > 0}
    <fieldset class="choice">
      <legend>Which page</legend>
      <input
        type="text"
        class="choice__number"
        inputmode="numeric"
        aria-label={`Page number, from 1 to ${pageCount}`}
        aria-describedby={pageProblem ? "page-problem" : undefined}
        aria-invalid={pageProblem ? "true" : undefined}
        data-testid="redact-page"
        bind:value={pageText}
      />
      {#if pageProblem}
        <p class="choice__problem" id="page-problem">{pageProblem}</p>
      {/if}
      {#if frame && !frame.refused}
        <p
          class="choice__hint"
          data-testid="redact-frame"
          data-width={frame.width}
          data-height={frame.height}
        >
          This page is {shownSize(frame)} points, measured from its top-left corner as it is shown.
        </p>
      {/if}
    </fieldset>

    {#if pageRefusal}
      <p class="notice" role="alert" data-testid="redact-page-refused">
        <strong>{pageRefusal.title}</strong>
        {pageRefusal.next}
        {#if pageRefusal.rule}<span class="notice__rule">({pageRefusal.rule})</span>{/if}
      </p>
    {:else if frame}
      <fieldset class="choice">
        <legend>The area, in points</legend>
        <div class="area">
          <label
            >From the left
            <input
              type="text"
              inputmode="decimal"
              aria-describedby={regionProblem ? "region-problem" : undefined}
              aria-invalid={regionProblem ? "true" : undefined}
              bind:value={left}
              data-testid="redact-left"
            /></label
          >
          <label
            >From the top
            <input
              type="text"
              inputmode="decimal"
              aria-describedby={regionProblem ? "region-problem" : undefined}
              aria-invalid={regionProblem ? "true" : undefined}
              bind:value={top}
              data-testid="redact-top"
            /></label
          >
          <label
            >Width
            <input
              type="text"
              inputmode="decimal"
              aria-describedby={regionProblem ? "region-problem" : undefined}
              aria-invalid={regionProblem ? "true" : undefined}
              bind:value={width}
              data-testid="redact-width"
            /></label
          >
          <label
            >Height
            <input
              type="text"
              inputmode="decimal"
              aria-describedby={regionProblem ? "region-problem" : undefined}
              aria-invalid={regionProblem ? "true" : undefined}
              bind:value={height}
              data-testid="redact-height"
            /></label
          >
        </div>
        {#if regionProblem}
          <p class="choice__problem" id="region-problem">{regionProblem}</p>
        {/if}
      </fieldset>
    {/if}
  {/if}

  {#if notice}
    <p class="notice" role="alert" data-testid="redact-notice">
      <strong>{notice.title}</strong>
      {notice.next}
      {#if notice.rule}<span class="notice__rule">({notice.rule})</span>{/if}
      {#if !notice.retryable}
        <button type="button" class="notice__action" onclick={startAgain}>Start again</button>
      {/if}
    </p>
  {/if}

  <div class="actions">
    <button
      type="button"
      class="action--primary"
      disabled={!canRun}
      onclick={run}
      data-testid="redact-run">Remove the text</button
    >
    {#if phase === "working"}
      <button type="button" onclick={cancel} data-testid="redact-stop">Stop</button>
      <p class="working" role="status">Working. This does not report progress.</p>
    {/if}
  </div>

  {#if phase === "done" && result && !stale}
    <div class="result">
      <p>
        <a class="download" href={result.url} download={result.name} data-testid="redact-download"
          >Download {result.name}</a
        >
      </p>
      <p>{ORIGINAL_MARK}</p>
      {#each resultNotes(retainedFonts, droppedCarriedText) as note (note)}
        <p data-testid="redact-result-note">{note}</p>
      {/each}
    </div>
  {/if}

  <p class="visually-hidden" role="status" aria-live="polite">{announcement}</p>
</section>

<style>
  .tool {
    margin-block-start: var(--space-6);
    max-width: var(--track-readout);
  }

  .drop {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 7rem;
    border: 2px dashed var(--edge);
    border-radius: var(--radius);
    padding: var(--space-5);
    text-align: center;
    cursor: pointer;
  }

  .drop--over {
    border-style: solid;
  }

  .chosen {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    border-block-end: 1px solid var(--rule);
    padding-block: var(--space-2);
    margin-block-start: var(--space-4);
  }

  .chosen__pages,
  .choice__hint,
  .preparing,
  .working,
  .notice__rule {
    color: var(--ink-quiet);
    font-variant-numeric: tabular-nums;
  }

  .choice {
    border: 0;
    padding: 0;
    margin-block-start: var(--space-4);
  }

  .choice legend {
    color: var(--ink-quiet);
    padding: 0;
  }

  .area {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr));
    gap: var(--space-3);
    margin-block-start: var(--space-2);
  }

  .area label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .area input,
  .choice__number {
    padding: var(--space-2);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    font: inherit;
    font-variant-numeric: tabular-nums;
  }

  .choice__problem,
  .notice {
    color: var(--refuse);
    margin-block-start: var(--space-3);
  }

  .notice__action {
    margin-inline-start: var(--space-2);
  }

  .actions {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    margin-block-start: var(--space-5);
  }

  .actions button {
    font-size: var(--step-0);
    padding: var(--space-3) var(--space-5);
  }

  .result {
    margin-block-start: var(--space-4);
  }

  .download {
    font-weight: var(--weight-strong);
  }
</style>
