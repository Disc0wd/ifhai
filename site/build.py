#!/usr/bin/env python3
"""Build docs/index.html (GitHub Pages) from site/template.html and site/style.css.

The numbers in the page are copied from docs/DATA.md; keep the two in step when the data changes.
"""
import os

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
site = os.path.join(root, "site")
page = (
    open(os.path.join(site, "template.html")).read()
    .replace("@@CSS@@", open(os.path.join(site, "style.css")).read())
    .replace("@@REPO@@", "https://github.com/Disc0wd/ifhai")
)
assert "@@" not in page
os.makedirs(os.path.join(root, "docs"), exist_ok=True)
open(os.path.join(root, "docs", "index.html"), "w").write(page)
print("wrote docs/index.html")
