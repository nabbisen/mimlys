#!/bin/bash
# SPIKE S1 driver (branch s1-macos-seatbelt only). Runs Q1..Q9 on a macOS runner;
# every result line starts with "S1 ". Never fails the job: the log is the finding.
# Usage: s1.sh <questions>   e.g. "1 2 3 4 5 6 9 78"
set +e
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$(cd -P /tmp && pwd)/s1"          # /private/tmp/s1: canonical
rm -rf "$S"; mkdir -p "$S/bin" "$S/ws/tmp" "$S/prof"
clang -O0 -Wno-deprecated-declarations -o "$S/bin/probe" "$HERE/probe.c" || { echo "S1 build: FAILED"; exit 0; }
P="$S/bin/probe"
tmo() { perl -e 'alarm shift; exec @ARGV' "$@"; }
hr() { echo; echo "::group::$*"; echo "S1 ===== $* ====="; }
end() { echo "::endgroup::"; }

echo "S1 machine: $(sw_vers -productName) $(sw_vers -productVersion) build $(sw_vers -buildVersion); $(uname -m); user $(id -un)"
echo "S1 xcode-select: $(xcode-select -p 2>&1)"

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
mk q1
mk q2A '(allow file-read* (subpath "/usr/share"))'
cat > "$S/prof/q2B.sb" <<EOF
(version 1)
(deny default)
(allow process-fork)
(allow process-exec*)
(allow file-read-metadata)
(allow file-read* (subpath "/usr/lib") (subpath "/System") (subpath "/private/var/db/dyld") (literal "/dev/urandom") (literal "/dev/null") (subpath "$S") (subpath "/private/etc"))
(allow sysctl-read)
EOF

# ---- Q1 ---------------------------------------------------------------------
case " $1 " in *" 1 "*)
hr Q1 sandbox_init; "$P" selftest "$S/prof/q1.sb" "$S/ws" 2>&1 | cat; end;; esac

# ---- Q2 ---------------------------------------------------------------------
case " $1 " in *" 2 "*)
hr Q2 second profile; tmo 60 "$P" stack "$S/prof/q2A.sb" "$S/prof/q2B.sb" "$P" 2>&1 | cat; end;; esac

# ---- Q3 ---------------------------------------------------------------------
case " $1 " in *" 3 "*)
hr Q3 membership
env S1_SECRET=outside sleep 600 & OUT=$!
echo "S1 q3.outside: pid $OUT"
for variant in A B C; do
  case $variant in
    A) mk q3 "$SIG" '(allow process-info* (target same-sandbox))';;
    B) mk q3 "$SIG" '(allow process-info* (target same-sandbox))' '(allow process-info-listpids)';;
    C) mk q3 "$SIG" '(allow process-info-pidinfo (target same-sandbox))' '(allow process-info-rusage (target same-sandbox))' '(allow process-info-listpids)';;
  esac
  echo "S1 q3: ---- profile variant $variant ----"
  rm -f "$S"/ws/*.pid "$S"/ws/*.done
  ( tmo 60 "$P" q3 "$S/prof/q3.sb" w1 "$S/ws" w2 $OUT 2>&1 | sed "s/^S1 /S1 [w1,$variant] /" > "$S/q3-w1.txt" ) & W1=$!
  ( tmo 60 "$P" q3 "$S/prof/q3.sb" w2 "$S/ws" w1 $OUT 2>&1 | sed "s/^S1 /S1 [w2,$variant] /" > "$S/q3-w2.txt" ) & W2=$!
  wait $W1 $W2
  cat "$S/q3-w1.txt" "$S/q3-w2.txt"
  pgrep -fl "probe q3" | head -3
done
kill $OUT 2>/dev/null; end;; esac

# ---- Q4 ---------------------------------------------------------------------
case " $1 " in *" 4 "*)
hr Q4 JIT
mk jit0; mk jit1 '(allow dynamic-code-generation)'
echo "S1 q4: ---- without dynamic-code-generation ----"; tmo 60 "$P" jit "$S/prof/jit0.sb" 2>&1 | cat
echo "S1 q4: ---- with (allow dynamic-code-generation) ----"; tmo 60 "$P" jit "$S/prof/jit1.sb" 2>&1 | cat
echo "S1 q4: ---- control, no sandbox ----"; echo '(version 1)(allow default)' > "$S/prof/jit_open.sb"; tmo 60 "$P" jit "$S/prof/jit_open.sb" 2>&1 | cat
end;; esac

# ---- Q5 ---------------------------------------------------------------------
case " $1 " in *" 5 "*)
hr Q5 setuid
sudo -n true && echo "S1 q5.sudo: passwordless sudo available"
sudo -n cp /usr/bin/id "$S/suid-id" && sudo -n chown root:wheel "$S/suid-id" && sudo -n chmod 4755 "$S/suid-id"
ls -l "$S/suid-id" | sed 's/^/S1 q5.file: /'
echo "S1 q5.control_unsandboxed: suid-id -u -> $("$S/suid-id" -u 2>&1)   (sudo -n id -u -> $(sudo -n /usr/bin/id -u 2>&1))"
mk q5a
mk q5b '(deny process-exec* (file-mode #o4000))'
mk q5r '(allow file-read*)'
echo "S1 q5.profile_a (exec allowed): $(tmo 30 "$P" apply "$S/prof/q5a.sb" -- "$S/suid-id" -u 2>&1 | tr '\n' ' ')"
echo "S1 q5.profile_b (deny process-exec* file-mode #o4000): $(tmo 30 "$P" apply "$S/prof/q5b.sb" -- "$S/suid-id" -u 2>&1 | tr '\n' ' ')"
echo "S1 q5.sudo_inside_readall: $(tmo 30 "$P" apply "$S/prof/q5r.sb" -- /usr/bin/sudo -n /usr/bin/id -u 2>&1 | tr '\n' ' ')"
echo "S1 q5.sudo_inside_base: $(tmo 30 "$P" apply "$S/prof/q5a.sb" -- /usr/bin/sudo -n /usr/bin/id -u 2>&1 | tr '\n' ' ')"
end;; esac

# ---- Q6 ---------------------------------------------------------------------
case " $1 " in *" 6 "*)
hr Q6 network
cat > "$S/listen.py" <<'EOF'
import socket, sys
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(8)
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
log = open(sys.argv[2], "a")
while True:
    c, _ = s.accept(); log.write("connection\n"); log.flush(); c.close()
EOF
python3 "$S/listen.py" "$S/port" "$S/listener.log" & LP=$!
sleep 1; PORT=$(cat "$S/port"); echo "S1 q6.listener: 127.0.0.1:$PORT"
UNIX='(allow network-bind (local unix-socket (subpath "'$S'/ws")))
(allow network-outbound (remote unix-socket (subpath "'$S'/ws")))
(allow network-inbound (local unix-socket (subpath "'$S'/ws")))'
mk n0
mk n1 '(deny network*)' "$UNIX"
mk n2 '(deny network*)' "$UNIX" '(allow mach-lookup (global-name "com.apple.dnssd.service") (global-name "com.apple.mDNSResponder"))' '(allow network-outbound (remote unix-socket (literal "/private/var/run/mDNSResponder")))'
mk n3 '(allow network*)'
for n in n0 n1 n2 n3; do echo "S1 q6: ---- profile $n ----"; tmo 60 "$P" net "$S/prof/$n.sb" $PORT "$S/ws" 2>&1 | cat; done
echo "S1 q6.listener_saw: $(wc -l < "$S/listener.log" 2>/dev/null | tr -d ' ') connection(s) in total (the control profile n3 allows TCP, so expect that many)"
mk n3r '(allow network*)' '(allow file-read*)' '(allow mach-lookup)' '(allow sysctl*)' '(allow ipc-posix*)'
echo "S1 q6.curl_inside_n3r: $(tmo 40 "$P" apply "$S/prof/n3r.sb" -- /usr/bin/curl -sS -o /dev/null -w '%{http_code}' https://example.com 2>&1 | tr '\n' ' ')"
echo "S1 q6.curl_inside_n1r_denied: $(mk n1r '(deny network*)' '(allow file-read*)' '(allow mach-lookup)'; tmo 40 "$P" apply "$S/prof/n1r.sb" -- /usr/bin/curl -sS -o /dev/null -w '%{http_code}' https://example.com 2>&1 | tr '\n' ' ')"
kill $LP 2>/dev/null; end;; esac

# ---- Q9 ---------------------------------------------------------------------
case " $1 " in *" 9 "*)
hr Q9 process info
env S1_SECRET=outside sleep 600 & OUT=$!
echo "S1 q9.runtime_stand_in: pid $OUT (environment holds S1_SECRET)"
mk q9a "$SIG" '(allow process-info* (target same-sandbox))'
mk q9b "$SIG" '(allow process-info*)'
echo "S1 q9: ---- control, no sandbox ----"; S1_SECRET=self tmo 30 "$P" q9 - $OUT 2>&1 | cat
echo "S1 q9: ---- process-info* limited to same-sandbox ----"; S1_SECRET=self tmo 30 "$P" q9 "$S/prof/q9a.sb" $OUT 2>&1 | cat
echo "S1 q9: ---- process-info* open ----"; S1_SECRET=self tmo 30 "$P" q9 "$S/prof/q9b.sb" $OUT 2>&1 | cat
kill $OUT 2>/dev/null; end;; esac

# ---- Q7, Q8 -------------------------------------------------------------------
case " $1 " in *" 78 "*)
hr Q7 Q8 iterative read set and mach services
S1_DIR="$S" HERE="$HERE" tmo 1500 python3 "$HERE/iterate.py" 2>&1 | cat
end;; esac
exit 0
