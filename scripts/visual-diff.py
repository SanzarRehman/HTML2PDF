#!/usr/bin/env python3
"""Visual-regression comparator: htmltopdf vs headless-Chrome references.

Rasterizes each rendered fixture PDF and diffs it, page by page, against the
committed-out reference PNGs (Chrome, produced by generate-references.sh). Emits
a per-fixture diff% table and a JSON report; can gate against a stored baseline
so a change that makes any fixture render visibly worse fails the run.

No ImageMagick dependency — poppler `pdftoppm` (raster) + Pillow (diff) only,
both already used elsewhere in the repo.

Usage:
  visual-diff.py --ours DIR --refs DIR [--out DIR] [--json FILE]
                 [--dpi 150] [--threshold 40]
                 [--baseline FILE --tol 1.5]   # regression gate
                 [--montage N]                 # write montages for worst N
"""
import argparse
import json
import os
import subprocess
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFont

LAYERS = ("features", "combined", "edge-cases")


def rasterize_pdf(pdf, out_prefix, dpi):
    """Render every page of `pdf` to `out_prefix-<n>.png`; return sorted paths."""
    subprocess.run(
        ["pdftoppm", "-r", str(dpi), "-png", pdf, out_prefix],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    d = os.path.dirname(out_prefix) or "."
    base = os.path.basename(out_prefix)
    pages = sorted(
        os.path.join(d, f)
        for f in os.listdir(d)
        if f.startswith(base + "-") and f.endswith(".png")
    )
    return pages


def ref_page_path(refs, layer, name, page):
    """Reference PNG for a page (page 1 = name.png, page N = name-pN.png)."""
    fname = f"{name}.png" if page == 1 else f"{name}-p{page}.png"
    p = os.path.join(refs, layer, fname)
    return p if os.path.exists(p) else None


def diff_pct(a, b, threshold):
    """Percent of pixels differing by > threshold in any channel, over the
    shared top-left region (pages may differ by a pixel in size)."""
    w, h = min(a.width, b.width), min(a.height, b.height)
    a = a.convert("RGB").crop((0, 0, w, h))
    b = b.convert("RGB").crop((0, 0, w, h))
    d = ImageChops.difference(a, b).convert("L").point(lambda p: 255 if p > threshold else 0)
    differing = sum(d.point(lambda p: 1 if p else 0).getdata())
    return 100.0 * differing / (w * h), (a, b, d)


def montage(ours, chrome, mask, path, label):
    w, h = ours.size
    red = Image.new("RGB", (w, h), (255, 0, 0))
    heat = Image.composite(red, chrome, mask)
    pad, lblh = 10, 20
    canvas = Image.new("RGB", (w * 3 + pad * 4, h + lblh + pad * 2), (255, 255, 255))
    draw = ImageDraw.Draw(canvas)
    try:
        font = ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial.ttf", 13)
    except Exception:
        font = ImageFont.load_default()
    draw.text((pad, pad), label, fill=(0, 0, 0), font=font)
    for i, img in enumerate((ours, chrome, heat)):
        canvas.paste(img, (pad + i * (w + pad), pad + lblh))
    canvas.save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ours", required=True, help="dir of rendered fixture PDFs (<layer>/<name>.pdf)")
    ap.add_argument("--refs", required=True, help="dir of Chrome reference PNGs (<layer>/<name>.png)")
    ap.add_argument("--out", default="/tmp/htmltopdf-visual", help="scratch dir for rasters/montages")
    ap.add_argument("--json", default=None, help="write the diff report as JSON here")
    ap.add_argument("--dpi", type=int, default=150)
    ap.add_argument("--threshold", type=int, default=40, help="per-pixel channel delta to count as different")
    ap.add_argument("--baseline", default=None, help="baseline JSON to gate against")
    ap.add_argument("--tol", type=float, default=1.5, help="allowed diff%% increase vs baseline before failing")
    ap.add_argument("--montage", type=int, default=0, help="write montages for the worst N fixtures")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    results = {}       # "layer/name" -> diff%
    montage_data = {}  # "layer/name" -> (ours,chrome,mask) of page 1
    missing_refs = []

    for layer in LAYERS:
        ours_layer = os.path.join(args.ours, layer)
        if not os.path.isdir(ours_layer):
            continue
        for f in sorted(os.listdir(ours_layer)):
            if not f.endswith(".pdf"):
                continue
            name = f[:-4]
            key = f"{layer}/{name}"
            pdf = os.path.join(ours_layer, f)
            work = os.path.join(args.out, layer)
            os.makedirs(work, exist_ok=True)
            our_pages = rasterize_pdf(pdf, os.path.join(work, name), args.dpi)
            if not our_pages:
                continue
            page_pcts = []
            for i, our_png in enumerate(our_pages, start=1):
                ref = ref_page_path(args.refs, layer, name, i)
                if ref is None:
                    if i == 1:
                        missing_refs.append(key)
                    continue
                pct, imgs = diff_pct(Image.open(our_png), Image.open(ref), args.threshold)
                page_pcts.append(pct)
                if i == 1:
                    montage_data[key] = imgs
            if page_pcts:
                results[key] = round(sum(page_pcts) / len(page_pcts), 2)

    if not results:
        print("No comparable fixtures (are the Chrome references generated?)", file=sys.stderr)
        if missing_refs:
            print("Missing references for:", ", ".join(missing_refs), file=sys.stderr)
        return 2

    ranked = sorted(results.items(), key=lambda kv: kv[1], reverse=True)
    overall = round(sum(results.values()) / len(results), 2)

    print(f"\n{'fixture':32} {'diff%':>7}")
    print("-" * 41)
    for key, pct in ranked:
        bar = "#" * min(40, int(pct))
        print(f"{key:32} {pct:6.2f}  {bar}")
    print("-" * 41)
    print(f"{'MEAN':32} {overall:6.2f}   over {len(results)} fixtures @ {args.dpi}dpi")
    if missing_refs:
        print(f"\n(no Chrome reference for {len(missing_refs)}: {', '.join(missing_refs)})")

    if args.json:
        with open(args.json, "w") as fh:
            json.dump({"mean": overall, "fixtures": results}, fh, indent=2, sort_keys=True)
        print(f"\nwrote {args.json}")

    if args.montage:
        mdir = os.path.join(args.out, "montages")
        os.makedirs(mdir, exist_ok=True)
        for key, pct in ranked[:args.montage]:
            if key in montage_data:
                ours, chrome, mask = montage_data[key]
                montage(ours, chrome, mask, os.path.join(mdir, key.replace("/", "_") + ".png"),
                        f"{key}  ({pct:.1f}%)")
        print(f"wrote {min(args.montage, len(ranked))} montages to {mdir}")

    # Regression gate against a stored baseline.
    if args.baseline and os.path.exists(args.baseline):
        with open(args.baseline) as fh:
            base = json.load(fh).get("fixtures", {})
        regressions = [
            (k, base[k], v) for k, v in results.items()
            if k in base and v > base[k] + args.tol
        ]
        if regressions:
            print(f"\nREGRESSIONS (> +{args.tol}%):")
            for k, was, now in sorted(regressions, key=lambda r: r[2] - r[1], reverse=True):
                print(f"  {k:32} {was:6.2f} -> {now:6.2f}  (+{now-was:.2f})")
            return 1
        print(f"\nno fixture regressed by more than +{args.tol}% vs baseline")

    return 0


if __name__ == "__main__":
    sys.exit(main())
