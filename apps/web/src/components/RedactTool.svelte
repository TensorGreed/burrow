<script lang="ts">
  // The redaction tool's island (#136, S3). HELD: mounted only by `src/held/redact-pdf.astro`,
  // which a production build never routes, so nothing here reaches a visitor until S9.
  //
  // WHAT IT DOES, AND ONLY THAT: a file, every page's frame (`page_frames`), one page and one
  // region typed in points (D2: one region per run), a run through REDACTION's own worker, a
  // cancel, a download, and "redact another area of this result" (S6b), which runs again on the
  // output. Pictures and pointer selection are S4.
  //
  // THE WORKER IS REDACTION'S, NOT THE BASE ONE. The redaction bundle is fetched on first use and
  // by nothing else (ADR 0029's 2026-09-21 amendment), through the same `createToolHost` every
  // island uses, so its integrity pin, origin guard and memoised build are the shared ones.
  // `BUNDLE_REDACT_WORKER` is null in a production build; this island is never in one.
  import { onDestroy, tick } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { redactWithSecondReading } from "../host/second-reading.js";
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
    freshReadingHost,
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
  /**
   * The finished run's document, for "redact another area of this result" (S6b, D2's re-feed).
   *
   * The same Blob the download link points at, held here rather than read back from the link:
   * one region per run, so a second area is a second run on the first one's output.
   */
  let output: Blob | null = null;
  /**
   * The result a re-fed run started from, kept downloadable until that run succeeds, the person
   * chooses a different file, or the page goes away -- then released (#199).
   */
  let earlier = $state<Handout | null>(null);
  /** Whether the chosen file is a re-fed result (S6b) rather than one the person chose. */
  let reFed = $state(false);
  /** The name of the file the person chose, which every result in its chain is named after. */
  let baseName = "document";
  /**
   * What the runs on this chain reported that §7 has a paragraph for: the most any run in it
   * counted, so a note stays while the chain lasts and goes with a genuinely new file (L2). A
   * removal the first run made is in every later result, and so is what it disclosed.
   */
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

  /**
   * Take a file: one the person chose, or (`fromResult`) the last result re-fed by `another()`.
   *
   * A file the person chose starts a new chain: every earlier result is released, the §7 notes
   * reset, and results are named after it. A re-fed result keeps the chain.
   */
  async function choose(files: FileList | File[], fromResult = false) {
    const [chosen] = Array.from(files);
    if (!chosen) return;
    delivery.invalidate();
    clearResult();
    if (!fromResult) {
      releaseEarlier();
      retainedFonts = 0;
      droppedCarriedText = 0;
      baseName = chosen.name.replace(/\.pdf$/i, "") || "document";
    }
    reFed = fromResult;
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
      //
      // WITH ITS SECOND READING (#206's web half): a fresh render worker reads the redacted page
      // again and the redaction worker judges it; the document reaches this page only after an
      // agreeing verdict. Without the reading the redaction worker refuses every redaction.
      const reply = await redactWithSecondReading(
        h,
        () => freshReadingHost(),
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
      output = reply.output;
      const retained = reply.retainedFonts ?? 0;
      const dropped = reply.droppedCarriedText ?? 0;
      retainedFonts = reFed ? Math.max(retainedFonts, retained) : retained;
      droppedCarriedText = reFed ? Math.max(droppedCarriedText, dropped) : dropped;
      // THE NEW RESULT REPLACES THE ONE IT STARTED FROM, which is released only now that this
      // one exists.
      releaseEarlier();
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

  /** Named after the file the person chose, however many areas its chain has redacted. */
  function suggestedName(): string {
    return `${baseName}-redacted.pdf`;
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

  /**
   * Start a second run on the first one's output (S6b). The output becomes the chosen file, so
   * its frames are read again and a new area is typed: nothing from the first run's area carries
   * over, because the person is choosing a different one.
   */
  function another() {
    if (!output || !result) return;
    const next = new File([output], result.name, { type: "application/pdf" });
    // THE RESULT STAYS DOWNLOADABLE while the next area is chosen and run.
    earlier = result;
    result = null;
    output = null;
    void choose([next], true);
  }

  function clearResult() {
    delivery.release(result);
    result = null;
    output = null;
  }

  function releaseEarlier() {
    delivery.release(earlier);
    earlier = null;
  }

  /** Release every result the page holds (#199): on page leave and on unmount. */
  function releaseAll() {
    delivery.invalidate();
    clearResult();
    releaseEarlier();
  }

  function announce(text: string) {
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  // LEAVING THE PAGE RELEASES EVERY RESULT (#199), as the render worker is terminated on
  // `pagehide` (D1): a result left behind in a bfcached page is a copy of the document nobody
  // asked to keep.
  $effect(() => {
    const onPageHide = () => releaseAll();
    // A PAGE RESTORED FROM THE BACK-FORWARD CACHE after leaving mid-run would still say "Working"
    // over a run whose reply `releaseAll` already invalidated; it is back to idle, with nothing
    // to download, which is what it now is.
    const onPageShow = (event: PageTransitionEvent) => {
      if (!event.persisted) return;
      if (phase === "working") {
        // AND THE WORKER IT WAS RUNNING ON IS GONE, so "Stopped." is true: a frozen worker could
        // otherwise still be holding the document when the page comes back.
        host?.discardWorker();
        phase = "idle";
        announce("Stopped.");
      }
    };
    globalThis.addEventListener("pagehide", onPageHide);
    globalThis.addEventListener("pageshow", onPageShow);
    return () => {
      globalThis.removeEventListener("pagehide", onPageHide);
      globalThis.removeEventListener("pageshow", onPageShow);
    };
  });

  onDestroy(() => {
    // INVALIDATE FIRST (inside `releaseAll`), so a reply landing during teardown cannot hand out
    // a URL after the release (L4).
    releaseAll();
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

  {#if earlier && !(phase === "done" && result && !stale)}
    <p class="result" data-testid="redact-earlier">
      <!-- NOT LABELLED WITH ITS FILE NAME (L-B): the name is the chain's, which a finished second
           run would share, and after a refused or stopped run a link saying it would read as the
           result with both areas removed. -->
      <a class="download" href={earlier.url} download={earlier.name}
        >Download the last result, with only the earlier areas removed</a
      >
    </p>
  {/if}

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
      <!-- ONE AREA PER RUN (D2): a second area is a run on this result, not on the file chosen
           first, so the first area's removal is kept. -->
      <p>
        <button type="button" onclick={another} data-testid="redact-another"
          >Redact another area of this result</button
        >
      </p>
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
