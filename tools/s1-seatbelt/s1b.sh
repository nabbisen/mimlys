#!/bin/bash
# SPIKE S1b driver (branch s1b-macos-seatbelt only, never merged). Runs B1..B5 on a macOS runner;
# every result line starts with "S1b ". Never fails the job on a NEGATIVE result: the log is the
# finding. It fails (at the very end) only when the harness itself could not be built.
# Usage: s1b.sh "<tokens>"   e.g. "b1 b2 b4 b3 b5"
set +e
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$(cd -P /tmp && pwd)/s1b"          # /private/tmp/s1b: canonical
rm -rf "$S"; mkdir -p "$S/bin" "$S/ws/tmp" "$S/prof"
tmo() { perl -e 'alarm shift; exec @ARGV' "$@"; }
hr() { echo; echo "::group::$*"; echo "S1b ===== $* ====="; }
end() { echo "::endgroup::"; }
has() { case " $1 " in *" $2 "*) return 0;; esac; return 1; }
BUILDFAIL=""

echo "S1b machine: $(sw_vers -productName) $(sw_vers -productVersion) build $(sw_vers -buildVersion); $(uname -m); user $(id -un) uid $(id -u)"
echo "S1b tokens: $1"
clang -O0 -Wno-deprecated-declarations -o "$S/bin/probe" "$HERE/probe.c" || { echo "S1b build: probe.c FAILED"; BUILDFAIL=1; }
clang -O0 -Wno-deprecated-declarations -Wall -o "$S/bin/probeb" "$HERE/probe_b.c" || { echo "S1b build: probe_b.c FAILED"; BUILDFAIL=1; }
P="$S/bin/probe"; PB="$S/bin/probeb"

# ---- profiles ---------------------------------------------------------------
base() { cat <<EOF
(version 1)
(deny default)
(allow process-fork)
(allow process-exec*)
(allow file-read-metadata)
(allow file-read* (subpath "/usr/lib") (subpath "/usr/bin") (subpath "/bin") (subpath "/System")
  (subpath "/private/var/db/dyld") (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null")
  (literal "/dev/zero") (subpath "$S"))
(allow file-write* (subpath "$S/ws") (literal "/dev/null"))
(allow sysctl-read)
(allow signal (target self))
EOF
}
mk() { name=$1; shift; { base; printf '%s\n' "$@"; } > "$S/prof/$name.sb"; }
SIG='(allow signal (target same-sandbox))'
PDENY='(deny process-info*)'
PALLOW='(allow process-info* (target same-sandbox))'
PRC='(deny sysctl-read (sysctl-name-prefix "kern.procargs"))'

# ---- B1 ---------------------------------------------------------------------
if has "$1" b1 && [ -x "$PB" ]; then
hr B1 kill-1 inside the profile
mk b1p1 "$SIG"
mk b1p2 "$SIG" "$PDENY" "$PALLOW" "$PRC"
mk b1p3 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids)' '(deny sysctl-read (sysctl-name-prefix "kern.proc."))'
tmo 600 sleep 600 & BY=$!
sleep 1
G="1:launchd,$BY:bystander"
pid=$$
while [ -n "$pid" ] && [ "$pid" -gt 1 ]; do
  nm=$(ps -o comm= -p "$pid" 2>/dev/null | sed 's/.*\///; s/[^A-Za-z0-9_.]/_/g')
  G="$G,$pid:ancestor_$nm"
  pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
done
for p in $(pgrep -x Runner.Listener) $(pgrep -x Runner.Worker); do G="$G,$p:runner_agent"; done
echo "S1b b1.guards: $G"
echo "S1b b1.job_shell: pid $$; runner processes: $(pgrep -lx 'Runner.Listener|Runner.Worker' | tr '\n' ' ')"
for vf in p3:oracle p2:helper; do
  v=${vf%%:*}; fl=${vf#*:}
  case $v in
    p2) d="base + signal same-sandbox + process-info members only + kern.procargs deny";;
    p3) d="p2 + (allow process-info-listpids) + (deny sysctl-read kern.proc.) (the listing-oracle shape)";;
  esac
  echo "S1b b1: ---- profile $v ($d), flavor $fl"
  tmo 180 "$PB" b1 "$S/prof/b1$v.sb" "$S/ws" "$G" "$v-$fl" "$fl" > "$S/b1-$v-$fl.txt" 2>&1
  cat "$S/b1-$v-$fl.txt"
  sleep 1
  echo "S1b b1.post[$v-$fl]: the job shell (pid $$) is alive: it is running this line"
  for tok in ${G//,/ }; do
    gp=${tok%%:*}
    if ps -p "$gp" -o pid= >/dev/null 2>&1; then st=alive; else st="GONE"; fi
    echo "S1b b1.post[$v-$fl] guard $tok: $st"
  done
  echo "S1b b1.post[$v-$fl]: runner processes now: $(pgrep -lx 'Runner.Listener|Runner.Worker' | tr '\n' ' ')"
  pids=$(sed -n 's/.*tree built=yes pids: T=\([0-9]*\) C=\([0-9]*\) G1=\([0-9]*\) G2=\([0-9]*\).*/\1 \2 \3 \4/p' "$S/b1-$v-$fl.txt" | head -1)
  if [ -n "$pids" ]; then
    echo "S1b b1.post[$v-$fl]: tool tree pids (T C G1 G2) = $pids; ps from outside: $(for q in $pids; do ps -p $q -o pid=,ppid=,stat=,comm= 2>/dev/null; done | tr '\n' ';') (empty = all gone)"
  else
    echo "S1b b1.post[$v-$fl]: no tree pids were logged (the pre-check failed or the tree was not built)"
  fi
  echo "S1b b1.post[$v-$fl]: probeb processes left on the machine: $(pgrep -lf 'probeb b1' | tr '\n' ';') (empty = none)"
  pkill -f 'probeb b1' 2>/dev/null
done
kill $BY 2>/dev/null
end
fi

# ---- B2 ---------------------------------------------------------------------
if has "$1" b2 && [ -x "$PB" ]; then
hr B2 listing restricted to the sandbox
L1='(allow process-info-listpids (target same-sandbox))'
mk b2v1 "$SIG" "$PDENY" "$PALLOW" "$PRC"
mk b2v2 "$SIG" "$PDENY" "$PALLOW" "$PRC" "$L1"
mk b2v3 "$SIG" "$PDENY" "$PRC" '(allow process-info-pidinfo (target same-sandbox))' '(allow process-info-rusage (target same-sandbox))' "$L1"
mk b2v4 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids (target self))'
mk b2v5 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids (target pgrp))'
mk b2v6 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids (target children))'
mk b2v7 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids)'
mk b2v8 "$SIG" "$PDENY" "$PALLOW" "$PRC" '(allow process-info-listpids)' '(deny sysctl-read (sysctl-name-prefix "kern.proc."))'
mk b2v9 "$SIG" "$PDENY" "$PALLOW" '(deny sysctl-read (sysctl-name-prefix "kern.proc"))'
mk b2v0 "$SIG" '(allow process-info*)'
for v in v1 v2 v3 v4 v5 v6 v7 v8 v9 v0; do
  case $v in
    v1) d="Amendment 2 shape (deny process-info*, allow same-sandbox, procargs deny), no listing rule";;
    v2) d="v1 + (allow process-info-listpids (target same-sandbox))";;
    v3) d="deny process-info*, then only pidinfo, rusage and listpids for (target same-sandbox)";;
    v4) d="v1 + (allow process-info-listpids (target self))";;
    v5) d="v1 + (allow process-info-listpids (target pgrp))";;
    v6) d="v1 + (allow process-info-listpids (target children))";;
    v7) d="v1 + (allow process-info-listpids) for every target (the membership-oracle shape)";;
    v8) d="v7 + (deny sysctl-read (sysctl-name-prefix kern.proc.))";;
    v9) d="v1 + (deny sysctl-read (sysctl-name-prefix kern.proc)) (kern.proc covers procargs)";;
    v0) d="control: (allow process-info*) for every target";;
  esac
  echo "S1b b2: ---- variant $v: $d"
  tmo 120 "$PB" b2 "$S/prof/b2$v.sb" "$v" > "$S/b2-$v.txt" 2>&1
  cat "$S/b2-$v.txt"
done
end
fi

# ---- B4 ---------------------------------------------------------------------
if has "$1" b4 && [ -x "$PB" ]; then
hr B4 UDP under deny network
cat > "$S/udp.py" <<'EOF'
import select, socket, sys
v4 = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
v4.bind(("127.0.0.1", 0))
port = v4.getsockname()[1]
socks = [v4]
try:
    v6 = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
    v6.bind(("::1", port))
    socks.append(v6)
except Exception:
    pass
open(sys.argv[1], "w").write(str(port))
log = open(sys.argv[2], "a")
while True:
    r, _, _ = select.select(socks, [], [])
    for s in r:
        d, a = s.recvfrom(1024)
        log.write(("v6 " if s is not v4 else "v4 ") + d.decode(errors="replace") + "\n")
        log.flush()
EOF
python3 "$S/udp.py" "$S/udp.port" "$S/udp.log" & LP=$!
sleep 1
PORT=$(cat "$S/udp.port"); echo "S1b b4.listener: UDP 127.0.0.1:$PORT (and ::1 if available), pid $LP"
UNIX='(allow network-bind (local unix-socket (subpath "'$S'/ws")))
(allow network-outbound (remote unix-socket (subpath "'$S'/ws")))
(allow network-inbound (local unix-socket (subpath "'$S'/ws")))'
mk u0
mk u1 '(deny network*)' "$UNIX"
mk u2 '(allow network*)'
for v in none:- u0:u0 u1:u1 u2:u2; do
  tag=${v%%:*}; pf=${v#*:}
  case $tag in
    none) d="control, no sandbox";;
    u0) d="(deny default) with no network rule";;
    u1) d="(deny network*) + unix sockets under the workspace (the Amendment 2 network profile)";;
    u2) d="control: (allow network*)";;
  esac
  echo "S1b b4: ---- $tag: $d"
  [ "$pf" = "-" ] && prof="-" || prof="$S/prof/$pf.sb"
  tmo 60 "$PB" b4 "$prof" "$PORT" "$tag" > "$S/b4-$tag.txt" 2>&1
  cat "$S/b4-$tag.txt"
  sleep 1
  echo "S1b b4.listener_saw[$tag]: $(grep -c "S1b-$tag\$" "$S/udp.log" 2>/dev/null) datagram(s): $(grep "S1b-$tag\$" "$S/udp.log" 2>/dev/null | tr '\n' ';')"
done
kill $LP 2>/dev/null
end
fi

# ---- B3, B5 -----------------------------------------------------------------
WHAT=""
has "$1" b3 && WHAT="b3"
if has "$1" b5; then WHAT="${WHAT:+$WHAT,}b5"; fi
if [ -n "$WHAT" ] && [ -x "$P" ]; then
hr B3 B5 iterative read set, mach services, necessity, dependency step "($WHAT)"
S1_DIR="$S" HERE="$HERE" S1B_WHAT="$WHAT" tmo 5700 python3 "$HERE/iterate_b.py" 2>&1 | cat
end
fi

echo "S1b finished; build failures: ${BUILDFAIL:-none}"
[ -n "$BUILDFAIL" ] && exit 1
exit 0
