---
name: harness-own-prefix-exemption
description: A recorder that exempts its own tagged messages by key PREFIX exempts the product's messages too; plant a same-prefix key carrying bytes, then make each predicate rule inert
metadata:
  type: feedback
---

When a message-log check skips "the harness's own" messages, plant a product message that
matches the skip test while carrying the payload. Measured on 9d9ec66 (#137, R8):
`harnessOwn` skipped any single-key message whose key `startsWith("__burrow")`, and so
`{ __burrowChunk: bytes }` and `{ __burrowSettled: bytes }` passed R8 with zero findings in all
three browsers. The bundle's own globals are named `__burrow_*`, so the prefix is the
product's naming convention as well as the harness's.

Also on that commit: making seven of r8/r9's rules inert one at a time (no-terminal-reply,
second reply, unlisted terminal fields, terminal port, refusal-with-bytes, success with 0
bytes, R9's settle rule) and the array-length branch of `longestRun` left every spec green.
Only the rules a plant was written *for* were witnessed.

**Why:** a positive case per plant is not a positive case per rule; the gap only shows when
each rule is disabled in turn.
**How to apply:** for any predicate with N `found.push` sites, disable each and run the specs;
list the survivors. See [[worker-scope-stubs-nested-worker]], [[funnelled-check-plant-inert]].
