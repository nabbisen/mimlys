# Handoff 012 — Acceptance checklist (0.9)

- [ ] REQ-MDL-009: the three values; `auto` resolved at the probe with the reason; no silent fallback; `status` shows the transport.
- [ ] Functions: deterministic schemas (golden), the `__` name mapping, invalid or colliding names unavailable with the reason; the pinned hash unchanged.
- [ ] REQ-DEC-003 (amended): exactly one structured call; every other shape `malformed`; no call read from text; text beside a call not in the context; `text_chars` recorded.
- [ ] External design §15.4: native turns with matching ids, labels intact, never a trailing `assistant` message; the budget counts `tools`; prefix stability for both transports.
- [ ] The M4 mock demonstration in both transports.
- [ ] The mutations of §3 are caught.
- [ ] Run B: five runs per engine with every measure; transcripts; nothing tuned.
- [ ] Documents and CHANGELOG; the model sentence changed only by run B's evidence.
- [ ] Gate; CI per platform with skips.
