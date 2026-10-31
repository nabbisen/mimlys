#!/usr/bin/env bash
# Every REQ-, LIM- and THR- identifier cited in the maintainers' documents and
# the RFCs must be defined in its register (REQ-TEST-014). Run from the root.
set -euo pipefail
docs='docs/src/maintainers'
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
grep -oE '^### REQ-[A-Z]+-[0-9]{3}' "$docs/requirements.md" | sed 's/^### //' | sort -u > "$tmp/req.def"
grep -oE '^\| LIM-[A-Z][0-9]+ ' "$docs/limits-and-defaults.md" | sed -E 's/^\| //; s/ $//' | sort -u > "$tmp/lim.def"
grep -oE '^\| THR-[0-9]{2} ' "$docs/threat-register.md" | sed -E 's/^\| //; s/ $//' | sort -u > "$tmp/thr.def"
files="$(ls "$docs"/*.md rfcs/*/*.md rfcs/handoffs/*/*.md 2>/dev/null)"
grep -ohE 'REQ-[A-Z]+-[0-9]{3}' $files | sort -u > "$tmp/req.use"
grep -ohE 'LIM-[A-Z][0-9]+' $files | sort -u > "$tmp/lim.use"
grep -ohE 'THR-[0-9]{2}' $files | sort -u > "$tmp/thr.use"
fail=0
for kind in req lim thr; do
  missing="$(comm -13 "$tmp/$kind.def" "$tmp/$kind.use" || true)"
  if [ -n "$missing" ]; then echo "undefined $kind identifiers cited:" >&2; printf '%s\n' "$missing" >&2; fail=1; fi
done
# every requirement must appear in the external design's coverage table
missing="$(comm -23 "$tmp/req.def" <(grep -oE 'REQ-[A-Z]+-[0-9]{3}' "$docs/external-design.md" | sort -u) || true)"
if [ -n "$missing" ]; then echo "requirements absent from the external design:" >&2; printf '%s\n' "$missing" >&2; fail=1; fi
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "ok: $(wc -l < "$tmp/req.def") requirements, $(wc -l < "$tmp/lim.def") limits, $(wc -l < "$tmp/thr.def") threats; all citations resolve"
