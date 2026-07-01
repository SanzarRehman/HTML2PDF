#!/usr/bin/env bash
# Generate Chromium reference rasters for every parity fixture.
# Renders each fixture with Chromium --print-to-pdf (Letter; margins from the
# fixture's @page rule) and converts every page to a 150-DPI PNG in
# crates/htmltopdf/tests/fixtures/references/<layer>/. Multi-page outputs use a
# "-pN" suffix for pages 2+.
# Usage: ./scripts/generate-references.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
FIXTURES_DIR="$REPO_DIR/crates/htmltopdf/tests/fixtures"
REF_DIR="$FIXTURES_DIR/references"

CHROME=$(command -v google-chrome-stable || command -v google-chrome \
    || command -v chromium || command -v chromium-browser || echo "")
if [ -z "$CHROME" ]; then
    for c in \
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
        "/Applications/Chromium.app/Contents/MacOS/Chromium"; do
        [ -x "$c" ] && CHROME="$c" && break
    done
fi
[ -z "$CHROME" ] && { echo "Chrome/Chromium not found — skipping"; exit 0; }
command -v pdftoppm >/dev/null || { echo "pdftoppm not found (install poppler-utils)"; exit 0; }

echo "Using: $CHROME"
count=0
for layer in features combined edge-cases; do
    mkdir -p "$REF_DIR/$layer"
    for html_file in "$FIXTURES_DIR/$layer"/*.html; do
        [ -f "$html_file" ] || continue
        name=$(basename "$html_file" .html)
        echo "  $layer/$name..."
        ref_pdf="$REF_DIR/$layer/$name.pdf"
        "$CHROME" --headless=new --disable-gpu --no-sandbox \
            --print-to-pdf="$ref_pdf" --no-pdf-header-footer \
            "file://$html_file" 2>/dev/null \
          || { echo "    WARN: failed $layer/$name"; continue; }
        [ -f "$ref_pdf" ] || continue

        pages=$(pdftoppm -r 10 -png "$ref_pdf" /tmp/_rc 2>/dev/null; \
                ls /tmp/_rc*.png 2>/dev/null | wc -l; rm -f /tmp/_rc*.png)
        [ "$pages" -lt 1 ] && pages=1
        for p in $(seq 1 "$pages"); do
            base=$([ "$p" -eq 1 ] && echo "$name" || echo "${name}-p${p}")
            pdftoppm -r 150 -png -f "$p" -l "$p" "$ref_pdf" "$REF_DIR/$layer/$base" 2>/dev/null
            for cand in "$REF_DIR/$layer/${base}-${p}.png" \
                        "$REF_DIR/$layer/${base}-0${p}.png" \
                        "$REF_DIR/$layer/${base}-00${p}.png"; do
                [ -f "$cand" ] && mv "$cand" "$REF_DIR/$layer/${base}.png" && count=$((count+1)) && break
            done
        done
        rm -f "$ref_pdf"
    done
done
echo "Done. $count reference PNGs in $REF_DIR"
