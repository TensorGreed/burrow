<script lang="ts">
  // The split tool. One island, and the only JavaScript this page ships.
  //
  // Built on the parts the other three established rather than re-deciding them: the worker
  // host is IMPORTED, the engines start on first use, every typed error becomes a sentence in
  // a pure function beside this file, a cancelled operation's reply is ignored by generation
  // rather than suppressed, and the result is a link a person activates. `apps/web/CLAUDE.md`'s
  // "What a tool page is made of" is the list; this file inherits the reasons with the parts.
  //
  // WHAT IS DIFFERENT HERE, and why:
  //
  //   * MORE THAN ONE DOCUMENT COMES OUT. This is the only operation where that is true, and
  //     it is a protocol (ADR 0023) rather than a loop: parts arrive one at a time, each
  //     verified before it is posted, and the host holds them until the terminal reply
  //     because a failure anywhere delivers NOTHING (§3). `split` is defined as a partition,
  //     and nine parts out of ten is not a partition of anything.
  //   * A REAL PROGRESS BAR, which no other tool page has. The other three report nothing
  //     until they finish and say so. Split reports per part (§6) -- one message before the
  //     first part so a bar knows how many are coming, and one after each. Within a part
  //     there is nothing to report and the page does not pretend otherwise.
  //   * NAMES THAT STATE WHAT IS IN THE FILE. `holiday-pages-004-007.pdf`, zero-padded to the
  //     width of the source's page count so a file manager sorts them. A person splitting a
  //     scan acts on WHICH PAGES are in which file; `-part-2` makes them open each one to
  //     find out. It also makes the name checkable against the bytes, which
  //     `e2e/split-pdf.spec.ts` does: it reads the span out of each downloaded file's own
  //     name and asserts the part has that many pages.
  //   * A CUT IS A GAP, NOT A PAGE, and that off-by-one is the thing people get wrong. The
  //     page shows the resulting parts before anything runs, which is the answer
  //     /reorder-pdf gave for its completion rule: a rule stays a rule by being visible.
  import { onDestroy } from "svelte";

  import { CANCELLED } from "../host/worker-host.js";
  import { messageFor, type Message } from "./split-messages.js";
  import { everyPageCuts, isWholeDocument, resolveCuts, type Part } from "./split-cuts.js";
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

  /**
   * How many parts the preview lists before it says "and the rest".
   *
   * A hundred-way split is a legitimate request and a hundred lines of preview is not a
   * preview. What the truncation hides is the uniform tail; every cut a person typed is
   * visible in the prefix, because the parts are in order.
   */
  const PREVIEW_PARTS = 12;

  let file = $state<File | null>(null);
  /** Pages, once counted. `null` while counting, `-1` if it could not be read. */
  let pageCount = $state<number | null>(null);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<Message | null>(null);
  let results = $state<Handout[] | null>(null);
  let announcement = $state("");

  /** Where to cut, as typed. Empty means "do not cut". */
  let wanted = $state("");

  /** Parts produced so far, and how many are coming. Both from the worker (ADR 0023 §6). */
  let done = $state(0);
  let expected = $state(0);

  // ---------------------------------------------------------------- the worker

  const host = createToolHost();

  /**
   * Generations, capture-before-await, and the object URLs -- one shared path (#69).
   *
   * A cancelled operation STILL ANSWERS: `discardWorker()` terminates the worker and the host
   * fails every in-flight request with `Internal` (ADR 0015), which is correct from its point
   * of view and a lie to a person who pressed Stop. The reply is ignored by generation rather
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

  // ---------------------------------------------------------------- choosing a file

  async function choose(files: FileList | File[]) {
    const [chosen] = Array.from(files);
    if (!chosen) return;

    // ANYTHING IN FLIGHT IS NOW ABOUT A DOCUMENT NOBODY HAS CHOSEN, so it is invalidated
    // before anything else changes. This is a disclosure path rather than a tidiness one:
    // choose a private document, press Split, change your mind, choose an innocuous one --
    // without this the first split's parts would arrive under names derived from the file
    // chosen SINCE. `tool-delivery.ts`'s header has both findings that produced this rule.
    delivery.invalidate();
    clearResults();
    phase = "idle";
    notice = null;
    file = chosen;
    // THE CUTS GO WITH THE DOCUMENT THEY WERE FOR. Carrying `3, 7` over to a different file
    // leaves a valid-looking preview for a document the person has not looked at.
    wanted = "";
    pageCount = null;
    done = 0;
    expected = 0;
    announce(`${chosen.name} chosen.`);
    await count(chosen);
  }

  async function count(chosen: File) {
    // The generation this count belongs to. A count still pending for the PREVIOUS file
    // resolves after `choose()` has swapped `file` and cleared `pageCount`, and without this
    // it would write the old document's page count beside the new document's name -- which
    // `resolveCuts` then validates against.
    const watching = delivery.watch();
    try {
      const h = await host.ensure();
      const raw = await h.run(
        { op: "page_count", blob: chosen, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      // ASK THE HOST whether a worker exists, rather than inferring it from a reply: the host
      // answers `Internal` when `ensureWorker()` fails and `EngineUnavailable` when the
      // breaker has latched, both without a worker ever existing.
      engineStarted = h.hasWorker();

      if (!watching.live()) return;
      if (!raw.ok && raw.kind === CANCELLED) {
        // A COUNT THE PAGE CANCELLED SAYS NOTHING ABOUT THE FILE -- but leaving `pageCount`
        // at `null` leaves `.chosen__pages` reading "counting…" for ever, with no recovery
        // but choosing another file. Unreachable today (Stop only renders while an operation
        // is working, and no count can be queued then), so this is a latent footgun closed
        // for one line rather than a live defect. Code review.
        pageCount = -1;
        return;
      }
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

  // ---------------------------------------------------------------- splitting

  /**
   * Where the cuts fall, or why what was typed cannot be used.
   *
   * Recomputed as the box is typed in, so the problem appears beside the box rather than after
   * a round trip. The core still refuses anything that gets past this -- the page does not
   * re-implement a ceiling, and this is not one: it is a question about the document in front
   * of the person, which the page already knows the answer to.
   */
  const resolved = $derived.by(() => {
    if (pageCount === null || pageCount < 0) return null;
    return resolveCuts(wanted, pageCount);
  });

  const cutProblem = $derived(resolved !== null && !resolved.ok ? resolved.problem : "");

  /** The parts the typed cuts would produce, or `null` while there is nothing to show. */
  const preview = $derived<Part[] | null>(resolved !== null && resolved.ok ? resolved.parts : null);

  /** Whether the typed cuts would leave the document whole. */
  const whole = $derived(preview !== null && isWholeDocument(preview));

  /**
   * The request the visible result was produced from.
   *
   * A finished split is over the moment the REQUEST changes. The defect is merge's, found by
   * both of its reviews and then again on rotate: a download link for the old result, whose
   * filename still looks right. It is sharpest here, because there are several links and a
   * person reads the set rather than each name.
   *
   * A SIGNATURE RATHER THAN AN EFFECT. An `$effect` that cleared the result when the controls
   * changed read `phase` inside itself, which made `phase` a dependency, so the moment an
   * operation finished the effect re-ran and undid it -- the download link never appeared and
   * ten e2e tests timed out at 45 seconds.
   */
  const request = $derived(`${file?.name ?? ""}|${wanted}`);
  const stale = $derived(results !== null && results.some((r) => r.signature !== request));

  const canSplit = $derived(
    file !== null &&
      typeof pageCount === "number" &&
      pageCount > 0 &&
      phase !== "working" &&
      resolved !== null &&
      resolved.ok &&
      // NOT CUTTING IS REFUSED BY THE PAGE, not by the core. The core accepts an empty cut
      // list -- it is a legitimate one-way split and the identity case its property tests use
      // -- but running it rewrites somebody's file into a copy of itself. Saying "that is the
      // document you already have" is more useful than doing it.
      !whole,
  );

  /**
   * What each part will be called.
   *
   * ZERO-PADDED TO THE SOURCE'S WIDTH, so ten parts sort as 001…010 rather than 1, 10, 2 in
   * every file manager a person is likely to open them in.
   */
  function partNames(base: string, parts: readonly Part[], pages: number): string[] {
    const width = String(pages).length;
    const pad = (n: number) => String(n).padStart(width, "0");
    return parts.map((p) => `${base}-pages-${pad(p.first)}-${pad(p.first + p.count - 1)}.pdf`);
  }

  /** The chosen file's name without its extension, or a fallback. */
  function baseName(): string {
    return file?.name.replace(/\.pdf$/i, "") ?? "document";
  }

  async function split() {
    if (!canSplit || !file) return;

    // DECIDED BEFORE THE FIRST AWAIT, all of it. `await host.ensure()` below is a 270 KB fetch
    // on the first operation of a page, and the box is live during it -- a person who edited
    // the cuts inside that window would otherwise have a DIFFERENT cut list posted than the
    // one they were looking at, and would be handed files whose names describe the cuts they
    // typed rather than the cuts that ran.
    const cuts = resolved !== null && resolved.ok ? resolved.cuts : null;
    const parts = preview;
    const pages = pageCount;
    if (cuts === null || parts === null || typeof pages !== "number") return;
    // THE NAMES ARE CAPTURED HERE, one per part, and are not readable after the awaits at all
    // -- `run.handAll()` takes bytes and nothing else. `handAll` refuses the whole set if the
    // number of parts does not match the number of names, which is the check that stops a
    // short set being handed out under confident names.
    const run = delivery.beginParts({
      names: partNames(baseName(), parts, pages),
      signature: request,
    });

    clearResults();
    notice = null;
    phase = "working";
    done = 0;
    expected = parts.length;
    announce(`Splitting into ${parts.length} parts.`);

    try {
      const h = await host.ensure();

      // CANCELLED BEFORE THERE WAS ANYTHING TO CANCEL: during the first operation on a page
      // the host is still being built, so Stop is a no-op and the work would be posted a
      // moment after the person stopped it. The generation is the record either way.
      if (!run.live()) {
        phase = "idle";
        return;
      }

      const reply = await h.run(
        { op: "split", blob: file, cuts, password: null, limits: LIMITS },
        {
          maxDurationMs: LIMITS.maxDurationMs,
          // ADR 0023 §6. `of` arrives on the FIRST message, before any part, so the bar knows
          // how many are coming rather than growing as they land. `part` counts parts
          // finished, not started.
          onProgress: (progress) => {
            if (!run.live()) return;
            expected = progress.of;
            done = progress.part;
          },
        },
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

      // ALL OR NOTHING, TWICE OVER. The host withholds `parts` entirely unless every one
      // arrived (ADR 0023 §3), and `handAll` withholds them again unless the set matches the
      // names captured for it. Neither creates a URL when it refuses.
      const handouts = run.handAll(reply.parts);
      if (!handouts) {
        // Either the run went stale in the same synchronous continuation -- unreachable
        // today, since `run.live()` was checked just above -- or the worker delivered a set
        // that does not match what was asked for. The second is a protocol violation rather
        // than a document problem, and `Internal` is what the page says about those.
        notice = messageFor({ kind: "Internal" });
        phase = "idle";
        announce(notice.title);
        return;
      }
      results = handouts;
      phase = "done";
      done = handouts.length;
      // ANNOUNCED ONLY IF IT WILL BE SHOWN. Editing the cut box during a run does not bump the
      // generation -- `handAll` succeeds, and the `stale` comparison is what hides the set --
      // so the completion path used to say "N parts, ready to download" while nothing was
      // rendered. A screen-reader user was told files were ready, found none, and got no
      // explanation. Security review. Compared against the live `request` rather than against
      // `stale`, which is derived and need not have settled in this continuation.
      const shows = handouts[0]?.signature === request;
      announce(
        shows
          ? `Done. ${handouts.length} parts, ready to download.`
          : "Done, but the cuts changed while it ran, so those files are not the answer to what the box says now. Split again.",
      );
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
    done = 0;
    expected = 0;
    announce("Stopped.");
  }

  /**
   * A result the page has decided not to show is released, not merely hidden.
   *
   * `stale` takes the links out of the DOM when the cuts change, and `results` used to keep
   * every handout alive behind them: N object URLs over N complete PDFs of the person's
   * document, resident until the next split, the next file, or the page closing. On a
   * forty-part split that is the whole document held a second time, after the page had
   * decided not to offer it. Security review.
   *
   * IT DOES NOT COME BACK when the original cuts are typed in again, and that is the intended
   * half of the trade: those bytes would be offered under a set of names whose provenance
   * nobody could check from the screen. Splitting again is cheap and is the honest answer.
   *
   * NOT COVERED BY A TEST, AND SAID SO RATHER THAN LEFT TO LOOK COVERED. What a browser can
   * see is that no link is offered and that the page says why, and `e2e/split-pdf.spec.ts`
   * asserts both. The REVOCATION is invisible from the DOM, and nothing in this app can drive
   * a Svelte effect in isolation -- there is no component-test harness, which is the same gap
   * security review raised about the delivery calls themselves. The three browser tests it
   * asked for exist now; this one line does not have an equivalent.
   *
   * READS ONLY `stale` AND `results`. The `$effect` trap this file's `request` comment records
   * was an effect that read `phase` and so took a dependency on it, undoing the result the
   * moment an operation finished. `phase` is written here and never read.
   */
  $effect(() => {
    if (stale && results) {
      const dropped = results;
      results = null;
      phase = "idle";
      delivery.releaseAll(dropped);
    }
  });

  /**
   * Fill the box with the cuts that give every page its own document.
   *
   * THE PRESET TYPES INTO THE BOX rather than setting a hidden mode, which is /reorder-pdf's
   * answer for "Reverse the order" and the same reason: what it did is visible and editable,
   * so a person can ask for every page and then change one cut, which a mode would not allow.
   *
   * It exists because without it this page could not do one of the two things people most
   * want from a splitter: 39 cut points typed by hand for a 40-page document, 499 for a
   * 500-page one.
   */
  function everyPage() {
    if (typeof pageCount !== "number" || pageCount < 2) return;
    wanted = everyPageCuts(pageCount);
  }

  /** The deliberate gesture that closes the circuit breaker. */
  function startAgain() {
    host.reset();
    notice = null;
    announce("Ready to try again.");
  }

  function clearResults() {
    delivery.releaseAll(results);
    results = null;
  }

  function announce(text: string) {
    // Reassigned even when identical, so a repeated action is still read out.
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  onDestroy(() => {
    // INVALIDATE FIRST. Teardown is safe today only because `host.dispose()` settles the
    // in-flight request as a failure and there is no client router, so unmount coincides with
    // the document going away. If either changed, the continuation after the await would run
    // with the run still live AFTER `clearResults()`, creating object URLs nothing holds a
    // reference to -- unrevocable for the life of the document. One line, rather than a
    // property that lives in two other files. Security review.
    delivery.invalidate();
    clearResults();
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
  <h2 id="tool-heading" class="visually-hidden">Split your PDF</h2>

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
      <legend>Where to cut</legend>

      <p class="choice__rule" id="cut-rule">
        Give the pages to cut <strong>after</strong>. On a ten-page document,
        <code>3, 7</code> makes three files: pages 1-3, 4-7 and 8-10. A range cuts after each page
        in it, so <code>1-9</code> gives every page its own file.
      </p>

      <!-- NOT `inputmode="numeric"`: iOS shows a digits-only keypad for that, and this
           grammar needs `,` — so on a phone the only cut list typable would be a single
           number. The same finding as /reorder-pdf's order box. -->
      <input
        type="text"
        class="choice__cuts"
        inputmode="text"
        placeholder="3, 7"
        aria-label="The pages to cut after, for example 3, 7"
        aria-describedby={cutProblem ? "cut-rule cut-problem" : "cut-rule cut-preview"}
        aria-invalid={cutProblem ? "true" : undefined}
        bind:value={wanted}
      />

      {#if pageCount > 1}
        <button type="button" class="choice__preset" onclick={everyPage}>
          Cut after every page
        </button>
      {/if}

      {#if cutProblem}
        <p class="choice__problem" id="cut-problem">{cutProblem}</p>
      {:else if preview}
        <!-- THE PARTS, SHOWN RATHER THAN DESCRIBED. A cut is a gap and not a page, and the
             way to keep that a rule rather than a guess is to let a person see the files
             they will get before running anything. -->
        <p class="choice__preview" id="cut-preview">
          {#if whole}
            That would leave the document in one piece, which is the document you already have.
          {:else}
            {preview.length} files:
            <span class="choice__parts">
              {preview
                .slice(0, PREVIEW_PARTS)
                .map((p) =>
                  p.count === 1 ? `page ${p.first}` : `${p.first}-${p.first + p.count - 1}`,
                )
                .join(", ")}{preview.length > PREVIEW_PARTS
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
    <button type="button" class="action--primary" disabled={!canSplit} onclick={split}>Split</button
    >
    {#if phase === "working"}
      <button type="button" onclick={cancel}>Stop</button>
    {/if}
  </div>

  <!-- THE READOUT IS NOT IN THE BUTTON ROW, and that is a fix rather than a layout choice.
       It was, and this is the only page whose readout CHANGES while it is on screen — "Part 3
       of 41" is wider than "Part 4 of 41" at some counts, so every progress message reflowed
       the flex row and moved the Stop button. Firefox then refused the click outright:
       "element is not stable … element was detached from the DOM". A person gets the same
       thing as a misclick. The other three pages carry a static sentence, so their row never
       moves and none of them met this.

       A REAL COUNT, because this operation genuinely reports one (ADR 0023 §6). It is parts
       FINISHED out of parts coming, and it does not interpolate inside a part — the core
       exposes no progress hook inside an operation, and a bar that moved smoothly would be
       inventing a number. -->
  {#if phase === "working"}
    <p class="working" role="status">
      {#if expected > 0}
        Part {Math.min(done + 1, expected)} of {expected}.
      {:else}
        Starting.
      {/if}
      Nothing is handed over until every part is done.
    </p>
  {/if}

  <!-- HIDDEN THE MOMENT THE REQUEST CHANGES. Links labelled for the old request are worse
       than no links: the filenames still look right. `stale` is the comparison. -->
  {#if phase === "done" && results && !stale}
    <div class="result">
      <p class="result__count">
        {results.length} files. Each one is a complete PDF of the pages its name gives.
      </p>
      <!-- LINKS A PERSON ACTIVATES, not downloads that start themselves. A tool that writes
           several files to somebody's disk without being asked is doing something they did
           not request, several times over. -->
      <ul class="result__list">
        {#each results as handout (handout.url)}
          <li>
            <a class="download" href={handout.url} download={handout.name}>{handout.name}</a>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <!-- `.visually-hidden`, RESTORED. This region exists to announce state changes to a screen
       reader, and every string it announces is already on screen -- so while the class was
       inert here it rendered a SECOND copy of the refusal, under the button, in a different
       style. Reported from the live site as "the error renders twice".

       IT WAS LEFT VISIBLE FOR A DAY ON A WRONG DIAGNOSIS. Making this class real coincided
       with a WebKit failure on /split-pdf, and removing it appeared to fix it; four runs per
       tree later, the failure happened at the same rate on a commit where the class was inert
       and could not have been involved. The cause was the page-picture strip holding a second
       wasm engine in the tab, and that strip is withdrawn (#107). Nothing implicates this
       rule, and the duplicate it caused is real, so it goes back. -->
  <p class="visually-hidden" role="status" aria-live="polite">{announcement}</p>
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

  .choice__rule {
    margin: 0 0 var(--space-3);
    color: var(--ink-quiet);
  }

  .choice__cuts {
    margin-block-start: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    font: inherit;
    font-variant-numeric: tabular-nums;
  }

  .choice__preset {
    margin-block-start: var(--space-3);
  }

  .choice__preview {
    margin: var(--space-3) 0 0;
    color: var(--ink-quiet);
  }

  .choice__parts {
    /* TABULAR, because it is a readout of numbers and a number that shifts width while it
       updates is a number that looks unreliable. */
    font-variant-numeric: tabular-nums;
    color: var(--ink);
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

  .result__count {
    margin: 0 0 var(--space-3);
    color: var(--ink-quiet);
  }

  .result__list {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .result__list li {
    padding-block: var(--space-1);
  }

  .download {
    /* INK, NOT `--signal` AND NOT `--accent`. `apps/web/CLAUDE.md`: "Links are still ink, and
       `--signal` is still not a control colour" -- the moment a control is the signal colour,
       the colour means "interactive" as well as "measured", and the one
       number that was measured stops standing out. The page count beside the filename is this
       page's measurement, and it is `--ink-quiet`. `--accent` is the PRIMARY action only
       (ADR 0028), and this link appears once the work is done. */
    font-weight: var(--weight-strong);
    font-variant-numeric: tabular-nums;
  }
</style>
