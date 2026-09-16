#!/usr/bin/env python3
"""What's changed, as a page: CHANGELOG.md rendered in the site's dress,
the releases in a sidebar, the entries between two releases gathered
under the release they went into — and the front page's own section,
which names the latest release and what has landed since.

    python3 site/build-changelog.py     # writes site/changelog.html, and the
                                        # "What's changed" section into site/index.html
"""

import html
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / "reference"))
from build import colour, is_output, pandoc, SHORT, VERSION  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "CHANGELOG.md"
SITE = ROOT / "site"

NAV = """      <a href="/#look">The language</a>
      <a href="/#models">Models</a>
      <a href="/#numbers">Numbers</a>
      <a href="/#missing">What it lacks</a>
      <a href="/#book">The book</a>
      <a href="/reference/">Reference</a>
      <a href="/changelog.html" class="here">What's changed</a>
      <a href="/#download">Download</a>
      <a href="https://rillsdl.baltavista.com/">The editor</a>
      <a href="/#made">Made with Rill</a>"""

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>What's changed — Rill</title>
<meta name="description" content="Every change to Rill, release by release: {latest}.">
<link rel="canonical" href="https://rill.baltavista.com/changelog.html">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Rill">
<meta property="og:url" content="https://rill.baltavista.com/changelog.html">
<meta property="og:title" content="What's changed — Rill">
<meta property="og:description" content="Every change to Rill, release by release: {latest}.">
<meta property="og:image" content="https://rill.baltavista.com/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="What's changed — Rill">
<meta name="twitter:description" content="Every change to Rill, release by release: {latest}.">
<meta name="twitter:image" content="https://rill.baltavista.com/og.png">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><text y='26' font-size='26'>🐦</text></svg>">
<link rel="stylesheet" href="reference/reference.css">
<style>
h2.release{{font-family:var(--mono);font-size:24px;letter-spacing:-.02em;margin:56px 0 4px;font-weight:700;scroll-margin-top:66px}}
h2.release:first-of-type{{margin-top:0}}
h2.release .v{{color:var(--teal)}}
p.when{{font-family:var(--mono);font-size:12px;color:var(--muted);margin:0 0 18px;letter-spacing:.04em}}
h3.entry{{font-size:16.5px;margin:30px 0 8px;scroll-margin-top:66px}}
h3.entry .tag{{font-family:var(--mono);font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--muted);font-weight:600;margin-right:10px}}
.releases li a{{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
</style>
</head>
<body>
<div class="top"></div>
<div class="wrap">
  <div class="bar">
    <a href="/" class="name">Rill</a>
    <span class="ver">v{short}</span>
    <span class="crumb">What's changed</span>
    <nav>
{nav}
    </nav>
  </div>
  <div class="page">
    <aside>
      <p class="side-head">Releases</p>
      <ul class="sections releases">
{side}
      </ul>
    </aside>
    <main>
      <p class="chapter-no">What's changed</p>
      <h1>Release by release</h1>
      <p>Every change, as it landed. A release names what it gathers; the entries
      under it are the steps, newest first, each written when it was made.</p>
{body}
    </main>
  </div>
  <footer>
    <p class="mono">Rill v{short} · generated from CHANGELOG.md</p>
  </footer>
</div>
</body>
</html>
"""


def entries():
    text = SRC.read_text()
    parts = re.split(r"^(?=## )", text, flags=re.M)[1:]
    out = []
    for part in parts:
        m = re.match(r"## (.+?)\n", part)
        head = m.group(1).strip()
        vm = re.match(r"(\d+\.\d+\.\d+) — (.+)", head)
        um = re.match(r"Unreleased — (.+)", head)
        out.append({
            "version": vm.group(1) if vm else None,
            "title": (vm.group(2) if vm else um.group(1) if um else head).strip(),
            "md": part[m.end():].strip(),
        })
    return out


def render(md):
    frag = pandoc(md)
    def one(m):
        code = html.unescape(m.group(1))
        return f'<pre class="out">{html.escape(code)}</pre>' if is_output(code) else f"<pre>{colour(code)}</pre>"
    return re.sub(r'<pre[^>]*><code[^>]*>(.*?)</code></pre>', one, frag, flags=re.S)


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def build():
    es = entries()
    # the entries before the first release are what has landed since it
    body, side = [], []
    latest = next(e for e in es if e["version"])
    pending = [e for e in es[:es.index(latest)]]
    groups = []   # (release entry or None, steps)
    if pending:
        groups.append((None, pending))
    i = es.index(latest)
    while i < len(es):
        rel = es[i]
        j = i + 1
        steps = []
        while j < len(es) and not es[j]["version"]:
            steps.append(es[j])
            j += 1
        groups.append((rel, steps))
        i = j
    for rel, steps in groups:
        if rel is None:
            rid = "since-" + slug(latest["version"])
            body.append(f'<h2 class="release" id="{rid}"><span class="v">Since {latest["version"]}</span></h2>')
            body.append(f'<p class="when">not yet released · {len(steps)} change{"s" if len(steps) != 1 else ""}</p>')
            side.append(f'<li><a href="#{rid}">Since {latest["version"]}</a></li>')
        else:
            rid = "v" + slug(rel["version"])
            body.append(f'<h2 class="release" id="{rid}"><span class="v">{rel["version"]}</span> — {html.escape(rel["title"])}</h2>')
            body.append(f'<p class="when">{len(steps)} change{"s" if len(steps) != 1 else ""} gathered</p>')
            body.append(render(rel["md"]))
            side.append(f'<li><a href="#{rid}">{rel["version"]} — {html.escape(rel["title"])}</a></li>')
        for st in steps:
            sid = rid + "-" + slug(st["title"])[:60]
            body.append(f'<h3 class="entry" id="{sid}"><span class="tag">change</span>{html.escape(st["title"])}</h3>')
            body.append(render(st["md"]))
    page = PAGE.format(latest=html.escape(latest["version"] + " — " + latest["title"]), short=SHORT, nav=NAV,
                       side="\n".join("        " + s for s in side), body="\n".join(body))
    (SITE / "changelog.html").write_text(page)

    # the front page's section: the latest release, and what has landed since
    first_para = latest["md"].split("\n\n")[0].replace("\n", " ")
    # the release's first two sentences; the page has the rest
    first_para = "".join(re.findall(r".+?[.!?](?:\s|$)", first_para)[:2]).strip()
    first_para = re.sub(r"`([^`]*)`", r"<code>\1</code>", html.escape(first_para))
    since = "".join(f'<li><a href="/changelog.html#since-{slug(latest["version"])}-{slug(e["title"])[:60]}">{html.escape(e["title"])}</a></li>' for e in pending)
    section = f'''  <!-- ================= what's changed (written by site/build-changelog.py) ================= -->
  <section id="changed">
    <h2 class="sec">What's changed</h2>
    <p class="sec-lede"><b>{html.escape(latest["version"])} — {html.escape(latest["title"])}.</b> {first_para}</p>
    <p>Since then:</p>
    <ul class="since">{since}</ul>
    <a class="chip" href="/changelog.html">Every release, every change</a>
  </section>
'''
    index = (SITE / "index.html").read_text()
    start = index.find("  <!-- ================= what's changed")
    if start >= 0:
        end = index.index("  </section>\n", start) + len("  </section>\n")
        index = index[:start] + section + index[end:]
    else:
        anchor = '  <!-- ================= book ================= -->'
        index = index.replace(anchor, section + "\n" + anchor)
    (SITE / "index.html").write_text(index)
    print(f"{len(es)} entries, {len(groups)} releases → site/changelog.html; front page section written")


if __name__ == "__main__":
    build()
