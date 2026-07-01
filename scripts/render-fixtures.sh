#!/usr/bin/env bash
# Render every parity fixture to PDF with the htmltopdf CLI.
# Usage: ./scripts/render-fixtures.sh [output-dir]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
FIXTURES_DIR="$REPO_DIR/crates/htmltopdf/tests/fixtures"
OUTPUT_DIR="${1:-/tmp/htmltopdf-parity/ours}"

echo "Building htmltopdf (release)..."
cargo build --release --manifest-path="$REPO_DIR/Cargo.toml" >/dev/null 2>&1
CLI="$REPO_DIR/target/release/htmltopdf"
[ -x "$CLI" ] || { echo "Error: CLI not found at $CLI"; exit 1; }

for layer in features combined edge-cases; do
    mkdir -p "$OUTPUT_DIR/$layer"
    for html_file in "$FIXTURES_DIR/$layer"/*.html; do
        [ -f "$html_file" ] || continue
        name=$(basename "$html_file" .html)
        echo "  $layer/$name..."
        # Letter matches Chromium --print-to-pdf; margins come from each
        # fixture's @page rule (28.8pt = Chromium's 0.4in default).
        "$CLI" --paper letter "$html_file" "$OUTPUT_DIR/$layer/$name.pdf" \
            || echo "    WARN: failed to render $layer/$name"
    done
done

echo "Done. PDFs in $OUTPUT_DIR ($(find "$OUTPUT_DIR" -name '*.pdf' | wc -l | tr -d ' ') files)"
