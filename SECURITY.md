# Security policy

burrow processes untrusted, potentially adversarial files on the user's own device, and
one of its intended uses is redaction. A parser bug is a memory-safety problem; a
redaction bug leaks secrets. We take both seriously.

## Reporting a vulnerability

**Please do not open a public issue, pull request, or discussion for a security
vulnerability.**

Report privately through GitHub's private vulnerability reporting:

1. Go to the [Security tab](https://github.com/TensorGreed/burrow/security/advisories/new).
2. Click **Report a vulnerability**.
3. Include the details below.

If that is not available to you, email the maintainers and we will open an advisory on
your behalf.

Helpful in a report:

- What kind of issue it is, and which component (core crate, engine wrapper, binding, app).
- A minimal reproducer. A crashing input file is ideal — please attach it rather than
  describe it. Say if the file contains anything sensitive and we will handle it
  accordingly, or work from a redacted variant.
- The version or commit, target platform, and how burrow was built.
- Your assessment of impact.

## What to expect

| | Target |
|---|---|
| Acknowledgement | 3 working days |
| Initial assessment | 10 working days |
| Fix or mitigation plan | depends on severity; we will tell you the timeline |
| Coordinated disclosure | up to 90 days from the report, sooner if a fix ships |

We will keep you updated, credit you in the advisory unless you prefer otherwise, and
tell you before we publish. If we conclude a report is not a vulnerability we will
explain why rather than close it silently.

burrow is pre-alpha with no releases yet, so there is nothing deployed to patch. Reports
against `main` are still welcome and will be fixed there.

## In scope

- Memory safety in any parser or engine wrapper: out-of-bounds access, use-after-free,
  uninitialised reads, integer overflow leading to any of these.
- Panics or aborts reachable from untrusted input, especially across an FFI boundary.
- Denial of service from crafted input: unbounded memory or CPU, decompression bombs,
  infinite loops — anything that escapes the configured `Limits`.
- **Redaction failures**: any way that content marked for removal survives in the output,
  including in text layers, embedded fonts, image data, annotations, incremental-update
  history, metadata, or attachments.
- **Privacy failures**: any code path reachable from user file content that makes a
  network call, writes content outside an intended destination, or emits content in
  telemetry, logs, or crash reports.
- Sandbox escapes in the web app, or unsafe handling of file content in the native apps.
- Supply-chain problems: a dependency with a license or provenance that violates
  [ADR 0003](docs/adr/0003-permissive-licensing.md), or a compromised build input.

## Out of scope

- Vulnerabilities in upstream engines (PDFium, qpdf, …) that we merely pass through —
  please report those upstream too, and tell us so we can pin or patch.
- Missing hardening that is not exploitable on its own.
- Reports produced solely by an automated scanner with no demonstrated impact.
- Attacks that require an already-compromised device or a malicious operating system.
- Social engineering, physical access, and issues in third-party hosting we do not control.

## Safe harbour

We will not pursue or support legal action against anyone who reports in good faith,
avoids privacy violations and service disruption, tests only against their own files and
installations, and gives us reasonable time to fix the issue before disclosing it.
