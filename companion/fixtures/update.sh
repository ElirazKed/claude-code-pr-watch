#!/bin/sh
# Regenerates status.ts from the cases in hooks/fixtures.test.ts and what the mod derives from
# them now. The test prints the file it expects between two marker lines whenever the stored
# one differs (the mod's tests can't write files), and this keeps what is between them.
# Run from anywhere: sh companion/fixtures/update.sh
set -eu
repo=$(cd "$(dirname "$0")/../.." && pwd)
file="$repo/companion/fixtures/status.ts"
out=$(mktemp)
fresh=$(mktemp)
trap 'rm -f "$out" "$fresh"' EXIT
# A first run has no file to import yet: start from none.
[ -f "$file" ] || printf 'export default []\n' >"$file"
claude plugin test "$repo" >"$out" 2>&1 || true
awk '
  $0 == "-----BEGIN companion/fixtures/status.ts-----" { on = 1; next }
  $0 == "-----END companion/fixtures/status.ts-----" { on = 0 }
  on
' "$out" >"$fresh"
if [ -s "$fresh" ]; then
  mv "$fresh" "$file"
  echo "Updated companion/fixtures/status.ts: review the diff, then run claude plugin test . again."
elif grep -q '(fail)' "$out"; then
  echo "status.ts is up to date, but some tests fail:" >&2
  grep -A3 '(fail)' "$out" >&2
  exit 1
else
  echo "status.ts is up to date."
fi
