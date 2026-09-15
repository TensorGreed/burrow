<script lang="ts">
  // The reorder tool. One island, and the only JavaScript this page ships.
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
  //   * AN ORDER, not a selection. `page-selection.ts` sorts and de-duplicates, which is right
  //     for "which pages" and wrong for "in what order" -- `3, 1` and `1, 3` are different
  //     documents, and `1, 1` is a request that would lose a page rather than a tidy-away.
  //     `reorder-order.ts` is the separate parser, with its own tests.
  //   * A PARTIAL ORDER IS COMPLETED, by a rule the page states and SHOWS. A permutation of a
  //     500-page document is 500 numbers; requiring all of them would make the tool useless
  //     for the thing people want. Pages you list come first, in the order you list them, and
  //     everything else keeps its place after them.
  //   * A PREVIEW, which neither other tool has. The completion rule is a rule rather than a
  //     guess, and the way to keep it one is to let somebody see the result before they run
  //     it -- the design brief's "the interface reports, it does not reassure".
  //   * NO THUMBNAILS, and it bites harder here than on rotate. ADR 0020 records the decision
  //     and #57 carries the work; reordering by number without seeing the pages is genuinely
  //     harder than turning them, and the page says so rather than leaving somebody to notice.
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { messageFor, type Message } from "./reorder-messages.js";
  import { isUnchanged, resolveOrder } from "./reorder-order.js";
  import {
    LIMITS,
    ORIGIN_MISMATCH,
    createToolHost,
    hostKind,
    originMismatchNotice,
  } from "./tool-host.js";
  import { createDelivery, type Handout } from "./tool-delivery.js";

  /**
   * How many of the resulting pages the preview shows before it says "and the rest".
   *
   * TWELVE IS ENOUGH RATHER THAN ARBITRARY: named pages come first, so every change a person
   * makes is in the visible prefix. What the truncation hides is the untouched tail.
   */
  const PREVIEW_PAGES = 12;

  let file = $state<File | null>(null);
  /** Pages, once counted. `null` while counting, `-1` if it could not be read. */
  let pageCount = $state<number | null>(null);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<Message | null>(null);
  let result = $state<Handout | null>(null);
  let announcement = $state("");

  /** The new order, as typed. Empty means "leave it alone". */
  let wanted = $state("");

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
    // arrived with the run still counted as current, and `suggestedName()` re-derived the name from
    // `file` AT THAT MOMENT. The page then showed the new file's name, the new file's page
    // count, and a download link reading `holiday-rotated.pdf` whose bytes were the private
    // document. Nothing on screen distinguished it. Found by security review.
    //
    // The generation bump is what makes the stale reply stale; `reorder()` captures the name
    // and the signature before it posts, so the two cannot disagree even if this line moves.
    // Both halves live in `tool-delivery.ts` now, with the test none of this had (#69).
    delivery.invalidate();
    clearResult();
    phase = "idle";
    notice = null;
    file = chosen;
    // THE ORDER GOES WITH THE DOCUMENT IT WAS FOR. Carrying `9-5` over to a different file
    // leaves a valid-looking preview for a document the person has not looked at, which is
    // the same "the readout is about a different document" class the rest of this file is
    // careful about. Found by security review.
    wanted = "";
    pageCount = null;
    announce(`${chosen.name} chosen.`);
    await count(chosen);
  }

  async function count(chosen: File) {
    // The generation this count belongs to. A count still pending for the PREVIOUS file
    // resolves after `choose()` has already swapped `file` and cleared `pageCount`, and
    // without this it would write the old document's page count beside the new document's
    // name -- which `resolveOrder` then validates against, and `canReorder` believes.
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

  // ---------------------------------------------------------------- reordering

  /**
   * The complete new order, or why what was typed cannot be used.
   *
   * Recomputed as the box is typed in, so the problem appears beside the box rather than
   * after a round trip. The core still refuses anything that gets past this -- the page does
   * not re-implement a ceiling, and this is not one: it is a question about the document in
   * front of the person, which the page already knows the answer to.
   */
  const resolved = $derived.by(() => {
    if (pageCount === null || pageCount < 0) return null;
    return resolveOrder(wanted, pageCount);
  });

  const orderProblem = $derived(resolved !== null && !resolved.ok ? resolved.problem : "");

  /** The resulting order, or `null` while there is nothing to show. */
  const preview = $derived(resolved !== null && resolved.ok ? resolved.order : null);

  /** Whether the typed order would leave the document exactly as it is. */
  const unchanged = $derived(preview !== null && isUnchanged(preview));

  /**
   * The request the visible result was produced from.
   *
   * A finished reorder is over the moment the REQUEST changes. The defect is merge's, found by
   * both of its reviews and then again on rotate: a download link for the old result, whose
   * filename still looks right. It is sharper here than anywhere -- one permutation of a
   * document looks exactly like another from the outside, so the filename is ALL there is.
   *
   * A SIGNATURE RATHER THAN AN EFFECT. ROTATE'S HISTORY, kept because it is the reason this is
   * a `$derived` comparison and not an effect: an `$effect` that cleared the result when the
   * controls changed read `phase` inside itself, which made `phase` a dependency, so the moment
   * an operation finished the effect re-ran and undid it. The download link never appeared and
   * ten e2e tests timed out at 45 seconds.
   */
  const request = $derived(`${file?.name ?? ""}|${wanted}`);
  const stale = $derived(result !== null && result.signature !== request);

  const canReorder = $derived(
    file !== null &&
      typeof pageCount === "number" &&
      pageCount > 0 &&
      phase !== "working" &&
      resolved !== null &&
      resolved.ok &&
      // AN IDENTITY IS REFUSED BY THE PAGE, not by the core. The core accepts it -- it is a
      // legitimate request and the ROADMAP names it a no-op -- but running it rewrites
      // somebody's file to no effect and hands them a download that differs from their
      // original in every byte while displaying identically. Saying "that is the order it is
      // already in" is more useful than doing it.
      !unchanged,
  );

  /**
   * Generations, capture-before-await, and the object URLs -- one shared path (#69).
   *
   * A cancelled operation STILL ANSWERS: `discardWorker()` terminates the worker and the host
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

  async function reorder() {
    if (!canReorder || !file) return;

    // DECIDED BEFORE THE FIRST AWAIT, both of them. `await host.ensure()` below is a 270 KB
    // fetch on the first operation of a page, and the box is live during it -- a person who
    // edited the order inside that window would otherwise have a DIFFERENT order posted than
    // the one they were looking at, which for this operation means a document whose pages are
    // somewhere nobody asked for. `suggestedName()` is captured for the same reason: rotate's
    // security review found the name re-derived after the await, so a file chosen during the
    // window named the download.
    const order = resolved !== null && resolved.ok ? resolved.order : null;
    if (order === null) return;
    // THE NAME AND THE SIGNATURE ARE CAPTURED HERE, beside `order`, and are not readable
    // after the awaits at all -- `run.hand()` takes bytes and nothing else. `request` is a
    // `$derived` over the LIVE controls, so recording it when the reply lands records what the
    // box says THEN rather than what was posted, and a person who edits the order mid-run gets
    // a download link that is not stale, sitting under a preview showing an order the bytes
    // are not in. Found by security review; now unexpressible rather than commented against.
    const run = delivery.begin({ name: suggestedName(), signature: request });

    clearResult();
    notice = null;
    phase = "working";
    announce("Putting the pages in order.");

    try {
      const h = await host.ensure();

      // CANCELLED BEFORE THERE WAS ANYTHING TO CANCEL: during the first operation on a page,
      // the host is still being built, so Stop is a no-op and the work would be posted a
      // moment after the person stopped it. The generation is the record either way.
      if (!run.live()) {
        phase = "idle";
        return;
      }

      const reply = await h.run(
        { op: "reorder", blob: file, order, password: null, limits: LIMITS },
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

  /** Fill the box with the order that reverses the document. */
  function reverse() {
    if (typeof pageCount !== "number" || pageCount < 2) return;
    // `N-1` is a descending range, which `resolveOrder` reads as a reversal. The preset types
    // into the box rather than setting a hidden mode, so what it did is visible and editable
    // -- a person can reverse and then move one page, which a mode would not allow.
    wanted = `${pageCount}-1`;
  }

  function suggestedName(): string {
    const base = file?.name.replace(/\.pdf$/i, "") ?? "document";
    return `${base}-reordered.pdf`;
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
  <h2 id="tool-heading" class="visually-hidden">Put your pages in order</h2>

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

  {#if preparing}
    <p class="preparing" role="status">
      Starting the PDF engine — about 420 KB, downloaded once. Nothing has been sent anywhere.
    </p>
  {/if}

  {#if typeof pageCount === "number" && pageCount > 0}
    <fieldset class="choice">
      <legend>The new order</legend>

      <p class="choice__rule" id="order-rule">
        Type the pages you want moved. <strong>They come first, in the order you type them</strong>,
        and every page you do not type keeps its place after them.
      </p>

      <!-- NOT `inputmode="numeric"`, which is what rotate's selection box uses and what this
           copied at first. iOS shows a digits-only keypad for that, and this grammar needs `,`
           and `-` — so on a phone the only order typable would be a single page. Found by code
           review. -->
      <input
        type="text"
        class="choice__pages"
        inputmode="text"
        placeholder="3, 1"
        aria-label="The new page order, for example 3, 1"
        aria-describedby={orderProblem ? "order-rule order-problem" : "order-rule order-preview"}
        aria-invalid={orderProblem ? "true" : undefined}
        bind:value={wanted}
      />

      {#if pageCount > 1}
        <button type="button" class="choice__preset" onclick={reverse}>Reverse the order</button>
      {/if}

      {#if orderProblem}
        <p class="choice__problem" id="order-problem">{orderProblem}</p>
      {:else if preview}
        <!-- THE RULE, SHOWN RATHER THAN DESCRIBED. Completing a partial order is a rule and
             not a guess, and the thing that keeps it one is that a person can see the result
             before running it. A sentence alone would be this page asking to be trusted. -->
        <p class="choice__preview" id="order-preview">
          {#if unchanged}
            That is the order the pages are already in.
          {:else}
            Pages will come out:
            <span class="choice__order">
              {preview.slice(0, PREVIEW_PAGES).join(", ")}{preview.length > PREVIEW_PAGES
                ? `, … (${preview.length} in total)`
                : ""}
            </span>
          {/if}
        </p>
      {/if}
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
    <button type="button" disabled={!canReorder} onclick={reorder}>Put pages in order</button>
    {#if phase === "working"}
      <button type="button" onclick={cancel}>Stop</button>
      <!-- NO PROGRESS BAR. The reordering is engine calls inside a worker and reports nothing
           until it finishes, so a bar would be animating against nothing. -->
      <p class="working" role="status">Putting pages in order. This does not report progress.</p>
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

  <p class="visually-hidden" role="status" aria-live="polite">{announcement}</p>
</section>

<style>
  .tool {
    margin-block-start: var(--space-6);
    max-width: var(--track-readout);
  }

  .drop {
    display: block;
    border: 1px dashed var(--edge);
    border-radius: var(--radius);
    padding: var(--space-5);
    text-align: center;
    cursor: pointer;
  }

  .drop--over {
    border-style: solid;
  }

  .choice__rule {
    margin: 0 0 var(--space-3);
    color: var(--ink-quiet);
  }

  .choice__preset {
    margin-block-start: var(--space-3);
  }

  .choice__preview {
    margin: var(--space-3) 0 0;
    color: var(--ink-quiet);
  }

  .choice__order {
    /* TABULAR, because it is a readout of numbers and a number that shifts width while it
       updates is a number that looks unreliable. The design brief asks for this wherever a
       count is shown. */
    font-variant-numeric: tabular-nums;
    color: var(--ink);
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

  .result {
    margin-block-start: var(--space-4);
  }

  .download {
    /* INK, NOT `--signal`. `apps/web/CLAUDE.md`: "Links and buttons are ink, not `--signal`.
       This is the costliest rule in the system and the point of it" -- the moment a control
       is the signal colour, the colour means "interactive" as well as "measured" and the one
       number that was measured stops standing out. Nothing here was measured; the page count
       beside the filename is this page's measurement, and it is `--ink-quiet`. `MergeTool`
       styles the same element exactly this way. Found by code review. */
    font-weight: var(--weight-strong);
  }
</style>
