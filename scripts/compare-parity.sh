#!/usr/bin/env bash
# Raster-diff htmltopdf output against the Chromium reference PNGs and print a
# markdown parity table. Needs poppler-utils (pdftoppm) and ImageMagick
# (compare, convert, identify).
#
# Usage: ./scripts/compare-parity.sh [pdf-dir] [threshold-percent]
#   pdf-dir            dir of rendered PDFs (default: run render-fixtures.sh output)
#   threshold-percent  max allowed diff % before a fixture FAILs (default: 5)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
REF_DIR="$REPO_DIR/crates/htmltopdf/tests/fixtures/references"
PDF_DIR="${1:-/tmp/htmltopdf-parity/ours}"
THRESHOLD="${2:-5}"

for cmd in pdftoppm compare convert identify; do
    command -v "$cmd" >/dev/null || { echo "Error: '$cmd' not found (poppler-utils + ImageMagick)"; exit 1; }
done
[ -d "$PDF_DIR" ] || { echo "No PDFs at $PDF_DIR — run scripts/render-fixtures.sh first"; exit 1; }
[ -d "$REF_DIR" ] || { echo "No references at $REF_DIR — run scripts/generate-references.sh first"; exit 0; }

WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
failed=0; any=0

echo "| Fixture | Page | Diff % | Status |"
echo "|---|---|---|---|"

for layer in features combined edge-cases; do
    [ -d "$REF_DIR/$layer" ] || continue
    for ref_png in "$REF_DIR/$layer"/*.png; do
        [ -f "$ref_png" ] || continue
        ref_base=$(basename "$ref_png" .png)
        if [[ "$ref_base" =~ ^(.*)-p([0-9]+)$ ]]; then
            name="${BASH_REMATCH[1]}"; page="${BASH_REMATCH[2]}"
        else
            name="$ref_base"; page=1
        fi
        pdf="$PDF_DIR/$layer/$name.pdf"
        [ -f "$pdf" ] || { echo "| $ref_base | $page | - | MISSING PDF |"; continue; }
        any=1

        prefix="$WORK/${layer}_${ref_base}"
        pdftoppm -r 150 -png -f "$page" -l "$page" "$pdf" "$prefix" 2>/dev/null || true
        render=""
        for c in "${prefix}-${page}.png" "${prefix}-0${page}.png" "${prefix}-00${page}.png"; do
            [ -f "$c" ] && render="$c" && break
        done
        [ -z "$render" ] && { echo "| $ref_base | $page | - | RENDER FAILED |"; continue; }

        # Normalize both rasters to the reference dimensions.
        dims=$(identify -format "%wx%h" "$ref_png")
        convert "$render" -resize "${dims}!" "$render"
        rref="$WORK/${layer}_${ref_base}_ref.png"
        convert "$ref_png" -resize "${dims}!" "$rref"

        diff=$(compare -metric AE "$rref" "$render" "$WORK/d.png" 2>&1 || true)
        diff=$(echo "$diff" | grep -oE '^[0-9]+(\.[0-9]+)?' || echo "0")
        total=$(identify -format "%[fx:w*h]" "$ref_png")
        pct=$(awk "BEGIN{printf \"%.2f\", ($diff/$total)*100}")
        status="PASS"
        awk "BEGIN{exit !($pct > $THRESHOLD)}" && { status="FAIL"; failed=1; }
        echo "| $ref_base | $page | ${pct}% | $status |"
    done
done

[ "$any" -eq 0 ] && { echo; echo "No references found. Run scripts/generate-references.sh."; exit 0; }
echo
if [ "$failed" -ne 0 ]; then
    echo "FAILED: one or more fixtures exceeded ${THRESHOLD}% diff."
    exit 1
fi
echo "All fixtures within ${THRESHOLD}% diff."
