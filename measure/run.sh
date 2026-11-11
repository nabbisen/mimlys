#!/bin/bash
# THROWAWAY (handoff 011j): measures what the host tools and the dependency step need of /etc.
set -u
cd "$(dirname "$0")/.."
BIN=$(ls -t target/debug/deps/zz_measure-* | grep -v '\.d$' | head -n1)
run() { # cmds.json
  python3 measure/mk.py "$1" measure/cmds-run.json
  ZZ_CMDS=$PWD/measure/cmds-run.json $BIN --nocapture 2>&1 | sed -n '/^TASK/,/^test zz_measure/p' | grep -v "^test zz_measure\|^ONLY.*{" | cut -c1-300
}
echo "##### $(uname -m); $(. /etc/os-release; echo $PRETTY_NAME); kernel $(uname -r)"
echo "##### hosts/resolv/nsswitch here:"; grep -v '^#' /etc/nsswitch.conf | grep -E '^hosts' ; cat /etc/resolv.conf | grep -v '^#' | head -3
ls -la /etc/ssl/certs/ca-certificates.crt /etc/localtime /etc/ld.so.cache 2>&1
echo "##### A. offline tools, no /etc, no /opt"
run measure/cmds0.json
echo "##### B. name and TLS, incremental"
for set in '' '"/etc/nsswitch.conf"' '"/etc/resolv.conf"' '"/etc/nsswitch.conf","/etc/resolv.conf"' '"/etc/ssl/certs/ca-certificates.crt"' '"/etc/nsswitch.conf","/etc/resolv.conf","/etc/ssl/certs/ca-certificates.crt"' '"/etc/resolv.conf","/etc/ssl/certs/ca-certificates.crt"' '"/etc/nsswitch.conf","/etc/ssl/certs/ca-certificates.crt"'; do
  echo "######## B [$set]"; ZZ_EXTRA="$set" run measure/cmds-net.json
done
echo "##### C. cargo fetch, incremental"
cat > measure/cmds-cargo.json <<JSON
[["cargo-fetch",true,"mkdir -p p/src && cd p && printf '[package]\\\\nname=\"x\"\\\\nversion=\"0.1.0\"\\\\nedition=\"2021\"\\\\n[dependencies]\\\\nitoa=\"1\"\\\\n' > Cargo.toml && : > src/lib.rs && $HOME/.cargo/bin/cargo fetch"]]
JSON
export ZZ_ENV="RUSTUP_HOME = \"$HOME/.rustup\", RUSTUP_TOOLCHAIN = \"stable\", PATH = \"/usr/bin:/bin\""
BASE="\"$HOME/.cargo/bin\", \"$HOME/.rustup\""
for set in '' '"/etc/nsswitch.conf"' '"/etc/resolv.conf"' '"/etc/ssl/certs/ca-certificates.crt"' '"/etc/nsswitch.conf","/etc/ssl/certs/ca-certificates.crt"' '"/etc/resolv.conf","/etc/ssl/certs/ca-certificates.crt"' '"/etc/nsswitch.conf","/etc/resolv.conf","/etc/ssl/certs/ca-certificates.crt"'; do
  echo "######## C [$set]"; ZZ_EXTRA="$BASE${set:+, $set}" run measure/cmds-cargo.json | head -n 14
done
