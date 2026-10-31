#!/usr/bin/env bash
# RFC 000 invariants: Status matches the folder, numbers are unique, the index
# lists every RFC and links resolve. Run from the repository root.
set -euo pipefail
fail=0
declare -A expect=([proposed]='Proposed' [accepted]='Accepted' [done]='Implemented' [archive]='(Withdrawn|Superseded)')
declare -A seen
for state in proposed accepted done archive; do
  for f in rfcs/"$state"/*.md; do
    [ -e "$f" ] || continue
    num="$(basename "$f" | cut -d- -f1)"
    if [ -n "${seen[$num]:-}" ]; then echo "duplicate RFC number $num: $f and ${seen[$num]}" >&2; fail=1; fi
    seen[$num]="$f"
    status="$(grep -m1 -E '^\*\*Status\.\*\*' "$f" || true)"
    if ! printf '%s' "$status" | grep -Eq "^\*\*Status\.\*\* ${expect[$state]}"; then
      echo "status/folder mismatch in $f: '$status' (folder $state)" >&2; fail=1
    fi
    if ! grep -q "$(basename "$f")" rfcs/README.md; then echo "not listed in rfcs/README.md: $f" >&2; fail=1; fi
  done
done
# relative links inside rfcs/README.md must resolve
while read -r link; do
  [ -e "rfcs/$link" ] || { echo "broken link in rfcs/README.md: $link" >&2; fail=1; }
done < <(grep -oE '\]\(\./[^)]+\)' rfcs/README.md | sed -E 's/^\]\(\.\///; s/\)$//')
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "ok: rfcs/ invariants hold"
