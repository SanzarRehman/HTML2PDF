#!/usr/bin/env bash
# Compare one-process htmltopdf worker concurrency with the same number of
# fresh headless-Chrome conversions. macOS `/usr/bin/time -l` supplies CPU time
# and RSS for htmltopdf; Chrome's peak RSS is sampled as the sum of every tagged
# browser/renderer process, because Chrome is multi-process. The Chrome timing
# endpoint is when every requested PDF exists, not when its idle helper process
# tree eventually exits.
#
# Usage: scripts/benchmark-concurrency.sh <input.html> [workers] [output-dir]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INPUT="${1:?usage: $0 <input.html> [workers] [output-dir]}"
WORKERS="${2:-20}"
OUT="${3:-/tmp/htmltopdf-concurrency-${WORKERS}}"
BIN="$ROOT/target/release/htmltopdf"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

[ -x "$BIN" ] || { echo "Build first: cargo build --release -p htmltopdf-cli" >&2; exit 1; }
[ -x "$CHROME" ] || { echo "Chrome not found: $CHROME" >&2; exit 1; }
[ -f "$INPUT" ] || { echo "Input not found: $INPUT" >&2; exit 1; }

INPUT="$(cd "$(dirname "$INPUT")" && pwd)/$(basename "$INPUT")"
mkdir -p "$OUT/ours" "$OUT/chrome"
TAG="htmltopdf-concurrency-$$"
now() { perl -MTime::HiRes=time -e 'printf "%.6f", time'; }

echo "htmltopdf: $WORKERS concurrent conversions"
/usr/bin/time -l -o "$OUT/ours.time" \
  "$BIN" bench-concurrent "$INPUT" "$OUT/ours" "$WORKERS" 1 \
  >"$OUT/ours.log"

echo "chrome: $WORKERS fresh concurrent conversions"
start="$(now)"
pids=()
for index in $(seq 1 "$WORKERS"); do
  "$CHROME" --headless=new --disable-gpu --no-sandbox --no-first-run \
    --disable-breakpad --no-pdf-header-footer --disable-background-networking \
    "--user-data-dir=$OUT/${TAG}-${index}" \
    "--print-to-pdf=$OUT/chrome/chrome-${index}.pdf" \
    "file://$INPUT" \
    >"$OUT/chrome/chrome-${index}.log" 2>&1 &
  pids+=("$!")
done

# Each Chrome child inherits the unique user-data-dir flag. Sum all matching
# browser/renderer RSS values every 100 ms; `ps` reports RSS in KiB and CPU as
# an instantaneous percentage. Stop at the last PDF becoming available, then
# close only processes belonging to this uniquely tagged benchmark.
completed=0
completed_at=""
while :; do
  alive=0
  for pid in "${pids[@]}"; do
    kill -0 "$pid" 2>/dev/null && alive=1
  done
  completed=0
  for pdf in "$OUT"/chrome/chrome-*.pdf; do
    [ -s "$pdf" ] && completed=$((completed + 1))
  done
  metrics="$(ps -axo rss=,pcpu=,command= | awk -v tag="$TAG" '
    /Google Chrome/ && index($0, tag) { rss += $1; cpu += $2 }
    END { printf "%.0f %.2f\n", rss, cpu }
  ')"
  printf '%s %s %d\n' "$(now)" "$metrics" "$completed" >>"$OUT/chrome-samples.log"
  if [ "$completed" -eq "$WORKERS" ]; then
    completed_at="$(now)"
    break
  fi
  [ "$alive" -eq 0 ] && break
  sleep 0.1
done

# Chrome on macOS may retain idle service helpers after --print-to-pdf. They
# are not part of request completion and are isolated by TAG from any user tab.
pkill -TERM -f "$TAG" 2>/dev/null || true
status=0
for pid in "${pids[@]}"; do
  wait "$pid" 2>/dev/null || true
done
[ "$completed" -eq "$WORKERS" ] || status=1
end="$(now)"

ours_wall_ms="$(awk -F': ' '/^wall_ms/ {print $2}' "$OUT/ours.log")"
ours_rss_kib="$(awk '/maximum resident set size/ {print $1}' "$OUT/ours.time")"
ours_cpu_s="$(awk '/ real / {print $3 + $5}' "$OUT/ours.time")"
chrome_end="${completed_at:-$end}"
chrome_wall_s="$(awk -v start="$start" -v end="$chrome_end" 'BEGIN {printf "%.3f", end - start}')"
chrome_peak_kib="$(awk '{if ($2 > max) max = $2} END {print max + 0}' "$OUT/chrome-samples.log")"
chrome_peak_cpu="$(awk '{if ($3 > max) max = $3} END {print max + 0}' "$OUT/chrome-samples.log")"
chrome_cpu_s="$(awk 'NR > 1 {cpu += previous_cpu / 100 * ($1 - previous_time)} {previous_time = $1; previous_cpu = $3} END {printf "%.3f", cpu}' "$OUT/chrome-samples.log")"

awk -v workers="$WORKERS" -v ours_ms="$ours_wall_ms" -v ours_rss="$ours_rss_kib" \
    -v ours_cpu="$ours_cpu_s" -v chrome_s="$chrome_wall_s" -v chrome_rss="$chrome_peak_kib" \
    -v chrome_cpu="$chrome_cpu_s" -v chrome_peak_cpu="$chrome_peak_cpu" 'BEGIN {
  printf "\n| Engine | Jobs | Wall | Throughput | Peak RSS | Total CPU | Peak sampled CPU |\n"
  printf "|---|---:|---:|---:|---:|---:|---:|\n"
  printf "| htmltopdf | %d | %.3f s | %.2f PDF/s | %.1f MiB | %.2f s | process total |\n", workers, ours_ms/1000, workers/(ours_ms/1000), ours_rss/1024/1024, ours_cpu
  printf "| Chrome (fresh processes) | %d | %.3f s | %.2f PDF/s | %.1f MiB | %.2f s | %.0f%% |\n", workers, chrome_s, workers/chrome_s, chrome_rss/1024, chrome_cpu, chrome_peak_cpu
}'

echo "\nRaw logs and PDFs: $OUT"
exit "$status"
