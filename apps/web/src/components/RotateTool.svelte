<script lang="ts">
  // The rotate tool. One island, and the only JavaScript this page ships.
  //
  // Built on the parts `/merge-pdf` established rather than re-deciding them: the worker host
  // is IMPORTED (not fetched from `/host/`), the engines start on first use, every typed error
  // becomes a sentence in a pure function beside this file, a cancelled operation's reply is
  // ignored by generation rather than suppressed, and the result is a link a person activates.
  // `apps/web/CLAUDE.md`'s "What a tool page is made of" is the list; each of those was a
  // decision with a reason, and this file inherits the reasons with them.
  //
  // WHAT IS DIFFERENT HERE, and why:
  //
  //   * ONE FILE, not a list. So there is no ordering, no per-file problem row, and no
  //     `InputFailed` wrapper to unwrap -- a failure is about the one document in front of
  //     the person.
  //   * A SELECTION, which merge had no equivalent of. Parsed by `page-selection.ts`, a pure
  //     function with its own tests, against the page count the core reported.
  //   * NO THUMBNAILS. ADR 0020 records the decision and issue #57 carries the work. The
  //     honest consequence is that choosing pages means knowing their numbers, and the page
  //     says so rather than leaving somebody to notice.
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { parseSelection } from "./page-selection.js";
  import { messageFor, type Message } from "./rotate-messages.js";
  import {
    LIMITS,
    ORIGIN_MISMATCH,
    createToolHost,
    hostKind,
    originMismatchNotice,
  } from "./tool-host.js";
  import { createDelivery, type Handout } from "./tool-delivery.js";
  import PageThumbnails from "./PageThumbnails.svelte";
  import { STRIP_WITHDRAWN } from "./strip-copy.js";

  let file = $state<File | null>(null);
  /** Pages, once counted. `null` while counting, `-1` if it could not be read. */
  let pageCount = $state<number | null>(null);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<Message | null>(null);
  let result = $state<Handout | null>(null);
  let announcement = $state("");

  /** Which pages to turn: everything, or what is typed in the box. */
  let scope = $state<"all" | "some">("all");
  let selection = $state("");
  /** A quarter turn clockwise, in degrees. The core accepts any multiple of 90. */
  let degrees = $state(90);

  // ---------------------------------------------------------------- the worker

  // THE WIRING IS SHARED, the operations are not. `tool-host.ts` holds the memoised-promise
  // build, the integrity-pinned fetch, the Blob worker and the `LIMITS` -- see its header for
  // why that block in particular is shared when the messages and the markup deliberately are
  // not: it is a fixed security defect, and a fix living in two copies is a fix that will be
  // made in one of them.
  const host = createToolHost();

  // ---------------------------------------------------------------- choosing a file

  async function choose(files: FileList | File[]) {
    const [chosen] = Array.from(files);
    if (!chosen) return;

    // ANYTHING IN FLIGHT IS NOW ABOUT A DOCUMENT NOBODY HAS CHOSEN, so it is invalidated
    // before anything else changes.
    //
    // THIS IS A DISCLOSURE PATH, not a tidiness one, and an earlier version of this comment
    // claimed to have closed it while closing only half. Choose a private document, press
    // Turn pages, change your mind, choose an innocuous one: the first rotation's reply still
    // counted as current, and `suggestedName()` re-derived the name from
    // `file` AT THAT MOMENT. The page then showed the new file's name, the new file's page
    // count, and a download link reading `holiday-rotated.pdf` whose bytes were the private
    // document. Nothing on screen distinguished it. Found by security review.
    //
    // The generation bump is what makes the stale reply stale; `rotate()` also captures the
    // name before it posts, so the two cannot disagree even if this line is ever moved. Both
    // halves live in `tool-delivery.ts` now, with the test none of this had (#69).
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
    // resolves after `choose()` has already swapped `file` and cleared `pageCount`, and
    // without this it would write the old document's page count beside the new document's
    // name -- which `parseSelection` then validates against, and `canRotate` believes.
    const watching = delivery.watch();
    try {
      const h = await host.ensure();
      const raw = await h.run(
        { op: "page_count", blob: chosen, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      // ASK THE HOST whether a worker exists, rather than inferring it from a reply: the host
      // answers `Internal` when `ensureWorker()` fails and `EngineUnavailable` when the
      // breaker has latched, both without a worker ever existing. Merge's first version
      // inferred it and one failed init hid the loading line for the rest of the page.
      engineStarted = h.hasWorker();

      // A count for a file that is no longer chosen is not news.
      if (!watching.live()) return;
      // A count the page cancelled says nothing about the file.
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
      // BY IDENTITY, NOT BY TEXT. `ORIGIN_MISMATCH` says this build is not on the origin it
      // was made for. ADR 0009 forbids reading anything out of a thrown value, so it is
      // compared rather than inspected -- a unique symbol has no text to read. Without this
      // the page showed "Something inside burrow failed" beside a banner that explained
      // exactly what was wrong: the interface contradicting its own explanation, on the one
      // screen whose job was to be believed. Found by code review.
      if (error === ORIGIN_MISMATCH) {
        pageCount = -1;
        notice = originMismatchNotice();
        announce(notice.title);
        return;
      }
      // Nothing else from the thrown value is read: it can carry module output, and module
      // output can carry input-derived bytes (ADR 0009).
      pageCount = -1;
      notice = messageFor({ kind: "Internal" });
    }
  }

  // ---------------------------------------------------------------- rotating

  /**
   * The pages to turn, or why the selection cannot be used.
   *
   * Recomputed as the box is typed in, so the problem appears beside the box rather than
   * after a round trip. The core still refuses anything that gets past this -- the page does
   * not re-implement a ceiling, and this is not one: it is a question about the document in
   * front of the person, which the page already knows the answer to.
   */
  const chosenPages = $derived.by(() => {
    if (pageCount === null || pageCount < 0) return null;
    if (scope === "all") return { ok: true as const, pages: null };
    const parsed = parseSelection(selection, pageCount);
    return parsed.ok ? { ok: true as const, pages: parsed.pages } : parsed;
  });

  const selectionProblem = $derived(
    chosenPages !== null && !chosenPages.ok ? chosenPages.problem : "",
  );

  /**
   * The request the visible result was produced from.
   *
   * A finished rotation is over the moment the REQUEST changes -- rotating pages 1-3 and then
   * typing `1-3, 7` used to leave "Turn pages" disabled with no explanation, beside a download
   * link for the old result whose filename still looked right. That is the defect
   * `MergeTool.svelte` records having been found by both of its reviews, in a new shape: its
   * request is the file list, and rotate's is the file AND the controls. Found by code review.
   *
   * A SIGNATURE RATHER THAN AN EFFECT, and the first attempt is worth recording. It was an
   * `$effect` that cleared the result when `scope`, `selection` or `degrees` changed -- with
   * `if (phase === "done")` inside it. Reading `phase` there makes `phase` a DEPENDENCY, so
   * the moment a rotation finished the effect re-ran and undid it: the download link never
   * appeared and every test that downloads timed out at 45 seconds. Ten of them, caught by the
   * e2e suite and by nothing else.
   */
  const request = $derived(`${file?.name ?? ""}|${scope}|${selection}|${degrees}`);
  const stale = $derived(result !== null && result.signature !== request);

  const canRotate = $derived(
    file !== null &&
      typeof pageCount === "number" &&
      pageCount > 0 &&
      phase !== "working" &&
      chosenPages !== null &&
      chosenPages.ok,
  );

  /**
   * Generations, capture-before-await, and the object URLs -- one shared path (#69).
   *
   * A cancelled rotation STILL ANSWERS: `discardWorker()` terminates the worker and the host
   * fails every in-flight request with `Internal` (ADR 0015) -- correct from its point of
   * view, and a lie to a person who pressed Stop. The reply is ignored by generation rather
   * than the failure branch suppressed, so a genuine failure arriving a moment late is still
   * reported.
   */
  const delivery = createDelivery();

  /**
   * Whether the engines have ever finished starting on this page.
   *
   * Engines load on first use (ADR 0018), so the first file waits for the engine payload.
   * At 6.8 MB that was 7 seconds on Fast 4G and 145 on Slow 3G, both measured; spike 0004 took
   * it to about 420 KB over the wire and the new timings are PREDICTED until the deploy
   * measures them. Saying nothing while it happens is the page being silent about the one
   * thing the person wants to know.
   */
  let engineStarted = $state(false);
  const preparing = $derived(!engineStarted && file !== null && pageCount === null);

  async function rotate() {
    if (!canRotate || !file) return;

    // DECIDED BEFORE THE FIRST AWAIT, both of them.
    //
    // `canRotate` is checked on the line above, and `await ensureHost()` below is a 270 KB
    // fetch on the first operation of a page. The selection box is live during it, so a
    // person who cleared the box inside that window used to fall through to "every page" --
    // the exact silent reinterpretation `page-selection.ts` refuses for a blank box, done by
    // the code that cites it. And `suggestedName()` read `file` after the await, so a file
    // chosen during the window named the download. Both found by security review.
    const chosen = chosenPages;
    const pages =
      chosen !== null && chosen.ok && chosen.pages !== null
        ? chosen.pages
        : Array.from({ length: pageCount ?? 0 }, (_, i) => i + 1);
    // THE NAME AND THE SIGNATURE, CAPTURED HERE and not readable after the awaits at all:
    // `run.hand()` takes bytes and nothing else. `request` is a `$derived` over the live
    // controls, so reading it back after the awaits records what the page says when the reply
    // lands rather than what was posted, and a person who changes the angle mid-rotation gets
    // a download link that is not stale, labelled for a rotation the bytes do not carry. Found
    // by security review on reorder's page; the same line sat here.
    const run = delivery.begin({ name: suggestedName(), signature: request });

    clearResult();
    notice = null;
    phase = "working";
    announce("Turning pages.");

    try {
      const h = await host.ensure();

      // CANCELLED BEFORE THERE WAS ANYTHING TO CANCEL: during the first operation on a page,
      // `host` is still null while the worker bundle is fetched, so Stop is a no-op and the
      // work would be posted a moment after the person stopped it. The generation is the
      // record either way, so it is read again on the far side of the await.
      if (!run.live()) {
        phase = "idle";
        return;
      }

      // EVERY PAGE IS A LIST, not an empty one: the core refuses an empty selection on
      // purpose. `pages` was decided before the await -- see the top of this function.

      const reply = await h.run(
        { op: "rotate", blob: file, pages, degrees, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );

      // The person stopped this one and has already been told so. Its answer is not news.
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

      // An object URL over the Blob, created only if this run is still current -- and not
      // created at all if it is not, because an unrevoked one holds the bytes for the life of
      // the page. The bytes stay in the Blob; the page never reads them into its own heap, and
      // `clearResult` revokes the URL so the browser can release them.
      const handout = run.hand(reply.output);
      if (!handout) {
        // STALE, so there is nothing to show -- and the page must not be left showing Stop
        // for work that is over. Unreachable today, because `run.live()` is checked in the
        // same synchronous continuation above; the branch exists for the day it is not, and a
        // branch that fails safe is the only kind worth having there.
        phase = "idle";
        return;
      }
      result = handout;
      phase = "done";
      announce(`Done. ${reply.pages} pages, ready to download.`);
    } catch (error) {
      if (!run.live()) return;
      // BY IDENTITY, NOT BY TEXT. `ORIGIN_MISMATCH` says this build is not on the origin it
      // was made for. ADR 0009 forbids reading anything out of a thrown value, so it is
      // compared rather than inspected -- a unique symbol has no text to read. Without this
      // the page showed "Something inside burrow failed" beside a banner that explained
      // exactly what was wrong: the interface contradicting its own explanation, on the one
      // screen whose job was to be believed. Found by code review.
      notice =
        error === ORIGIN_MISMATCH ? originMismatchNotice() : messageFor({ kind: "Internal" });
      phase = "idle";
    }
  }

  function suggestedName(): string {
    const base = file?.name.replace(/\.pdf$/i, "") ?? "document";
    return `${base}-rotated.pdf`;
  }

  /**
   * Stop the operation in flight.
   *
   * `discardWorker()` terminates the worker, which is the only thing that can stop work inside
   * an engine call -- no engine here offers a cancellation hook (ADR 0007). A person changing
   * their mind is not the watchdog's hang case and must not count as a crash: the breaker
   * counts crashes, not respawns, precisely so a cancel does not take the page offline.
   */
  function cancel() {
    // BEFORE the worker goes, so the reply the discard provokes is already stale on arrival.
    delivery.invalidate();
    host.discardWorker();
    phase = "idle";
    notice = null;
    announce("Stopped.");
  }

  /** The deliberate gesture that closes the circuit breaker. */
  function startAgain() {
    host.reset();
    notice = null;
    announce("Ready to try again.");
  }

  function clearResult() {
    delivery.release(result);
    result = null;
  }

  function announce(text: string) {
    // Reassigned even when identical, so a repeated action is still read out.
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  onDestroy(() => {
    clearResult();
    // Disposes a host still being built, too -- see `tool-host.ts`.
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
  <h2 id="tool-heading" class="visually-hidden">Turn your pages</h2>

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
        // Cleared so choosing the SAME file again still fires a change event.
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

  <!-- THE STRIP, after the file line and before the controls. It is an AID: if it draws
       nothing the selection below still works by number, which is how both these pages worked
       before there were pictures at all (ADR 0020). -->
  <!-- WITHDRAWN, NOT REMOVED. `STRIP_WITHDRAWN` in `strip-copy.ts` carries the measurement:
       a tab holding qpdf and PDFium at once loses itself on WebKit, about four times in 580
       runs, and zero without the strip. Losing a tab mid-operation is worse than not seeing
       thumbnails, and this page selected pages by number for its whole life before the strip
       existed. The component, its tests and the render bundle are untouched; restoring it is
       this one boolean, and `strip-copy.test.ts` makes the prose follow it. #107. -->
  {#if !STRIP_WITHDRAWN}
    <PageThumbnails {file} {pageCount} />
  {/if}

  {#if preparing}
    <p class="preparing" role="status">
      Starting the PDF engine — about 420 KB, downloaded once. Nothing has been sent anywhere.
    </p>
  {/if}

  {#if typeof pageCount === "number" && pageCount > 0}
    <fieldset class="choice">
      <legend>Which pages</legend>
      <label class="choice__option">
        <input type="radio" name="scope" value="all" bind:group={scope} />
        Every page
      </label>
      <label class="choice__option">
        <input type="radio" name="scope" value="some" bind:group={scope} />
        Just these:
      </label>
      <input
        type="text"
        class="choice__pages"
        inputmode="numeric"
        placeholder="1-3, 5"
        aria-label="Page numbers to turn, for example 1-3, 5"
        aria-describedby={selectionProblem ? "selection-problem" : undefined}
        aria-invalid={selectionProblem ? "true" : undefined}
        bind:value={selection}
        onfocus={() => (scope = "some")}
      />
      {#if selectionProblem}
        <p class="choice__problem" id="selection-problem">{selectionProblem}</p>
      {/if}
    </fieldset>

    <fieldset class="choice">
      <legend>Which way</legend>
      {#each [{ value: 90, label: "Right, a quarter turn" }, { value: 180, label: "Upside down" }, { value: 270, label: "Left, a quarter turn" }] as option (option.value)}
        <label class="choice__option">
          <input type="radio" name="degrees" value={option.value} bind:group={degrees} />
          {option.label}
        </label>
      {/each}
    </fieldset>
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
    <button type="button" class="action--primary" disabled={!canRotate} onclick={rotate}
      >Turn pages</button
    >
    {#if phase === "working"}
      <button type="button" onclick={cancel}>Stop</button>
      <!-- NO PROGRESS BAR. The rotation is engine calls inside a worker and reports nothing
           until it finishes, so a bar would be animating against nothing. -->
      <p class="working" role="status">Turning pages. This does not report progress.</p>
    {/if}
  </div>

  <!-- HIDDEN THE MOMENT THE REQUEST CHANGES. A link labelled for the old request is worse
       than no link: the filename still looks right. `stale` is the comparison. -->
  {#if phase === "done" && result && !stale}
    <p class="result">
      <!-- A LINK A PERSON ACTIVATES, not a download that starts itself: a tool that writes to
           somebody's disk without being asked is doing something they did not request. -->
      <a class="download" href={result.url} download={result.name}>Download {result.name}</a>
    </p>
  {/if}

  <!-- NOT `.visually-hidden`, AND THE REASON RECORDED HERE FIRST WAS WRONG.

       `.visually-hidden` was defined in `MergeTool.svelte` only, so in this island the class
       did nothing and this region has always rendered visibly. When ADR 0028 moved the rule
       into `base.css` and made it real, a WebKit failure appeared on /split-pdf -- a split
       stalling until its budget expired, its output never arriving -- and removing the class
       here appeared to fix it. IT DID NOT. Measured afterwards at four WebKit suite runs per
       tree: the same failure happens once in four runs at `e40a3a5`, the commit BEFORE that
       ADR, where this class was inert and could not have been involved. Two clean runs after
       the change were simply what a one-in-four failure looks like most of the time.

       So the real defect is older and is not this: an operation on the qpdf worker path whose
       output silently never arrives, on /split-pdf and /rotate-pdf alike. Issue #107 carries
       the rate and the evidence.

       This region keeps the visibility it has had all along -- the status quo, and no
       regression -- because nothing here has been shown to justify changing it in either
       direction. Hiding it is the correct behaviour and wants its own change, with #107
       understood first. -->
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

  .choice {
    border: 0;
    padding: 0;
    margin-block-start: var(--space-4);
  }

  .choice legend {
    color: var(--ink-quiet);
    padding: 0;
  }

  .choice__option {
    display: block;
    margin-block-start: var(--space-2);
  }

  .choice__pages {
    margin-block-start: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    font: inherit;
    font-variant-numeric: tabular-nums;
  }

  .choice__problem {
    color: var(--refuse);
    margin-block-start: var(--space-2);
  }

  .notice {
    color: var(--refuse);
    margin-block-start: var(--space-4);
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

  /* Every control in this row is the same size, so the row reads as one set rather than as
     a primary button with two chips beside it. The primary's fill and size come from
     `.action--primary` in `base.css`. */
  .actions button {
    font-size: var(--step-0);
    padding: var(--space-3) var(--space-5);
  }

  .result {
    margin-block-start: var(--space-4);
  }

  .download {
    /* INK, NOT `--signal` AND NOT `--accent`. `apps/web/CLAUDE.md`: "Links are still ink, and
       `--signal` is still not a control colour" -- the moment a control is the signal colour,
       the colour means "interactive" as well as "measured" and the one number that was
       measured stops standing out. `--accent` is reserved for the PRIMARY action (ADR 0028)
       and this is a download link that appears after the work is done, not the thing the page
       asked you to press. Nothing here was measured; the page count
       beside the filename is this page's measurement, and it is `--ink-quiet`. `MergeTool`
       styles the same element exactly this way. Found by code review. */
    font-weight: var(--weight-strong);
  }
</style>
