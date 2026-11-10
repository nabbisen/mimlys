# Handoff 010b — A book page on the request-to-response flow (release 0.10)

**RFC.** [RFC 010](../../done/010-cli-and-operator-interface.md) (the operator interface and its book) · **Specification.** REQ-CLI-007's spirit (the book explains what users need to predict the product); external design §5, §6, §15 · **From.** The owner's direction of 2026-11-10, §1.3 and issue R11.

One page, "How a task runs", in the user's words, true of 0.10's code:
- what the user's goal becomes (never rewritten; questions only through `ask_user`);
- what the model is offered (visible tools only; the capability text; the transport);
- `plan` as a stored decision fed back at every step;
- what reaches the model at each step (the stable prefix, summarised history, the latest observation last; rejections that name what to change);
- how an action is checked (the twelve stages, approvals, confinement);
- how "done" is decided (the completion check, never the model's opinion);
- what persists, and how to continue (`fork`, `promote`);
- what is bounded today (per task; RFC 013 part two for the machine).

Link each claim to its reference page. Commands on the page are tested where the book's tests run.

Return: `.git-exclude/review-requests/010b-flow-page/README.md`.
