#!/usr/bin/env python3
"""Copy the parity fixtures into a temp tree with a low-priority base
`font-family` injected, so htmltopdf and Chrome render the same font and the
visual diff measures *layout* instead of the Helvetica-vs-Times mismatch.

The rule is placed at the top of <head> as `html { font-family: … }`: because it
targets the root and sits first, any fixture that sets its own font on <body> or
an element still wins (font-family / font-face fixtures keep their intent), while
font-less fixtures fall back to the shared family.

Usage: inject-font.py <src-fixtures-root> <dst-root> <font-family>
"""
import os
import re
import sys

LAYERS = ("features", "combined", "edge-cases")


def main():
    src, dst, family = sys.argv[1], sys.argv[2], sys.argv[3]
    rule = f"<style>html {{ font-family: '{family}', sans-serif; }}</style>"
    head_re = re.compile(r"<head[^>]*>", re.IGNORECASE)
    count = 0
    for layer in LAYERS:
        sdir = os.path.join(src, layer)
        if not os.path.isdir(sdir):
            continue
        ddir = os.path.join(dst, layer)
        os.makedirs(ddir, exist_ok=True)
        for name in os.listdir(sdir):
            if not name.endswith(".html"):
                continue
            html = open(os.path.join(sdir, name), encoding="utf-8").read()
            m = head_re.search(html)
            out = html[: m.end()] + rule + html[m.end():] if m else rule + html
            open(os.path.join(ddir, name), "w", encoding="utf-8").write(out)
            count += 1
    print(f"injected '{family}' into {count} fixtures -> {dst}")


if __name__ == "__main__":
    main()
