---
name: worker-scope-stubs-nested-worker
description: Stubs installed in a blob: worker's own scope miss a nested Worker spawned from self.location.href; plant one before trusting any "no exit taken" check
metadata:
  type: feedback
---

When a check watches a worker's exits by stubbing APIs in the worker's scope (R9's
`redaction-no-side-channel.spec.ts`, #137), plant a copy that does
`new Worker(self.location.href, { name: "n" })` and has the child take the exit. Measured
2026-09-27 on 5c3c174: in all three browsers the child (no `createObjectURL` call needed, the
parent's own blob URL is reused) broadcast the full input to the page via BroadcastChannel;
its stub reports went to the parent, not the recorder, and R8 and R9 both returned []. The ADR
bullet said nested workers were "refused at one remove" because a blob: URL needs
`createObjectURL`, which is false.

Also: byte counting by type (ArrayBuffer/view/Blob) misses `Array.from(bytes)` everywhere and
`ImageData` in Firefox, and an id-based predicate should be probed with a *foreign* id and
`id: undefined`, not only null and the request's id.

**Why:** a stub-in-scope design sees only the scope it was installed in; the recorder sees
only the parent's postMessage.
**How to apply:** for any in-worker instrumentation, try a nested worker, a foreign id, and a
non-binary encoding before accepting "SHOWN TO FAIL" as coverage. See [[probe-reimplements-rule]].
