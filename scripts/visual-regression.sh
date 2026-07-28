#!/usr/bin/env bash
# Visual-regression harness: render every parity fixture, diff it against a
# headless-Chrome reference raster, and print a per-fixture diff% table. With a
# stored baseline it gates: a change that makes any fixture visibly worse fails.
#
# Deps: cargo, headless Chrome/Chromium, poppler `pdftoppm`, Python3 + Pillow.
# No ImageMagick required.
#
# Usage:
#   ./scripts/visual-regression.sh                 # render, diff, print table
#   ./scripts/visual-regression.sh --refresh-refs  # regenerate Chrome references first
#   ./scripts/visual-regression.sh --update-baseline  # write visual-baseline.json
#   ./scripts/visual-regression.sh --check         # fail if any fixture regressed vs baseline
#   ./scripts/visual-regression.sh --montage 8     # also write montages for the worst 8
#   ./scripts/visual-regression.sh --matched-font  # render BOTH engines in one font (measures
#                                                  # layout, not Helvetica-vs-Times); own refs+baseline
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
FIXTURES_DIR="$REPO_DIR/crates/htmltopdf/tests/fixtures"
REF_DIR="$FIXTURES_DIR/references"
OURS_DIR="/tmp/htmltopdf-parity/ours"
OUT_DIR="$REPO_DIR/visual-report"
BASELINE="$REPO_DIR/visual-baseline.json"

REFRESH_REFS=0
CHECK=0
UPDATE_BASELINE=0
MONTAGE=0
MATCHED_FONT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --refresh-refs) REFRESH_REFS=1 ;;
        --check) CHECK=1 ;;
        --update-baseline) UPDATE_BASELINE=1 ;;
        --montage) MONTAGE="${2:-8}"; shift ;;
        --matched-font) MATCHED_FONT="${2:-Arial}"; [ "${2:-}" = "" ] || shift ;;
        *) echo "unknown arg: $1"; exit 2 ;;
    esac
    shift
done

# Matched-font mode: preprocess fixtures with a shared base font so both engines
# render identical glyphs (rasterized by the same poppler), isolating layout.
# Uses its own reference set and baseline so it never clobbers the default ones.
if [ -n "$MATCHED_FONT" ]; then
    echo "==> Matched-font mode: '$MATCHED_FONT'"
    SRC="/tmp/htmltopdf-parity/matched-src"
    rm -rf "$SRC"; mkdir -p "$SRC"
    python3 "$SCRIPT_DIR/inject-font.py" "$FIXTURES_DIR" "$SRC" "$MATCHED_FONT"
    export HTMLTOPDF_FIXTURES_DIR="$SRC"
    REF_DIR="$FIXTURES_DIR/references-matched"
    export HTMLTOPDF_REF_DIR="$REF_DIR"
    OURS_DIR="/tmp/htmltopdf-parity/ours-matched"
    BASELINE="$REPO_DIR/visual-baseline-matched.json"
fi

mkdir -p "$OUT_DIR"

echo "==> Rendering fixtures with htmltopdf"
"$SCRIPT_DIR/render-fixtures.sh" "$OURS_DIR" >/dev/null

if [ "$REFRESH_REFS" = 1 ] || [ ! -d "$REF_DIR" ] || [ -z "$(ls -A "$REF_DIR" 2>/dev/null)" ]; then
    echo "==> Generating Chrome reference rasters (this launches headless Chrome per fixture)"
    "$SCRIPT_DIR/generate-references.sh"
else
    echo "==> Using existing Chrome references in $REF_DIR (pass --refresh-refs to rebuild)"
fi

echo "==> Diffing"
DIFF_ARGS=(--ours "$OURS_DIR" --refs "$REF_DIR" --out "$OUT_DIR" --json "$OUT_DIR/report.json")
if [ "$MONTAGE" -gt 0 ]; then
    DIFF_ARGS+=(--montage "$MONTAGE")
fi
if [ "$CHECK" = 1 ]; then
    DIFF_ARGS+=(--baseline "$BASELINE" --tol 1.5)
fi

set +e
python3 "$SCRIPT_DIR/visual-diff.py" "${DIFF_ARGS[@]}"
STATUS=$?
set -e

if [ "$UPDATE_BASELINE" = 1 ]; then
    cp "$OUT_DIR/report.json" "$BASELINE"
    echo "==> Baseline updated: $BASELINE"
fi

exit $STATUS
