// The test servers, and the request log that is the ground truth for what the browser did.
//
// WHY NOT `astro preview`
//
// ADR 0014 §3 splits the guarantee in two:
//
//   * no cross-origin request, enforced by the browser;
//   * **no request at all after engine init, enforced by test** — because CSP ignores query
//     strings, so `…/qpdf.<hash>.wasm?leak=<bytes>` matches the permitted source and no policy
//     that lets the engines load can close it.
//
// The second half needs to know every request that was actually made. `page.on("request")`
// cannot carry it: browser-reported network events for **dedicated workers** are not equally
// complete across engines, and the worker is the only place file bytes ever exist — so the
// half that matters is exactly the half the browser reports least reliably.
//
// A server's own accept log has no such gap. If a request reached it, it happened.
//
// THE SECOND ORIGIN
//
// `foreign` listens on another port and logs anything at all. Nothing should ever reach it —
// `default-src 'none'` with no cross-origin source anywhere means the browser refuses such
// requests before they are sent. That is the point: the test asserts the log is empty, so it
// fails if the policy is ever absent, misgenerated, or not inherited by the worker. A check
// that can only confirm what CSP already promises is worth having precisely when CSP stops
// delivering it.
//
// WHY THE LOG IS A FILE
//
// Playwright test workers are separate processes from this one, so the tests cannot read an
// in-memory array. A file is the simplest channel that works across processes, and it lets a
// test write its own marker line — which it does directly, because asking the server to record
// a marker would mean making a request, and a request is the thing being counted.

import { createReadStream } from "node:fs";
import { appendFile, mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const webApp = resolve(here, "..");

/** Where both servers append. Read by `e2e/zero-requests.spec.ts`. */
export const LOG_PATH = join(webApp, "test-results", "request-log.jsonl");

/**
 * Content types, stated explicitly rather than sniffed.
 *
 * `application/wasm` is load-bearing: `WebAssembly.compileStreaming` **rejects** a response
 * with any other type, so a server that guessed would fail the engine load with an error that
 * looks nothing like a MIME problem.
 */
const TYPES = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".txt", "text/plain; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".ico", "image/x-icon"],
  [".png", "image/png"],
  [".woff2", "font/woff2"],
]);

/**
 * @param {string} origin
 * @param {import("node:http").IncomingMessage} request
 */
async function record(origin, request) {
  await appendFile(
    LOG_PATH,
    `${JSON.stringify({
      origin,
      method: request.method,
      // The FULL url, query string included. The query string is the whole reason this log
      // exists — it is the channel CSP cannot close — so stripping it would remove the
      // evidence.
      url: request.url,
      at: Date.now(),
      // Which browser made it. Playwright runs the three projects in sequence against one
      // server, and an assertion that cannot tell them apart is an assertion about the wrong
      // run.
      agent: request.headers["user-agent"] ?? "",
    })}\n`,
  );
}

/**
 * Serve `root` on `port`, logging every request.
 *
 * @param {object} options
 * @param {number} options.port
 * @param {string} options.root
 * @param {string} options.origin A label for the log.
 */
export async function serveStatic({ port, root, origin }) {
  const server = createServer((request, response) => {
    void record(origin, request).then(async () => {
      try {
        // INSIDE the try, and that is not tidiness. `decodeURIComponent` throws `URIError` on
        // a malformed escape, and `GET /%` outside it became an unhandled rejection that
        // exited the process -- taking both servers down mid-run and failing every later spec
        // with a network error that said nothing about the cause. `zero-requests.spec.ts` is
        // the worst casualty, since this server's log IS its evidence.
        //
        // Query and fragment stripped only for RESOLVING the file. The log above kept them.
        const path = decodeURIComponent((request.url ?? "/").split("?")[0].split("#")[0]);
        let file = join(root, normalize(path));
        // `normalize` collapses `..`, and this rejects anything that still escapes. A test
        // server is still a server, and a traversal here would read the repository.
        if (!file.startsWith(root + sep) && file !== root) {
          response.writeHead(403).end();
          return;
        }
        let info = await stat(file);
        if (info.isDirectory()) {
          file = join(file, "index.html");
          info = await stat(file);
        }
        response.writeHead(200, {
          "content-type": TYPES.get(extname(file)) ?? "application/octet-stream",
          "content-length": info.size,
          // No caching, at all. A cached engine response would make "no request was made"
          // true for the wrong reason, which is the same trap ADR 0014 §1b describes for the
          // guard's control fetch.
          "cache-control": "no-store",
          "x-content-type-options": "nosniff",
        });
        createReadStream(file).pipe(response);
      } catch {
        response.writeHead(404, { "content-type": "text/plain" }).end("not found");
      }
    });
  });
  await new Promise((done) => server.listen(port, "127.0.0.1", () => done(undefined)));
  return server;
}

/**
 * A second origin that serves nothing and logs everything.
 *
 * @param {object} options
 * @param {number} options.port
 * @param {string} options.origin
 */
export async function serveForeign({ port, origin }) {
  const server = createServer((request, response) => {
    void record(origin, request).then(() => {
      // A response is sent so a request that DOES arrive completes rather than hanging — the
      // test should fail on the log entry, not time out and leave the reason to guesswork.
      response.writeHead(204, { "access-control-allow-origin": "*" }).end();
    });
  });
  await new Promise((done) => server.listen(port, "127.0.0.1", () => done(undefined)));
  return server;
}

/** Start both servers with an empty log. Called by Playwright's `globalSetup`. */
export async function startServers({ port, foreignPort, root }) {
  await mkdir(dirname(LOG_PATH), { recursive: true });
  await writeFile(LOG_PATH, "");
  return {
    site: await serveStatic({ port, root, origin: `http://localhost:${port}` }),
    foreign: await serveForeign({ port: foreignPort, origin: `http://localhost:${foreignPort}` }),
  };
}
