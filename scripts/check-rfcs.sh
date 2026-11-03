#!/usr/bin/env bash
# RFC 000 invariants: Status matches the folder, numbers are unique, the index
# lists every RFC, and links resolve; every handoff directory belongs to an RFC
# of the same number and holds no lifecycle state of its own (repository-guards.md §2).
# Run from the repository root.
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
# Every relative link in rfcs/**/*.md resolves. Fenced code and inline code
# are skipped: RFC 000 shows example links that are not meant to exist.
while IFS= read -r -d '' f; do
  dir="$(dirname "$f")"
  while IFS= read -r target; do
    case "$target" in http://*|https://*|mailto:*|\#*|'') continue ;; esac
    path="${target%%#*}"
    [ -z "$path" ] && continue
    [ -e "$dir/$path" ] || { echo "broken link in $f: $target" >&2; fail=1; }
  done < <(awk '/^[[:space:]]*```/ { code = !code; next } !code { print }' "$f" \
             | sed -E 's/`[^`]*`//g' | grep -oE '\]\([^) ]+' | sed -E 's/^\]\(//' || true)
done < <(find rfcs -name '*.md' -print0)
# Every handoff directory belongs to an RFC of the same number (in any state
# folder) and holds no state folder of its own: its state is its RFC's.
for d in rfcs/handoffs/*/; do
  [ -d "$d" ] || continue
  name="$(basename "$d")"
  num="${name%%[!0-9]*}"
  if [ -z "$num" ] || ! compgen -G "rfcs/*/$num-*.md" > /dev/null; then
    echo "handoff $name has no RFC numbered ${num:-?}" >&2; fail=1
  fi
  for state in proposed accepted done archive draft; do
    [ -e "$d$state" ] && { echo "handoff $name holds a state folder: $state" >&2; fail=1; }
  done
done
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "ok: rfcs/ invariants hold"
