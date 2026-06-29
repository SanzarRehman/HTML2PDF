#!/usr/bin/env bash
#
# Convert an HTML file to PDF through the htmltopdf-server HTTP API.
# Tunable concurrency, per-request latency, and a summary (min/avg/p50/p95/max,
# throughput). The first successful PDF is saved to the output path.
#
# Usage:
#   scripts/api-convert.sh [-u URL] [-i INPUT.html] [-o OUTPUT.pdf]
#                          [-c CONCURRENCY] [-n TOTAL_REQUESTS] [-q QUERY]
#
# Examples:
#   scripts/api-convert.sh                          # 1 request, default file
#   scripts/api-convert.sh -c 16 -n 64              # 64 requests, 16 in parallel
#   scripts/api-convert.sh -c 8 -n 32 -q 'landscape=true&font=Georgia'

set -uo pipefail

# ----- worker mode (re-invoked by xargs for each request) --------------------
if [[ "${1:-}" == "--worker" ]]; then
    i="$2"
    pdf="$TMP_DIR/r_$i.pdf"
    metrics=$(curl -s -X POST "$URL" \
        -H 'Content-Type: text/html' \
        --data-binary @"$INPUT" \
        -o "$pdf" \
        -w '%{http_code} %{time_total} %{size_download}' 2>/dev/null || echo "000 0 0")
    echo "$i $metrics" >"$TMP_DIR/m_$i"
    code=$(awk '{print $1}' <<<"$metrics")
    ms=$(awk '{printf "%.1f", $2 * 1000}' <<<"$metrics")
    bytes=$(awk '{print $3}' <<<"$metrics")
    printf 'req %-5s HTTP %-3s %9s ms %10s bytes\n' "$i" "$code" "$ms" "$bytes"
    exit 0
fi

# ----- main ------------------------------------------------------------------
cd "$(dirname "$0")/.." || exit 1

URL="http://127.0.0.1:8123/render"
INPUT="reg-2-9 1 copy.html"
OUTPUT="out/reg-2-9-1-copy.pdf"
CONCURRENCY=1
TOTAL=""
QUERY=""

usage() {
    sed -n '3,16p' "$0"
}

while getopts ":u:i:o:c:n:q:h" opt; do
    case "$opt" in
    u) URL="$OPTARG" ;;
    i) INPUT="$OPTARG" ;;
    o) OUTPUT="$OPTARG" ;;
    c) CONCURRENCY="$OPTARG" ;;
    n) TOTAL="$OPTARG" ;;
    q) QUERY="$OPTARG" ;;
    h)
        usage
        exit 0
        ;;
    *)
        usage
        exit 1
        ;;
    esac
done

TOTAL="${TOTAL:-$CONCURRENCY}"

if [[ -n "$QUERY" ]]; then
    case "$URL" in
    *\?*) URL="$URL&$QUERY" ;;
    *) URL="$URL?$QUERY" ;;
    esac
fi

if [[ ! -f "$INPUT" ]]; then
    echo "error: input file not found: $INPUT" >&2
    exit 1
fi

if ! curl -s -o /dev/null --max-time 2 "${URL%%/render*}/health"; then
    echo "error: server not reachable at $URL" >&2
    echo "       start it with: cargo run --release -p htmltopdf-server -- 127.0.0.1:8123" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
export URL INPUT TMP_DIR

echo "endpoint:    $URL"
echo "input:       $INPUT ($(wc -c <"$INPUT" | tr -d ' ') bytes)"
echo "requests:    $TOTAL    concurrency: $CONCURRENCY"
echo "------------------------------------------------------------"

start=$(python3 -c 'import time; print(time.time())')
seq 1 "$TOTAL" | xargs -P "$CONCURRENCY" -I{} "$0" --worker {}
end=$(python3 -c 'import time; print(time.time())')

# Save the first successful PDF to the output path.
saved=""
for i in $(seq 1 "$TOTAL"); do
    code=$(awk '{print $2}' "$TMP_DIR/m_$i" 2>/dev/null)
    if [[ "$code" == "200" && -s "$TMP_DIR/r_$i.pdf" ]]; then
        cp "$TMP_DIR/r_$i.pdf" "$OUTPUT"
        saved="$OUTPUT"
        break
    fi
done

echo "------------------------------------------------------------"
awk '$2 == "200" {print $3 * 1000}' "$TMP_DIR"/m_* | sort -n >"$TMP_DIR/times.txt"

python3 - "$TMP_DIR/times.txt" "$TOTAL" "$start" "$end" <<'PY'
import math, sys
times = [float(x) for x in open(sys.argv[1]) if x.strip()]
total = int(sys.argv[2])
wall = float(sys.argv[4]) - float(sys.argv[3])
ok = len(times)
print(f"ok: {ok}    failed: {total - ok}")
if ok:
    times.sort()
    pct = lambda p: times[min(ok - 1, max(0, math.ceil(p / 100 * ok) - 1))]
    print(
        f"latency ms   min {times[0]:.1f}   avg {sum(times)/ok:.1f}   "
        f"p50 {pct(50):.1f}   p95 {pct(95):.1f}   max {times[-1]:.1f}"
    )
if wall > 0:
    print(f"wall: {wall:.2f} s    throughput: {total / wall:.1f} req/s")
PY

if [[ -n "$saved" ]]; then
    echo "------------------------------------------------------------"
    echo "saved: $saved ($(wc -c <"$saved" | tr -d ' ') bytes)"
else
    echo "warning: no successful response to save" >&2
fi
