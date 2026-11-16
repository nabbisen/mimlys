# Handoff 009h — Acceptance checklist (0.11, phase 1)

- [ ] A destination is admitted only when its parse serializes back to the same text and holds no user information; otherwise `destination_not_canonical` with the canonical form (A-12); the server receives exactly the judged string
- [ ] `sanitize` removes the default-ignorable and missing format characters; visible symbols stay (A-25)
- [ ] A server's standard error is bounded per line and by rate, the bounds proposed for registration (A-27)
- [ ] The pin is compared on the connection the call uses; `dirty` is cleared before the listing (A-29 a, b)
- [ ] A failed start counts against LIM-X3, and the reason keeps the last start's (A-30)
- [ ] Omitted content blocks carry `content_omitted{count}`; a server runs in a session of its own (A-39)
- [ ] An unanswering server costs one LIM-O6 per step
- [ ] The stack-margin guard runs in debug CI at a quarter of the stack
- [ ] Tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG; linear history
