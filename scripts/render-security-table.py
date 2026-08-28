#!/usr/bin/env python3
"""Render docs/Security-Table.md to styled HTML (and optional PDF via Chromium).

Usage:
    python3 scripts/render-security-table.py
    python3 scripts/render-security-table.py --pdf
"""

from __future__ import annotations

import argparse
import html
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "docs" / "Security-Table.md"
HTML_OUT = ROOT / "docs" / "Security-Table.html"
PDF_OUT = ROOT / "docs" / "Security-Table.pdf"

CSS = """
:root {
  --bg: #fafafa;
  --text: #1a1a1a;
  --muted: #555;
  --border: #d0d0d0;
  --head: #f0f4f8;
  --link: #0b57d0;
  --code-bg: #f4f4f4;
  --strong: #0d7a3e;
  --good: #1565c0;
  --careful: #b45309;
  --weak: #b91c1c;
  --na: #6b7280;
}

* { box-sizing: border-box; }

body {
  font-family: "Segoe UI", system-ui, -apple-system, sans-serif;
  line-height: 1.55;
  color: var(--text);
  background: var(--bg);
  max-width: 1280px;
  margin: 0 auto;
  padding: 2rem 1.5rem 4rem;
}

h1 { font-size: 1.85rem; margin-top: 0; border-bottom: 2px solid var(--border); padding-bottom: 0.4rem; }
h2 { font-size: 1.35rem; margin-top: 2.2rem; color: #222; }
h3 { font-size: 1.1rem; margin-top: 1.6rem; }

p, li { max-width: 72ch; }
p.lead { font-size: 1.05rem; color: var(--muted); }

a { color: var(--link); }
code {
  font-family: ui-monospace, "Cascadia Code", monospace;
  font-size: 0.9em;
  background: var(--code-bg);
  padding: 0.1em 0.35em;
  border-radius: 4px;
}

pre {
  background: #1e1e1e;
  color: #e8e8e8;
  padding: 1rem 1.1rem;
  border-radius: 8px;
  overflow-x: auto;
  font-size: 0.82rem;
  line-height: 1.45;
  max-width: 100%;
}

pre code { background: none; padding: 0; color: inherit; }

hr { border: none; border-top: 1px solid var(--border); margin: 2rem 0; }

blockquote {
  margin: 1rem 0;
  padding: 0.75rem 1rem;
  border-left: 4px solid var(--link);
  background: #eef4fc;
  border-radius: 0 6px 6px 0;
}

blockquote p { margin: 0; max-width: none; }

ul { padding-left: 1.4rem; }
li { margin: 0.35rem 0; }

.meta {
  font-size: 0.92rem;
  color: var(--muted);
  margin-bottom: 1.5rem;
}

.table-wrap {
  overflow-x: auto;
  margin: 1.25rem 0 1.75rem;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: #fff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.06);
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.92rem;
}

th, td {
  border: 1px solid var(--border);
  padding: 0.65rem 0.75rem;
  vertical-align: top;
  text-align: left;
}

th {
  background: var(--head);
  font-weight: 600;
  white-space: normal;
  word-wrap: break-word;
  hyphens: auto;
}

table.table-grid {
  table-layout: fixed;
}

table.table-grid th,
table.table-grid td {
  width: calc(100% / var(--cols, 7));
  font-size: 0.8rem;
  line-height: 1.3;
  padding: 0.55rem 0.45rem;
  vertical-align: middle;
  text-align: center;
  overflow-wrap: anywhere;
}

table.table-grid thead th {
  font-size: 0.78rem;
  font-weight: 700;
}

table.table-grid tbody td:first-child {
  text-align: left;
  font-weight: 500;
  font-size: 0.82rem;
}

tr:nth-child(even) td { background: #fbfcfd; }

.rating-strong { background: #ecfdf3 !important; color: var(--strong); font-weight: 600; }
.rating-good { background: #eff6ff !important; color: var(--good); font-weight: 600; }
.rating-careful { background: #fffbeb !important; color: var(--careful); font-weight: 600; }
.rating-weak { background: #fef2f2 !important; color: var(--weak); font-weight: 600; }
.rating-na { background: #f3f4f6 !important; color: var(--na); }

.center { text-align: center; }

.footer-note {
  margin-top: 2.5rem;
  padding-top: 1rem;
  border-top: 1px solid var(--border);
  font-size: 0.85rem;
  color: var(--muted);
}

@media print {
  body { background: #fff; padding: 0.5in; max-width: none; }
  .table-wrap { box-shadow: none; page-break-inside: avoid; }
  h2, h3 { page-break-after: avoid; }
  a { color: inherit; text-decoration: none; }
  .no-print { display: none; }
}
"""


def inline_md(text: str) -> str:
    text = html.escape(text)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"\*(.+?)\*", r"<em>\1</em>", text)
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    text = re.sub(
        r"\[([^\]]+)\]\(([^)]+)\)",
        lambda m: f'<a href="{html.escape(m.group(2), quote=True)}">{m.group(1)}</a>',
        text,
    )
    return text


def rating_class(cell: str) -> str | None:
    plain = re.sub(r"<[^>]+>", "", cell)
    plain = plain.strip()
    if plain.startswith("Strong") or "**Strong**" in cell:
        return "rating-strong"
    if plain.startswith("Good") or "**Good**" in cell:
        return "rating-good"
    if plain.startswith("Careful") or "**Careful**" in cell:
        return "rating-careful"
    if plain.startswith("Weak") or "**Weak**" in cell:
        return "rating-weak"
    if plain in {"—", "-", "N/A"} or plain.startswith("—"):
        return "rating-na"
    return None


def parse_table(lines: list[str], start: int) -> tuple[str, int]:
    rows: list[list[str]] = []
    i = start
    while i < len(lines) and lines[i].strip().startswith("|"):
        row = [c.strip() for c in lines[i].strip().strip("|").split("|")]
        rows.append(row)
        i += 1

    if len(rows) < 2:
        return "", start

    header = rows[0]
    body_rows = rows[2:] if len(rows) > 2 else []
    alignments = rows[1] if len(rows) > 1 else []
    col_count = len(header)
    grid = col_count >= 6
    table_cls = "table-grid" if grid else ""
    table_attr = f' class="{table_cls}" style="--cols: {col_count};"' if grid else ""

    parts = [f"<div class=\"table-wrap\"><table{table_attr}>"]
    parts.append("<thead><tr>")
    for idx, cell in enumerate(header):
        cls_parts: list[str] = []
        if grid or (idx < len(alignments) and alignments[idx].strip().startswith(":")):
            cls_parts.append("center")
        cls = f' class="{" ".join(cls_parts)}"' if cls_parts else ""
        parts.append(f"<th{cls}>{inline_md(cell)}</th>")
    parts.append("</tr></thead><tbody>")

    for row in body_rows:
        parts.append("<tr>")
        for idx, cell in enumerate(row):
            rendered = inline_md(cell)
            classes: list[str] = []
            if grid:
                if idx > 0:
                    classes.append("center")
            elif idx < len(alignments) and alignments[idx].strip().startswith(":"):
                classes.append("center")
            rating = rating_class(rendered)
            if rating:
                classes.append(rating)
            attr = f' class="{" ".join(classes)}"' if classes else ""
            parts.append(f"<td{attr}>{rendered}</td>")
        parts.append("</tr>")

    parts.append("</tbody></table></div>")
    return "".join(parts), i


def parse_markdown(source: str) -> str:
    lines = source.splitlines()
    out: list[str] = []
    i = 0
    first_para = True

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        if not stripped:
            i += 1
            continue

        if stripped == "---":
            out.append("<hr>")
            i += 1
            continue

        if stripped.startswith("```"):
            fence = stripped
            i += 1
            block: list[str] = []
            while i < len(lines) and not lines[i].strip().startswith("```"):
                block.append(html.escape(lines[i]))
                i += 1
            out.append(f"<pre><code>{''.join(block)}</code></pre>")
            i += 1
            continue

        if stripped.startswith("|"):
            table_html, i = parse_table(lines, i)
            if table_html:
                out.append(table_html)
            continue

        if stripped.startswith("> "):
            out.append(f"<blockquote><p>{inline_md(stripped[2:])}</p></blockquote>")
            i += 1
            continue

        if stripped.startswith("# "):
            out.append(f"<h1>{inline_md(stripped[2:])}</h1>")
            i += 1
            continue

        if stripped.startswith("## "):
            out.append(f"<h2>{inline_md(stripped[3:])}</h2>")
            i += 1
            continue

        if stripped.startswith("### "):
            out.append(f"<h3>{inline_md(stripped[4:])}</h3>")
            i += 1
            continue

        if stripped.startswith("- "):
            out.append("<ul>")
            while i < len(lines) and lines[i].strip().startswith("- "):
                out.append(f"<li>{inline_md(lines[i].strip()[2:])}</li>")
                i += 1
            out.append("</ul>")
            continue

        if stripped.startswith("*") and stripped.endswith("*") and not stripped.startswith("**"):
            out.append(f"<p><em>{inline_md(stripped.strip('*'))}</em></p>")
            i += 1
            continue

        if stripped.startswith("**Last updated:**"):
            out.append(f'<p class="meta">{inline_md(stripped)}</p>')
            i += 1
            continue

        para_lines = [stripped]
        i += 1
        while i < len(lines) and lines[i].strip() and not lines[i].strip().startswith(
            ("#", "-", "|", "```", "---", "*")
        ):
            para_lines.append(lines[i].strip())
            i += 1

        joined = " ".join(para_lines)
        cls = ' class="lead"' if first_para else ""
        first_para = False
        out.append(f"<p{cls}>{inline_md(joined)}</p>")

    return "\n".join(out)


def build_html(body: str) -> str:
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Web3 &amp; DEX Security — Vaughan Paths</title>
  <style>{CSS}</style>
</head>
<body>
  <p class="no-print meta">Generated from <code>Security-Table.md</code> — open in a browser; use Print → Save as PDF for a PDF copy.</p>
  {body}
  <p class="footer-note">Source: docs/Security-Table.md · Regenerate: <code>python3 scripts/render-security-table.py --pdf</code></p>
</body>
</html>
"""


def render_pdf(html_path: Path, pdf_path: Path) -> None:
    chromium = shutil.which("chromium") or shutil.which("google-chrome") or shutil.which("chrome")
    if not chromium:
        print("warning: Chromium not found; skipped PDF generation", file=sys.stderr)
        return

    cmd = [
        chromium,
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        f"--print-to-pdf={pdf_path}",
        html_path.as_uri(),
    ]
    subprocess.run(cmd, check=True, capture_output=True)
    print(f"wrote {pdf_path.relative_to(ROOT)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pdf", action="store_true", help="Also render PDF via headless Chromium")
    args = parser.parse_args()

    if not SOURCE.is_file():
        print(f"error: missing {SOURCE}", file=sys.stderr)
        return 1

    body = parse_markdown(SOURCE.read_text(encoding="utf-8"))
    HTML_OUT.write_text(build_html(body), encoding="utf-8")
    print(f"wrote {HTML_OUT.relative_to(ROOT)}")

    if args.pdf:
        render_pdf(HTML_OUT, PDF_OUT)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
