---
name: user-role
description: burrow's maintainer — sets precise, pre-analysed security review scopes and wants only demonstrated findings
metadata:
  type: user
---

The user is burrow's author/maintainer (git user `anugram`, mr.anurag.jain@gmail.com) and
commissions security reviews on their own work, one PR at a time.

How they brief a review:

- They name the exact files and, per file, the specific mechanisms they are unsure about
  (e.g. "the `manifest()` heredoc that emits tab-separated fields into `read -r`"). They
  have usually already reasoned about the obvious attack and tell you the result
  ("I verified `LD_LIBRARY_PATH` cannot shadow the pinned library, but consider other
  vectors"). Do not re-derive what they state; go after the vector they did not name.
- They ask 2-4 numbered yes/no questions at the end. Answer each one explicitly, with the
  evidence, including when the answer is "yes, here is the exact path".
- They pre-weight severity themselves ("this is dev/CI-only tooling, weight accordingly")
  and explicitly ask for "only real problems or things that carry into production". Scanner
  output and unexploitable hardening gaps are unwelcome; a demonstrated failure is welcome
  regardless of how small the blast radius is.

Empirical proof lands much better than reasoning. `readelf -d` on the actual built test
binary, a reduced bash repro of an exit-status bug, `nm -u` on the produced archive — these
answered their questions definitively where argument would not have.

See [[review-in-progress-edits]] and [[m1-engine-supply-chain]].
