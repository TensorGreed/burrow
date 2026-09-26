---
name: cited-test-name-must-exist
description: A comment that cites a test by its name ("`X` pins it deterministically") may point at a test that never existed; rg the name when the diff touches that comment.
metadata:
  type: feedback
---

When a diff rewrites a comment that names a test as the witness for a property, rg the quoted
test name across the tree. In #107's restore (2026-09-26) SplitTool/RotateTool cited
`the render engine is not brought back while a split is running` in three places; no such test
existed anywhere (introduced as a comment-only reference in 902cb7b), and open issue #114 says
no test proves that property. The rewrite kept the false citation.

**Why:** a named witness reads as coverage; the reader does not go looking.
**How to apply:** for every backticked test title in touched comments/docs, `rg -F` it; also
check whether an open issue says the opposite. Related: [[probe-reimplements-rule]].
