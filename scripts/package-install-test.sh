#!/usr/bin/env bash
# Installs a V2 Linux package in a clean container, checks it, and removes it (#82,
# docs/PACKAGING.md). Run as root inside the container, from the Linux packages workflow:
#
#   bash scripts/package-install-test.sh <package.deb|package.rpm> <expected-files.txt>
#
# <expected-files.txt> lists the files every package must install, one path a line without the
# leading "/" (scripts/package-lib.mjs, linuxPackageFiles, written by the workflow), so the list
# has one home. Each step prints PASS or stops the script with FAIL.
set -euo pipefail

package=$1
expected=$2
name=velocity-download-manager-v2

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

echo "== Install $(basename "$package") on $(. /etc/os-release && echo "$PRETTY_NAME")"
case "$package" in
  *.deb)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -q
    # apt resolves the package's dependencies (WebKitGTK, GTK) from the distribution.
    apt-get install -y -q "./$package"
    ;;
  *.rpm)
    dnf install -y -q "./$package"
    ;;
  *) fail "not a deb or an rpm: $package" ;;
esac
echo "PASS: installed"

while IFS= read -r path; do
  [ -n "$path" ] || continue
  [ -f "/$path" ] || fail "/$path is missing"
done <"$expected"
echo "PASS: every expected file is installed"

# Every program finds every library it loads; ldd prints "not found" for one it doesn't.
programs=()
while IFS= read -r path; do
  case "$path" in usr/bin/*) programs+=("/$path") ;; esac
done <"$expected"
for program in "${programs[@]}"; do
  [ -x "$program" ] || fail "$program isn't executable"
  if ldd "$program" | grep -q 'not found'; then
    ldd "$program" >&2
    fail "$program can't load a library"
  fi
done
echo "PASS: every program loads"

# The engine and the torrent helper start and print their usage when given no command. A missing
# library or a too-old glibc would stop them before main, with another message.
for program in velocity-transfer velocity-torrent; do
  set +e
  message=$("/usr/bin/$program" 2>&1)
  status=$?
  set -e
  [ "$status" -eq 1 ] || fail "$program exited with $status: $message"
  case "$message" in *usage:*) ;; *) fail "$program printed no usage: $message" ;; esac
done
echo "PASS: the engine and the torrent helper start"

echo "== Remove"
case "$package" in
  *.deb) apt-get remove -y -q "$name" ;;
  *.rpm) dnf remove -y -q "$name" ;;
esac
while IFS= read -r path; do
  [ -n "$path" ] || continue
  [ ! -e "/$path" ] || fail "/$path is still there after removing the package"
done <"$expected"
echo "PASS: removing the package removes its files"
