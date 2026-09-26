---
name: funnelled-check-plant-inert
description: When a refactor collapses many scattered checks (qpdf error drains, bounds checks) into one trait method, plant that one method inert; #191's drain and page() bound both survived the whole suite.
metadata:
  type: feedback
---

When a refactor replaces N inline checks with one choke-point method (e.g. #191: ~60 `document.take_error()` sites became `PdfObject::drained`, three `unsafe` page lookups became the bounds-checked `PdfDocument::page`), make that method inert in a worktree and run the native suites plus the golden. In #191 both mutations survived all 1,107 tests: `drained` returning `Ok(())` and `page` dropping its bound (callers pre-check, so the check guarding the old `unsafe` was never reached).

**Why:** a golden that pins outcomes cannot see a drain that goes quiet when no corpus document latches an error there, and a defence-in-depth check that every caller pre-empts never runs. The choke point makes the gap testable for the first time, so a unit test is cheap to ask for.

**How to apply:** for any "moved behind a trait" refactor, list each new single-site check and plant it inert. Ask for a direct unit test (latch an error with `key` on a free-standing null -- one with no owning document; an absent-key null only warns -- then assert `drained()` is Err; call `page(count)`, then assert InvalidArgument). Related: [[inert-detector-path]], [[mutate-the-wiring-not-the-policy]].
