// The one console line that is not ours, and cannot be suppressed.
//
// Extracted from `e2e/console-silence.spec.ts` when `e2e/merge-pdf.spec.ts` needed to assert
// the same silence against the real tool page. Shared as a module rather than by importing one
// spec from another, which would register that spec's tests twice.

/**
 * **Firefox reports the fail-closed guard's own probe to the page console**, once per worker
 * spawn, measured in M1 PR 4a-ii:
 *
 *     Content-Security-Policy: The page's settings blocked the loading of a resource
 *     (connect-src) at http://localhost:4321/__csp-probe because it violates the following
 *     directive: "connect-src …"
 *
 * **WebKit does the same, in different words** — measured in the same run:
 *
 *     Refused to connect to http://localhost:4321/__csp-probe because it does not appear in
 *     the connect-src directive of the Content Security Policy.
 *
 * Note the unhyphenated spelling. The first version of this filter matched only Firefox's
 * "Content-Security-Policy" and WebKit failed, which is a reminder that the recognisable part
 * of these messages is the **probe path**, not the prose.
 *
 * There is no way to avoid any of it: the guard establishes that a policy is in force by
 * making a request the policy MUST refuse, and a browser logs refusals. The exclusion is
 * deliberately narrow rather than a category — it names the guard's own probe URL, so a
 * message that does not is not one of these and is not excluded.
 */
export function isBrowserPolicyReport(line: string): boolean {
  return /content.security.policy/i.test(line) && line.includes("/__csp-probe");
}
