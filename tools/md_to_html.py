#!/usr/bin/env python3
# Copyright (C) 2026 YuanChi Hsieh
# SPDX-License-Identifier: GPL-3.0-or-later
"""The smallest Markdown to HTML that the user guide needs (P13 K24,
docs/guide.md): Initrd:/doc/guide.html for Periplus.

Usage: md_to_html.py IN.md OUT.html
       md_to_html.py --test

Headings (#, ##, ###), paragraphs, "- " lists, "> " notes, ``` blocks,
tables (each row a line, the cells joined by " | "; Periplus has no table
layout), and inline `code`, **bold** and [text](url). The title is the
first # heading. Everything else is text.
"""
import html
import re
import sys


def inline(text):
    """`code`, **bold** and [text](url) in an escaped line."""
    out = []
    # Split out code spans first: nothing inside them is markup.
    for i, part in enumerate(re.split(r"`([^`]*)`", text)):
        if i % 2:
            out.append(f"<code>{html.escape(part)}</code>")
            continue
        part = html.escape(part)
        part = re.sub(r"\*\*(.+?)\*\*", r"<b>\1</b>", part)
        part = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", r'<a href="\2">\1</a>', part)
        out.append(part)
    return "".join(out)


def convert(md):
    """The HTML page for Markdown text `md`."""
    lines = md.split("\n")
    body = []
    title = ""
    para = []
    in_list = False

    def flush():
        nonlocal in_list
        if para:
            body.append("<p>" + inline(" ".join(para)) + "</p>")
            para.clear()
        if in_list:
            body.append("</ul>")
            in_list = False

    i = 0
    while i < len(lines):
        line = lines[i].rstrip()
        if line.startswith("```"):
            flush()
            code = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                code.append(lines[i])
                i += 1
            body.append("<pre>" + html.escape("\n".join(code)) + "</pre>")
        elif m := re.match(r"(#{1,3}) (.*)", line):
            flush()
            level = len(m.group(1))
            if level == 1 and not title:
                title = m.group(2)
            body.append(f"<h{level}>{inline(m.group(2))}</h{level}>")
        elif line.startswith("- "):
            if para:
                body.append("<p>" + inline(" ".join(para)) + "</p>")
                para.clear()
            if not in_list:
                body.append("<ul>")
                in_list = True
            body.append("<li>" + inline(line[2:]) + "</li>")
        elif line.startswith("|"):
            flush()
            cells = [c.strip() for c in line.strip("|").split("|")]
            if all(re.fullmatch(r":?-+:?", c) for c in cells):
                pass
            else:
                header = i + 1 < len(lines) and re.match(r"\|\s*:?-", lines[i + 1])
                text = " | ".join(inline(c) for c in cells)
                body.append(f"<p><b>{text}</b></p>" if header else f"<p>{text}</p>")
        elif line.startswith("> "):
            flush()
            body.append("<p>" + inline(line[2:]) + "</p>")
        elif not line:
            flush()
        elif in_list and lines[i].startswith("  "):
            # A list item's next line.
            body[-1] = body[-1][: -len("</li>")] + " " + inline(line.strip()) + "</li>"
        else:
            if in_list:
                body.append("</ul>")
                in_list = False
            para.append(line.strip())
        i += 1
    flush()
    head = f"<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{html.escape(title)}</title></head>\n<body>\n"
    return head + "\n".join(body) + "\n</body></html>\n"


def test():
    page = convert("# T\n\nA `x<y` and **b** [l](u.html).\n\n- one\n- two\n\n"
                   "| a | b |\n|---|---|\n| 1 | 2 |\n\n```\nq < r\n```\n")
    assert "<title>T</title>" in page, page
    assert "<p>A <code>x&lt;y</code> and <b>b</b> <a href=\"u.html\">l</a>.</p>" in page, page
    assert "<ul>\n<li>one</li>\n<li>two</li>\n</ul>" in page, page
    assert "<p><b>a | b</b></p>\n<p>1 | 2</p>" in page, page
    assert "<pre>q &lt; r</pre>" in page, page
    print("md_to_html: OK")


def main():
    if sys.argv[1:] == ["--test"]:
        test()
        return
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as f:
        page = convert(f.read())
    with open(sys.argv[2], "w", encoding="utf-8") as f:
        f.write(page)


if __name__ == "__main__":
    main()
