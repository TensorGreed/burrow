---
name: timing-witness-decays-under-load
description: A timing plant that "witnesses" a defence can go green-with-defence-removed on a slow machine; re-run the ablation under CPU load, not just the test
metadata:
  type: feedback
---

When a plant is timed to witness a defence (#137 R8: a copy 700 ms after THE_CALL vs a
500+500 ms settle), the plant itself rarely flakes red; what decays is the WITNESS. Measured on
cf4bf34: chromium's redaction ran 42-67 ms unloaded, 115-142 ms with 40 spinners on 20 cores.
The witness only holds while redaction + settle round trip < 200 ms. With 60 spinners, removing
the settle's captured timer (D9) left every spec green, and removing the page wait lost one of
its two witnesses. Unloaded, both went red.

Also on that commit: an exemption for the harness's own messages with no COUNT is a channel.
200 `{__burrowSideChannelArmed: [EXIT names]}` messages carried 1400 input bytes past R8 and R9
in all three browsers (`name in EXITS` also admits "toString"). And a hand-written log that
flips `ok` without its `shape` "witnesses" a rule the exact-shape rule already makes
unreachable: a belt counted as a rule.

**Why:** a timing witness is keyed to the machine's speed; an allowlisted message is only as
narrow as its count and alphabet.
**How to apply:** anchor timing plants to the reply, not to the start of the work; spin CPUs
(`timeout N sh -c 'while :; do :; done' &` x 3*nproc) and re-run the defence ablation. For each
exemption, plant it N times carrying data. See [[harness-own-prefix-exemption]].
