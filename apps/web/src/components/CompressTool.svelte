<script lang="ts">
  // The compress tool. One island, and the only JavaScript this page ships.
  //
  // Built on the parts `/merge-pdf` established rather than re-deciding them: the worker host
  // is IMPORTED (not fetched from `/host/`), the engines start on first use, every typed error
  // becomes a sentence in a pure function beside this file, a cancelled operation's reply is
  // ignored by generation rather than suppressed, and the result is a link a person activates.
  // `apps/web/CLAUDE.md`'s "What a tool page is made of" is the list.
  //
  // WHAT IS DIFFERENT HERE, and it is one thing that changes the shape of the whole component:
  //
  //   **A SUCCESSFUL RESULT CAN BE "NOTHING HAPPENED", AND THAT IS NOT A FAILURE.**
  //
  // Every other tool has one successful answer: here is your document. Compression has three,
  // and two of them return a person to where they started. Spike 0005 measured 85.6% on a form
  // and **0.15% on a scan** -- and a scan is the modal thing somebody brings to a page called
  // "Compress PDF".
  //
  // So `result` is not a `Handout | null` the way the other islands have it. It is a `Result`
  // from `compress-messages.ts` -- a kind, a headline, an explanation -- and the handout hangs
  // off it when there is one. The rendering below treats all three as results, in ordinary
  // ink: `--refuse` is reserved for refusals and limits (the design brief), and a document that
  // was already efficient is neither.
  //
  // There is also NO SELECTION. Compression takes no page list, no angle, no cut: the whole
  // request is the document. That removes the `request` signature's controls, but not the
  // signature -- the file can still change under an in-flight operation.
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { type Message, type Result, messageFor, resultFor } from "./compress-messages.js";
  import {
    LIMITS,
    ORIGIN_MISMATCH,
    createToolHost,
    hostKind,
    originMismatchNotice,
  } from "./tool-host.js";
  import { createDelivery, type Handout } from "./tool-delivery.js";

  let file = $state<File | null>(null);
  /** Pages, once counted. `null` while counting, `-1` if it could not be read. */
  let pageCount = $state<number | null>(null);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<Message | null>(null);
  /** What compression found. Present on every successful outcome, including the empty ones. */
  let outcome = $state<Result | null>(null);
  /** The document, when there is one to offer. `null` on the unchanged outcome, by design. */
  let handout = $state<Handout | null>(null);
  let announcement = $state("");

  // ---------------------------------------------------------------- the worker

  const host = createToolHost();
  const delivery = createDelivery();

  /**
   * Whether the engines have ever finished starting on this page.
   *
   * Engines load on first use (ADR 0018), so the first file waits for the engine payload --
   * about 420 KB over the wire since spike 0004. Saying nothing while it happens is the page
   * being silent about the one thing the person wants to know.
   */
  let engineStarted = $state(false);
  const preparing = $derived(!engineStarted && file !== null && pageCount === null);

  // ---------------------------------------------------------------- choosing a file

  async function choose(files: FileList | File[]) {
    const [chosen] = Array.from(files);
    if (!chosen) return;

    // ANYTHING IN FLIGHT IS NOW ABOUT A DOCUMENT NOBODY HAS CHOSEN, so it is invalidated
    // before anything else changes. This is a disclosure path rather than a tidiness one --
    // #69, found by security review on merge and repeated on reorder: without it a finished
    // operation's bytes could be offered under the newly-chosen file's name, with nothing on
    // screen distinguishing them.
    delivery.invalidate();
    clearResult();
    phase = "idle";
    notice = null;
    file = chosen;
    pageCount = null;
    announce(`${chosen.name} chosen.`);
    await count(chosen);
  }

  async function count(chosen: File) {
    // The generation this count belongs to. A count still pending for the PREVIOUS file
    // resolves after `choose()` has swapped `file`, and without this it would write the old
    // document's page count beside the new document's name.
    const watching = delivery.watch();
    try {
      const h = await host.ensure();
      const raw = await h.run(
        { op: "page_count", blob: chosen, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      // ASK THE HOST whether a worker exists rather than inferring it from a reply: the host
      // answers `Internal` when `ensureWorker()` fails and `EngineUnavailable` when the
      // breaker has latched, both without a worker ever existing.
      engineStarted = h.hasWorker();

      if (!watching.live()) return;
      if (!raw.ok && raw.kind === CANCELLED) return;
      const reply = raw.ok ? raw : { ...raw, kind: hostKind(raw.kind) };

      if (reply.ok) {
        pageCount = reply.pages;
        announce(`${reply.pages} pages.`);
        return;
      }

      pageCount = -1;
      notice = messageFor(reply);
      announce(notice.title);
    } catch (error) {
      if (!watching.live()) return;
      // BY IDENTITY, NOT BY TEXT. ADR 0009 forbids reading anything out of a thrown value, so
      // `ORIGIN_MISMATCH` is compared rather than inspected -- a unique symbol has no text to
      // read. Without it the page says "Something inside burrow failed" beside a banner
      // explaining exactly what is wrong.
      if (error === ORIGIN_MISMATCH) {
        pageCount = -1;
        notice = originMismatchNotice();
        announce(notice.title);
        return;
      }
      pageCount = -1;
      notice = messageFor({ kind: "Internal" });
    }
  }

  // ---------------------------------------------------------------- compressing

  /**
   * The request the visible result was produced from.
   *
   * Only the file, because compression has no controls -- but the signature is still needed:
   * a person can choose a different document while one is in flight, and a result labelled for
   * the previous file is the #69 class.
   */
  const request = $derived(file?.name ?? "");
  /**
   * Whether a visible handout belongs to a document other than the chosen one.
   *
   * **THE GENERATION COUNTER IS WHAT CLOSES THE #69 CLASS HERE, NOT THIS.** Kept honest rather
   * than left implied: `choose()` bumps the generation and nulls `handout` before `file`
   * changes, so on this page the signature can never actually disagree -- and the `unchanged`
   * outcome has no handout at all, so it is `false` by construction there while still showing
   * the previous document's two byte counts. What gates that outcome is the generation check
   * and nothing else.
   *
   * It stays as a second, cheap gate on the one thing a person would be handed. Nobody should
   * read its presence as licence to weaken the generation bump in `choose()`, which is the
   * guard doing the work.
   */
  const stale = $derived(handout !== null && handout.signature !== request);

  const canCompress = $derived(
    file !== null && typeof pageCount === "number" && pageCount > 0 && phase !== "working",
  );

  async function compress() {
    if (!canCompress || !file) return;

    // THE NAME AND THE SIGNATURE, CAPTURED BEFORE THE FIRST AWAIT and not readable after it at
    // all: `run.hand()` takes bytes and nothing else. Reading `request` back after the awaits
    // would record what the page says when the reply lands rather than what was posted.
    const run = delivery.begin({ name: suggestedName(), signature: request });

    clearResult();
    notice = null;
    phase = "working";
    announce("Compressing.");

    try {
      const h = await host.ensure();

      // CANCELLED BEFORE THERE WAS ANYTHING TO CANCEL: during the first operation on a page
      // the host is still being built, so Stop is a no-op and the work would be posted a
      // moment after the person stopped it.
      if (!run.live()) {
        phase = "idle";
        return;
      }

      const reply = await h.run(
        { op: "compress", blob: file, password: null, limits: LIMITS },
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

      // A SUCCESS WITH NO DOCUMENT IS AN ANSWER, NOT AN ERROR -- and this is the line that
      // makes the whole page work. `reply.output` is null when the re-encoding was no smaller
      // (ADR 0025 §3), and the other four islands treat a missing output as `Internal`.
      // Doing that here would tell somebody their file was broken when it was merely already
      // efficient, on the outcome roughly 8.7% of documents produce.
      const found = resultFor({
        originalBytes: reply.originalBytes,
        producedBytes: reply.producedBytes,
        hasDocument: Boolean(reply.output),
      });

      if (reply.output) {
        // An object URL over the Blob, created only if this run is still current -- an
        // unrevoked one holds the bytes for the life of the page. The bytes stay in the Blob;
        // the page never reads them into its own heap.
        const handed = run.hand(reply.output);
        if (!handed) {
          phase = "idle";
          return;
        }
        handout = handed;
      }

      outcome = found;
      phase = "done";
      announce(found.title);
    } catch (error) {
      if (!run.live()) return;
      notice =
        error === ORIGIN_MISMATCH ? originMismatchNotice() : messageFor({ kind: "Internal" });
      phase = "idle";
    }
  }

  function suggestedName(): string {
    const base = file?.name.replace(/\.pdf$/i, "") ?? "document";
    return `${base}-compressed.pdf`;
  }

  /**
   * Stop the operation in flight.
   *
   * `discardWorker()` terminates the worker, which is the only thing that can stop work inside
   * an engine call -- no engine here offers a cancellation hook (ADR 0007). That matters more
   * on this page than on the others: compression is a SINGLE engine call and the most
   * expensive one burrow makes, so there is no page boundary for a deadline to land on.
   */
  function cancel() {
    delivery.invalidate();
    host.discardWorker();
    phase = "idle";
    notice = null;
    announce("Stopped.");
  }

  /** The deliberate gesture that closes the circuit breaker (ADR 0015 §3). */
  function startAgain() {
    host.reset();
    notice = null;
    announce("Ready to try again.");
  }

  function clearResult() {
    delivery.release(handout);
    handout = null;
    outcome = null;
  }

  function announce(text: string) {
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  onDestroy(() => {
    clearResult();
    host.dispose();
  });

  // ---------------------------------------------------------------- drag and drop

  let dragging = $state(false);

  function onDrop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    const dropped = event.dataTransfer?.files;
    if (dropped && dropped.length > 0) void choose(dropped);
  }
</script>

<section class="tool" aria-labelledby="tool-heading">
  <h2 id="tool-heading" class="visually-hidden">Compress your PDF</h2>

  <!-- THE DROP ZONE HAS A REAL FILE INPUT INSIDE IT, not a click handler that opens one: a
       label wrapping an input is reachable by keyboard and by a screen reader without
       anything being simulated. -->
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
        {#if pageCount === null}
          counting…
        {:else if pageCount < 0}
          could not be read
        {:else}
          {pageCount}
          {pageCount === 1 ? "page" : "pages"}
        {/if}
      </span>
    </p>
  {/if}

  {#if preparing}
    <p class="preparing" role="status">
      Starting the PDF engine — about 420 KB, downloaded once. Nothing has been sent anywhere.
    </p>
  {/if}

  {#if notice}
    <p class="notice" role="alert">
      <strong>{notice.title}</strong>
      {notice.next}
      {#if !notice.retryable}
        <button type="button" class="notice__action" onclick={startAgain}>Start again</button>
      {/if}
    </p>
  {/if}

  <div class="actions">
    <button type="button" class="action--primary" disabled={!canCompress} onclick={compress}
      >Compress</button
    >
    {#if phase === "working"}
      <button type="button" onclick={cancel}>Stop</button>
      <!-- NO PROGRESS BAR. Compression is ONE engine call inside a worker and reports nothing
           until it finishes, so a bar would be animating against nothing. -->
      <p class="working" role="status">Compressing. This does not report progress.</p>
    {/if}
  </div>

  <!-- ALL THREE OUTCOMES RENDER HERE, in ordinary ink.
       `--refuse` is reserved for refusals and limits (the design brief), and a document that
       was already efficiently stored is neither. Presenting it in the refusal colour would be
       the interface calling a fact about somebody's file an error. -->
  {#if phase === "done" && outcome && !stale}
    <!-- NO `role="status"` HERE, matching the other four islands. `announce(found.title)` has
         already put the headline through the visually-hidden live region at the foot of this
         component, so a live region here would read the same sentence a second time -- and
         this one contains the download link, which a live region should not be announcing at
         all. The preparing and working lines keep theirs: those have no announcer. -->
    <div class="outcome">
      <p class="outcome__title"><strong>{outcome.title}</strong></p>
      {#if outcome.detail}
        <p class="outcome__detail">{outcome.detail}</p>
      {/if}
      {#if handout}
        <!-- A LINK A PERSON ACTIVATES, not a download that starts itself. Offered on the
             negligible outcome too: the page reports what it found and does not decide for
             somebody whether 0.15% is worth having. -->
        <p class="outcome__download">
          <a class="download" href={handout.url} download={handout.name}>
            Download {handout.name}
          </a>
        </p>
      {/if}
    </div>
  {/if}

  <!-- NOT `.visually-hidden`, AND THAT IS A MEASUREMENT RATHER THAN AN OVERSIGHT.

       `.visually-hidden` was defined in `MergeTool.svelte` only, so in this island the class
       did nothing and this region has always rendered visibly. Moving the rule into
       `base.css` (ADR 0028) made it real -- and made WebKit fail on this page: a split stalled
       at "Part 1 of 4" until its 45-second budget expired, and one run ended in "Target page,
       context or browser has been closed". It reproduced about once per suite, only under the
       parallel load of a full run, never in isolation, and it went away when this one class
       was removed and came back when it was restored. Both `clip-path: inset(50%)` and the
       older `clip: rect(0 0 0 0)` do it, so it is not the clipping technique.

       What is different here from the merge tool, which has hidden this region for months: a
       split announces once per part, so this element updates repeatedly while the operation
       runs. That is the shape that had never been exercised.

       So the region keeps the visibility it has had all along -- no regression against what
       ships today -- and it is hidden once the WebKit behaviour is understood. Issue #107. -->
  <p role="status" aria-live="polite">{announcement}</p>
</section>

<style>
  .tool {
    margin-block-start: var(--space-6);
    max-width: var(--track-readout);
  }

  /* A surface you can act on, so `--edge` rather than `--rule`. It is the first thing the
     page asks you to do, so it is a real target rather than a hairline: 2px and 7rem tall,
     matching `MergeTool`, which was the only tool that had it. */
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

  .download {
    /* INK, NOT `--signal`, as on all four other tool pages -- `apps/web/CLAUDE.md`: "Links and
       buttons are ink, not `--signal`. This is the costliest rule in the system and the point
       of it." THE CASE IS SHARPEST ON THIS PAGE: the headline above is the one genuinely
       measured number any tool page shows ("4.0 MB -> 2.8 MB. 30% smaller."), and if the link
       beneath it were the signal colour, the colour would mean "interactive" here and
       "measured" there, and the measurement would stop standing out. The headline is ordinary
       ink for the same reason the outcome block is: `--signal` is a machine-state readout, and
       these three outcomes are prose about a file. */
    font-weight: var(--weight-strong);
    font-variant-numeric: tabular-nums;
  }

  .drop__text {
    color: var(--ink);
  }

  .chosen {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    border-block-end: 1px solid var(--rule);
    padding-block: var(--space-2);
    margin-block-start: var(--space-4);
  }

  .chosen__pages {
    color: var(--ink-quiet);
    font-variant-numeric: tabular-nums;
  }

  .preparing,
  .working {
    color: var(--ink-quiet);
    margin-block-start: var(--space-3);
  }

  .notice {
    margin-block-start: var(--space-4);
    color: var(--refuse);
  }

  .notice__action {
    margin-inline-start: var(--space-2);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
    margin-block-start: var(--space-4);
  }

  /* Every control in this row is the same size, so the row reads as one set rather than as
     a primary button with two chips beside it. The primary's fill and size come from
     `.action--primary` in `base.css`. */
  .actions button {
    font-size: var(--step-0);
    padding: var(--space-3) var(--space-5);
  }

  .outcome {
    margin-block-start: var(--space-5);
    border-block-start: 1px solid var(--rule);
    padding-block-start: var(--space-4);
  }

  /* TABULAR NUMERALS, because the headline is two sizes and a percentage side by side and a
     measurement that shifts width as it updates looks unreliable. */
  .outcome__title {
    font-variant-numeric: tabular-nums;
  }

  .outcome__detail {
    color: var(--ink-quiet);
    margin-block-start: var(--space-2);
    max-width: var(--track-read);
  }

  .outcome__download {
    margin-block-start: var(--space-4);
  }
</style>
