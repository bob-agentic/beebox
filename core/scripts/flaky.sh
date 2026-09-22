#!/usr/bin/env bash
# Runs the library test suite N times and reports which tests failed, how often.
#
# A suite that fails "sometimes" is a suite you cannot trust: every red run
# needs a human to decide whether it counts. Use this to get a number instead
# of a hunch — before a change, and after.
#
#   ./scripts/flaky.sh          # 20 runs
#   ./scripts/flaky.sh 50       # 50 runs
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

runs=${1:-20}
fails=0
log=$(mktemp)

cargo build --tests --quiet 2>/dev/null   # keep compile time out of the loop

for i in $(seq 1 "$runs"); do
  out=$(cargo test --lib 2>&1)
  if grep -qE '^test result: ok' <<<"$out"; then
    printf '.'
  else
    printf 'F'
    fails=$((fails + 1))
    grep -E '^test .* FAILED$' <<<"$out" | sed 's/^test //; s/ \.\.\. FAILED$//' >>"$log"
  fi
done

echo
echo "── $runs runs, $fails failed ──"
if [ "$fails" -gt 0 ]; then
  echo "failures by test:"
  sort "$log" | uniq -c | sort -rn | sed 's/^/  /'
fi
rm -f "$log"
[ "$fails" -eq 0 ]
