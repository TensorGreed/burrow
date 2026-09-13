<script lang="ts">
  // The merge tool. One island, and the only JavaScript this page ships.
  //
  // THE WORKER HOST IS IMPORTED, NOT FETCHED FROM /host/.
  //
  // `src/host/worker-host.js` is also STAGED to `public/host/` by
  // `tools/stage-web-engines.mjs`, so the harness page can load it from a URL — and
  // `astro.config.mjs` deletes that directory from production builds, which
  // `production-build.test.ts` asserts. None of that applies here: importing it means Vite
  // bundles the lifecycle into this island's chunk, so no `/host/` URL ships and the
  // deletion stays correct. The same file, reached two ways, for two different consumers.
  //
  // DO NOT PUT LIFECYCLE LOGIC IN THIS FILE. `apps/web/CLAUDE.md` is explicit: the state
  // machine lives in `worker-host.js`, with tests. This component decides what to show.
  import { onDestroy } from "svelte";

  import { CANCELLED, ENGINE_UNAVAILABLE, createWorkerHost } from "../host/worker-host.js";
  import { ENGINES } from "../generated/engines.js";
  import { messageFor, type Message } from "./merge-messages.js";

  interface Entry {
    id: number;
    file: File;
    /** Pages, once counted. `null` while counting, `-1` if it could not be read. */
    pages: number | null;
    /** Why this file cannot be used, or null. */
    problem: Message | null;
  }

  let entries = $state<Entry[]>([]);
  let phase = $state<"idle" | "working" | "done">("idle");
  let notice = $state<Message | null>(null);
  let result = $state<{ url: string; name: string; pages: number } | null>(null);
  let announcement = $state("");
  let nextId = 0;

  // ---------------------------------------------------------------- the worker

  /** Built on first use, never on page load. See the page's comment on `client:visible`. */
  let host: ReturnType<typeof createWorkerHost> | null = null;
  let workerUrl: string | null = null;

  /**
   * THE PROMISE IS MEMOISED, NOT THE RESULT, and that is the rule `apps/web/CLAUDE.md`
   * states for the worker's own engine init: "Guard flags set after an `await` are exactly
   * what allows it, so memoise the **promise**, not the result."
   *
   * The same shape was here: `if (host) return host` guarded a function that assigned `host`
   * only after awaiting a 270 KB fetch. `add()` puts a file in the list BEFORE awaiting its
   * count, so Merge is live during that fetch -- two callers arriving inside the window each
   * built a host, each spawned a worker with its own pdfium and qpdf, they shared one
   * `workerUrl` binding so one revoked the other's, and `cancel()`, `startAgain()` and
   * `dispose()` reached only the last. The first worker then held file bytes for the life of
   * the page. Found by security review; no hostile input needed, just a second click.
   */
  let hostPromise: Promise<ReturnType<typeof createWorkerHost>> | null = null;

  function ensureHost() {
    hostPromise ??= buildHost();
    return hostPromise;
  }

  async function buildHost() {
    // The worker's SOURCE is fetched with its integrity digest and the worker is built from
    // a Blob. Not a convenience: a dedicated worker loaded from a same-origin script URL
    // does not inherit this page's CSP — it takes its policy from that script's response
    // headers, and a static host sends none, so it would run unpoliced. ADR 0014 §1a,
    // measured. The worker is the only place file bytes ever exist.
    //
    // Fetched once per page, because this function runs once per page: the memoised promise
    // above is what makes that true, so the source needs no guard of its own.
    const entry = ENGINES.worker;
    const response = await fetch(entry.url, { integrity: entry.integrity });
    if (!response.ok) throw new Error("worker fetch failed");
    const workerSource = await response.text();

    const built = createWorkerHost({
      spawn: () => {
        if (workerUrl) URL.revokeObjectURL(workerUrl);
        // The engine manifest is NOT injected here. `stage-web-engines.mjs` generates
        // `const BURROW_ENGINES = {...}` into the bundle itself, because `pdfium.js` begins
        // instantiating as it is parsed -- there is no moment after load and before
        // instantiation at which a postMessage could arrive (ADR 0014 §1a). The source is
        // complete as fetched.
        workerUrl = URL.createObjectURL(new Blob([workerSource], { type: "text/javascript" }));

        return new Worker(workerUrl);
      },
      release: () => {
        if (workerUrl) {
          URL.revokeObjectURL(workerUrl);
          workerUrl = null;
        }
      },
      now: () => performance.now(),
      setTimer: (fn, ms) => setTimeout(fn, ms),
      clearTimer: (handle) => clearTimeout(handle as number),
    });
    host = built;
    return built;
  }

  /**
   * A host verdict, as the kind `messageFor` expects.
   *
   * `worker-host.js` produces two kinds that are not `burrow_types::Error` variants:
   * `ENGINE_UNAVAILABLE` when the breaker has latched, and `CANCELLED` when the page stopped
   * a request before it was posted. Neither is a failure of a file. Normalised in one place
   * so the counting path and the merging path cannot describe the same verdict differently.
   */
  function hostKind(kind: string): string {
    if (kind === ENGINE_UNAVAILABLE) return "EngineUnavailable";
    return kind;
  }

  const LIMITS = {
    maxInputBytes: 512 * 1024 * 1024,
    maxMemoryBytes: 1024 * 1024 * 1024,
    maxDurationMs: 120_000,
    maxPages: 10_000,
    maxPixels: 256 * 1024 * 1024,
  };

  // ---------------------------------------------------------------- adding files

  async function add(files: FileList | File[]) {
    // A FINISHED MERGE IS OVER THE MOMENT THE LIST CHANGES. `phase` stayed `"done"` until the
    // list was emptied one file at a time, and `canMerge` requires `"idle"` -- so adding a
    // third file to a finished pair rendered Merge disabled with no explanation, beside a
    // download link for a document that no longer matched the list. Found by both reviews.
    finished();

    const added: Entry[] = [];
    for (const file of Array.from(files)) {
      const entry: Entry = { id: nextId++, file, pages: null, problem: null };
      added.push(entry);
    }
    entries = [...entries, ...added];
    announce(`${added.length} file${added.length === 1 ? "" : "s"} added.`);

    // Count pages one at a time. `run()` queues anyway — the worker's message loop is
    // single-threaded (ADR 0015 §2a) — so awaiting each is what actually happens either
    // way, and doing it explicitly means a failure is attributable to its own file.
    for (const entry of added) {
      await count(entry);
    }
  }

  async function count(entry: Entry) {
    try {
      const h = await ensureHost();
      const raw = await h.run(
        { op: "page_count", blob: entry.file, password: null, limits: LIMITS },
        { maxDurationMs: LIMITS.maxDurationMs },
      );
      // `ENGINE_UNAVAILABLE` and `CANCELLED` are HOST verdicts, not `Error` variants, so the
      // kind is normalised here exactly as `merge()` does it rather than in two dialects.
      // A count the page cancelled says nothing about the file. Leave the row as it was.
      if (!raw.ok && raw.kind === CANCELLED) return;
      const reply = raw.ok ? raw : { ...raw, kind: hostKind(raw.kind) };
      // THE BREAKER IS NOT A PROPERTY OF THIS FILE. `EngineUnavailable` means the page has no
      // engine and will not be given one until somebody asks; pinning it to the row would
      // mark an innocent document "cannot be used", and -- worse -- the "Start again" button
      // that clears the breaker is rendered from `notice`, so three hostile files could
      // disable the page while telling the person to press a control that was not on it.
      // Found by security review.
      //
      // THE KIND IS THE TEST, not `message.file < 0`. A first attempt used the index and was
      // wrong in all three browsers: a `page_count` reply has one input and so never carries
      // an index, which made every ordinary refusal -- malformed, encrypted -- look like a
      // verdict about the page. Here the file is known because we are the ones counting it;
      // what has to be decided is whether the failure is about that file at all.
      const message = reply.ok ? null : messageFor(reply);
      if (message !== null && reply.kind === "EngineUnavailable") {
        notice = message;
        entries = entries.map((e) => (e.id === entry.id ? { ...e, pages: -1 } : e));
        announce(message.title);
        return;
      }

      // The list is reassigned rather than mutated, because Svelte tracks the array.
      entries = entries.map((e) =>
        e.id === entry.id
          ? reply.ok
            ? { ...e, pages: reply.pages, problem: null }
            : { ...e, pages: -1, problem: message }
          : e,
      );
    } catch {
      // Nothing from the thrown value is read. It can carry module output, and module
      // output can carry input-derived bytes (ADR 0009).
      entries = entries.map((e) =>
        e.id === entry.id ? { ...e, pages: -1, problem: messageFor({ kind: "Internal" }) } : e,
      );
    }
  }

  // ---------------------------------------------------------------- ordering

  function move(index: number, by: number) {
    finished();
    const to = index + by;
    if (to < 0 || to >= entries.length) return;
    const next = [...entries];
    const [moved] = next.splice(index, 1);
    next.splice(to, 0, moved);
    entries = next;
    announce(`${moved.file.name} moved to position ${to + 1} of ${next.length}.`);
  }

  function remove(index: number) {
    finished();
    const [removed] = entries.slice(index, index + 1);
    entries = entries.filter((_, i) => i !== index);
    if (removed) announce(`${removed.file.name} removed.`);
    if (entries.length === 0) reset();
  }

  // ---------------------------------------------------------------- merging

  const unusable = $derived(entries.filter((e) => e.problem !== null));
  const totalPages = $derived(
    entries.every((e) => typeof e.pages === "number" && e.pages >= 0)
      ? entries.reduce((n, e) => n + (e.pages ?? 0), 0)
      : null,
  );
  const canMerge = $derived(entries.length > 0 && unusable.length === 0 && phase === "idle");

  /**
   * Which merge is current.
   *
   * A cancelled merge STILL ANSWERS. `discardWorker()` terminates the worker and the host
   * then fails every in-flight request with `Internal` (ADR 0015) -- correctly, because from
   * the host's point of view the operation did not finish. But a person who pressed Stop did
   * not have anything go wrong, and telling them "something inside burrow failed" for doing
   * what the button offered is the interface lying about its own state. Measured in
   * `e2e/merge-pdf.spec.ts`: the notice appeared after every cancel.
   *
   * So the reply is ignored rather than the failure suppressed -- the difference matters, and
   * a bare `if (phase !== "working") return` would also swallow a genuine failure that
   * arrived a moment late.
   */
  let currentMerge = 0;

  async function merge() {
    if (!canMerge) return;
    clearResult();
    notice = null;
    phase = "working";
    const mine = ++currentMerge;
    announce("Merging.");

    try {
      const h = await ensureHost();

      // CANCELLED BEFORE THERE WAS ANYTHING TO CANCEL. `cancel()` calls `host?.discardWorker()`,
      // and during the first merge on a page `host` is still null while the worker bundle is
      // being fetched -- so Stop was a no-op and the operation was posted a moment after the
      // person stopped it. The generation is the record of the cancel either way, so it is
      // read again here, on the far side of the await. Found by security review.
      if (mine !== currentMerge) {
        phase = "idle";
        return;
      }

      const reply = await h.run(
        {
          op: "merge",
          blob: entries[0].file,
          blobs: entries.map((e) => e.file),
          password: null,
          limits: LIMITS,
        },
        { maxDurationMs: LIMITS.maxDurationMs },
      );

      // The person stopped this one and has already been told so. Its answer is not news.
      if (mine !== currentMerge) return;

      if (!reply.ok) {
        // A request the page cancelled is not news: `cancel()` has already said "Stopped."
        // This is the pre-post case the generation guard above cannot see, because a
        // cancelled queued request never got a generation of its own.
        if (reply.kind === CANCELLED) {
          phase = "idle";
          return;
        }
        notice = messageFor({ ...reply, kind: hostKind(reply.kind) });
        markProblemFile(notice);
        phase = "idle";
        announce(notice.title);
        return;
      }

      if (!reply.output) {
        notice = messageFor({ kind: "Internal" });
        phase = "idle";
        return;
      }

      // An object URL over the Blob. The bytes stay in the Blob — the page never reads them
      // into its own heap, and `clearResult` revokes the URL so the browser can release
      // them the moment the person is done.
      result = {
        url: URL.createObjectURL(reply.output),
        name: suggestedName(),
        pages: reply.pages,
      };
      phase = "done";
      announce(`Done. ${reply.pages} pages, ready to download.`);
    } catch {
      if (mine !== currentMerge) return;
      notice = messageFor({ kind: "Internal" });
      phase = "idle";
    }
  }

  /** Attribute a failure to the file it came from, so the list can show it. */
  function markProblemFile(message: Message) {
    if (message.file < 0 || message.file >= entries.length) return;
    entries = entries.map((e, i) => (i === message.file ? { ...e, problem: message } : e));
  }

  function suggestedName(): string {
    const first = entries[0]?.file.name.replace(/\.pdf$/i, "") ?? "merged";
    return `${first}-merged.pdf`;
  }

  /**
   * Stop the operation in flight.
   *
   * `discardWorker()` terminates the worker, which is the only thing that CAN stop work
   * inside an engine call — no engine here offers a cancellation hook (ADR 0007). A person
   * changing their mind is not the watchdog's hang case, and it must not count as a crash:
   * the breaker counts crashes, not respawns, precisely so a cancel does not take the page
   * offline (ADR 0015 §3).
   */
  function cancel() {
    // BEFORE the worker goes, so the reply that the discard provokes is already stale when it
    // arrives rather than racing this line.
    currentMerge += 1;
    host?.discardWorker();
    phase = "idle";
    notice = null;
    announce("Stopped.");
  }

  /** The deliberate gesture that closes the circuit breaker. */
  function startAgain() {
    host?.reset();
    notice = null;
    announce("Ready to try again.");
  }

  /**
   * Put a completed merge behind us, because the list it described has changed.
   *
   * Only from `"done"`: a merge in flight is not finished, and a refusal still on screen is
   * still the answer to what is in the list until something about the list moves.
   */
  function finished() {
    if (phase !== "done") return;
    clearResult();
    phase = "idle";
  }

  function clearResult() {
    if (result) URL.revokeObjectURL(result.url);
    result = null;
  }

  function reset() {
    clearResult();
    notice = null;
    phase = "idle";
  }

  function announce(text: string) {
    // Reassigned even when identical, so a repeated action is still read out.
    announcement = "";
    queueMicrotask(() => (announcement = text));
  }

  onDestroy(() => {
    clearResult();
    host?.dispose();
  });

  // ---------------------------------------------------------------- drag and drop

  let dragging = $state(false);

  function onDrop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    const dropped = event.dataTransfer?.files;
    if (dropped && dropped.length > 0) void add(dropped);
  }
</script>

<section class="tool" aria-labelledby="tool-heading">
  <h2 id="tool-heading" class="visually-hidden">Merge your files</h2>

  <!-- THE DROP ZONE HAS A REAL FILE INPUT INSIDE IT, not a click handler that opens one.
       A label wrapping an input is reachable by keyboard and by a screen reader without
       anything being simulated, which is what the definition of done means by "the drop
       zone has a file-input fallback". -->
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
      multiple
      onchange={(e) => {
        const input = e.currentTarget as HTMLInputElement;
        if (input.files) void add(input.files);
        // Cleared so choosing the same file twice still fires a change.
        input.value = "";
      }}
    />
    <span class="drop__text">Drop PDFs here, or choose files</span>
  </label>

  {#if entries.length > 0}
    <ol class="files" role="list">
      {#each entries as entry, index (entry.id)}
        <li class="file" class:file--bad={entry.problem !== null}>
          <span class="file__position" aria-hidden="true">{index + 1}</span>
          <span class="file__name">{entry.file.name}</span>
          <span class="file__pages">
            {#if entry.problem}
              <span class="refusal">cannot be used</span>
            {:else if entry.pages === null}
              counting…
            {:else}
              <span class="measure">{entry.pages}</span>
              {entry.pages === 1 ? "page" : "pages"}
            {/if}
          </span>
          <span class="file__actions">
            <button
              type="button"
              onclick={() => move(index, -1)}
              disabled={index === 0 || phase === "working"}
              aria-label={`Move ${entry.file.name} up`}>↑</button
            >
            <button
              type="button"
              onclick={() => move(index, 1)}
              disabled={index === entries.length - 1 || phase === "working"}
              aria-label={`Move ${entry.file.name} down`}>↓</button
            >
            <button
              type="button"
              onclick={() => remove(index)}
              disabled={phase === "working"}
              aria-label={`Remove ${entry.file.name}`}>Remove</button
            >
          </span>
          {#if entry.problem}
            <p class="file__problem">
              <span class="refusal">{entry.problem.title}</span>
              {entry.problem.next}
            </p>
          {/if}
        </li>
      {/each}
    </ol>

    <p class="total">
      {#if totalPages !== null}
        <span class="measure">{totalPages}</span>
        {totalPages === 1 ? "page" : "pages"} in total.
      {:else if unusable.length > 0}
        <span class="refusal">Merge is unavailable until every file can be read.</span>
      {:else}
        Counting pages…
      {/if}
    </p>
  {/if}

  {#if notice}
    <p class="notice" role="alert">
      <span class="refusal">{notice.title}</span>
      {notice.next}
    </p>
  {/if}

  <div class="actions">
    {#if phase === "working"}
      <!-- NO PROGRESS BAR. The merge is one engine call per page inside a worker and
           reports nothing until it finishes, so a bar would be an animation pretending to
           be a measurement. The honest thing is to say what is happening and that the rest
           is not observable. -->
      <p class="working" role="status">
        Merging {entries.length} files. This is not reportable while it runs — burrow will say when it
        is done.
      </p>
      <button type="button" onclick={cancel}>Stop</button>
    {:else if notice && !notice.retryable}
      <button type="button" onclick={startAgain}>Start again</button>
    {:else}
      <button type="button" onclick={merge} disabled={!canMerge}>Merge</button>
    {/if}
  </div>

  {#if result}
    <p class="result">
      <a class="download" href={result.url} download={result.name}>Download {result.name}</a>
      <span class="result__detail">
        <span class="measure">{result.pages}</span>
        {result.pages === 1 ? "page" : "pages"}. It stays on this computer.
      </span>
    </p>
  {/if}

  <!-- Announcements for a screen reader. Visually hidden rather than absent: the list and
       the state are visible, and this is how the same changes reach someone who is not
       watching them. -->
  <p class="visually-hidden" role="status" aria-live="polite">{announcement}</p>
</section>

<style>
  .tool {
    margin-block: var(--space-6);
    max-width: var(--track-readout);
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  /* A surface you can act on, so `--edge` rather than `--rule`. See tokens.css. */
  .drop {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 7rem;
    padding: var(--space-5);
    border: 2px dashed var(--edge);
    border-radius: var(--radius);
    cursor: pointer;
  }

  .drop--over {
    border-style: solid;
  }

  /* The input is the control; it is the LABEL that is drawn. Not `display: none`, which
     would take it out of the tab order along with its accessible name. */
  .drop input {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
  }

  /* The focus ring belongs on the thing that is drawn, because the input is invisible. */
  .drop:focus-within {
    outline: var(--focus-width) solid var(--ink);
    outline-offset: var(--focus-offset);
  }

  .drop__text {
    font-weight: var(--weight-strong);
  }

  .files {
    margin: var(--space-5) 0 0;
    padding: 0;
    list-style: none;
  }

  /* Numbered, and here the numbers EARN it: merge order is the whole point of the tool, so
     the position is information rather than decoration. */
  .file {
    display: grid;
    grid-template-columns: 2rem 1fr auto auto;
    align-items: baseline;
    gap: var(--space-3);
    padding-block: var(--space-3);
    border-block-start: 1px solid var(--rule);
  }

  .file--bad {
    border-inline-start: 3px solid var(--refuse);
    padding-inline-start: var(--space-3);
  }

  .file__position {
    color: var(--ink-quiet);
    font-variant-numeric: tabular-nums;
  }

  .file__name {
    overflow-wrap: anywhere;
  }

  .file__pages {
    color: var(--ink-quiet);
    font-size: var(--step--1);
    white-space: nowrap;
  }

  .file__actions {
    display: flex;
    gap: var(--space-2);
  }

  .file__problem {
    grid-column: 1 / -1;
    margin: var(--space-2) 0 0;
    font-size: var(--step--1);
  }

  button {
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: var(--step--1);
    cursor: pointer;
  }

  button:disabled {
    color: var(--ink-quiet);
    border-color: var(--rule);
    cursor: default;
  }

  .actions {
    margin-block-start: var(--space-5);
    display: flex;
    align-items: baseline;
    gap: var(--space-4);
    flex-wrap: wrap;
  }

  .actions button {
    font-size: var(--step-0);
    padding: var(--space-3) var(--space-5);
  }

  .total,
  .notice,
  .working,
  .result {
    margin-block-start: var(--space-4);
  }

  .result {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    flex-wrap: wrap;
  }

  .result__detail {
    color: var(--ink-quiet);
    font-size: var(--step--1);
  }

  .download {
    font-weight: var(--weight-strong);
  }
</style>
