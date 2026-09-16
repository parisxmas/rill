#!/usr/bin/env python3
"""The language reference as pages: LANGUAGE.md split by chapter, each
chapter a page in the site's dress, the code coloured as the editor
colours it, a sidebar of chapters and the page's own sections, and
previous/next at the foot.

    python3 site/reference/build.py        # writes site/reference/*.html

Needs pandoc for the Markdown itself; the Rill colouring is done here,
a port of the model editor's highlight.js, so a theme written for one
serves the other.
"""

import html
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SRC = ROOT / "LANGUAGE.md"
OUT = ROOT / "site" / "reference"
LIB = ROOT / "lib"
RILL = ROOT / "target" / "release" / "rill"
VERSION = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
SHORT = ".".join(VERSION.split(".")[:2])

# ---- the Rill colouring -----------------------------------------------------------

KEYWORDS = {"fn", "type", "trait", "impl", "extern", "asm", "naked", "export", "private", "where", "module",
            "import", "as", "if", "then", "else", "match", "for", "in", "with", "while", "spawn", "select",
            "timeout", "default", "blocking", "machine", "state", "after", "save", "when", "priority",
            "system", "stay", "stop", "done"}
CONSTANTS = {"true", "false", "None", "Some", "Ok", "Err", "Nil", "Cons"}
BUILTIN_TYPES = {"Int", "Float", "Bool", "Str", "Unit", "Ptr", "Chan", "Buf", "Map", "Mail", "Option", "Result",
                 "List", "Show", "Ord"}
MACHINE_WORDS = {"sender", "reply", "output", "post", "set", "reset", "active", "me"}


def colour_line(text):
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c == "#":
            out.append(f'<span class="c">{html.escape(text[i:])}</span>')
            break
        if c == '"':
            j = i + 1
            body = []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    body.append(html.escape(text[j:j + 2]))
                    j += 2
                else:
                    body.append(html.escape(text[j]))
                    j += 1
            close = '"' if j < n else ""
            out.append(f'<span class="s">"{"".join(body)}{close}</span>')
            i = j + 1
            continue
        if c.isdigit() and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            j = i
            while j < n and text[j].isdigit():
                j += 1
            if j < n and text[j] == "." and j + 1 < n and text[j + 1].isdigit():
                j += 1
                while j < n and text[j].isdigit():
                    j += 1
            out.append(f'<span class="n">{text[i:j]}</span>')
            i = j
            continue
        if c.isalpha() or c == "_":
            j = i
            while j < n and (text[j].isalnum() or text[j] == "_"):
                j += 1
            word = text[i:j]
            rest = text[j:].lstrip()
            after = rest[:1]
            cls = ""
            if word in KEYWORDS:
                cls = "k"
            elif word in CONSTANTS:
                cls = "t"
            elif word in BUILTIN_TYPES:
                cls = "t"
            elif word[0].isupper():
                cls = "t"
            elif word in MACHINE_WORDS:
                cls = "m"
            elif after == "(" and not (i > 0 and text[i - 1] == "."):
                cls = "f"
            out.append(f'<span class="{cls}">{word}</span>' if cls else word)
            i = j
            continue
        if c == "-" and i + 1 < n and text[i + 1] == ">":
            out.append('<span class="o">-&gt;</span>')
            i += 2
            continue
        if c == "|" and i + 1 < n and text[i + 1] == ">":
            out.append('<span class="o">|&gt;</span>')
            i += 2
            continue
        out.append(html.escape(c))
        i += 1
    return "".join(out)


def colour(code):
    return "\n".join(colour_line(l) for l in code.split("\n"))


def is_output(code):
    """A block that is a terminal session or a compiler's words, not a program."""
    first = code.lstrip().split("\n", 1)[0]
    return first.startswith("$ ") or first.startswith("rill:") or " error at line " in first or first.startswith("ok    ") or first.startswith("FAIL")


# ---- the chapters ----------------------------------------------------------------

def chapters():
    text = SRC.read_text()
    head, *rest = re.split(r"^(?=## \d+\. )", text, flags=re.M)
    out = []
    for body in rest:
        m = re.match(r"## (\d+)\. (.+)\n", body)
        num, title = int(m.group(1)), m.group(2).strip()
        slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
        out.append({"num": num, "title": title, "slug": f"{num:02d}-{slug}", "md": body[m.end():].strip()})
    intro = re.sub(r"^# .*\n", "", head).strip().rstrip("-").strip()
    # the page before the chapters: how to get it and run the first program
    start = (OUT / "getting-started.md").read_text()
    out.insert(0, {"num": 0, "title": "Getting started", "slug": "00-getting-started", "md": start.strip()})
    # and the guide to the array library, after the chapters
    guide = (OUT / "grid.md").read_text()
    out.append({"num": len(out), "title": "The grid library", "slug": f"{len(out):02d}-the-grid-library", "md": guide.strip()})
    return intro, out


def pandoc(md):
    return subprocess.run(["pandoc", "-f", "markdown-smart", "-t", "html", "--no-highlight", "--wrap=none"],
                          input=md, capture_output=True, text=True, check=True).stdout


def render_code(fragment):
    def one(m):
        code = html.unescape(m.group(1))
        if is_output(code):
            return f'<pre class="out">{html.escape(code)}</pre>'
        return f"<pre>{colour(code)}</pre>"
    return re.sub(r'<pre[^>]*><code[^>]*>(.*?)</code></pre>', one, fragment, flags=re.S)


def sections_of(fragment):
    return [(m.group(1), re.sub(r"<[^>]+>", "", m.group(2))) for m in re.finditer(r'<h3 id="([^"]+)"[^>]*>(.*?)</h3>', fragment)]


PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — The Rill Language Reference</title>
<meta name="description" content="{description}">
<link rel="canonical" href="https://rill.baltavista.com/reference/{file}">
<meta property="og:type" content="article">
<meta property="og:site_name" content="Rill">
<meta property="og:url" content="https://rill.baltavista.com/reference/{file}">
<meta property="og:title" content="{title} — The Rill Language Reference">
<meta property="og:description" content="{description}">
<meta property="og:image" content="https://rill.baltavista.com/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title} — The Rill Language Reference">
<meta name="twitter:description" content="{description}">
<meta name="twitter:image" content="https://rill.baltavista.com/og.png">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><text y='26' font-size='26'>🐦</text></svg>">
<link rel="stylesheet" href="reference.css">
</head>
<body>
<div class="top"></div>
<div class="wrap">
  <div class="bar">
    <a href="/" class="name">Rill</a>
    <span class="ver">v{short}</span>
    <span class="crumb"><a href="index.html">Reference</a></span>
  </div>
  <div class="page">
    <aside>
      <p class="side-head">Site</p>
      <nav class="site">
        <a href="/#look">The language</a>
        <a href="/#models">Models</a>
        <a href="/#numbers">Numbers</a>
        <a href="/#missing">What it lacks</a>
        <a href="/#book">The book</a>
        <a href="/reference/" class="here">Reference</a>
        <a href="/changelog.html">What's changed</a>
        <a href="/#download">Download</a>
        <a href="https://rillsdl.baltavista.com/">The editor</a>
        <a href="/#made">Made with Rill</a>
      </nav>
      <p class="side-head">Chapters</p>
      <ol class="chapters">
{chapters}
      </ol>
      <p class="side-head">Standard library</p>
      <ul class="sections modules">
{modules}
      </ul>
      {own}
    </aside>
    <main{mainclass}>
      <p class="chapter-no">{num}</p>
      <h1>{title}</h1>
{body}
      <div class="prevnext">
        {prev}
        {next}
      </div>
    </main>
  </div>
  <footer>
    <p class="mono">Rill v{short} · The Rill Language Reference · generated from LANGUAGE.md, which every construct's test keeps honest</p>
    <p class="mono">Rill is designed and written by <a href="mailto:barisakin@gmail.com"><b>Barış Akın</b></a> with Anthropic's AI models.</p>
  </footer>
</div>
</body>
</html>
"""

INDEX = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>The Rill Language Reference</title>
<meta name="description" content="The whole of Rill, chapter by chapter, with the programs that show each construct: types, expressions, functions, traits, memory, concurrency and machines, the prelude, sockets, files, C, the toolchain.">
<link rel="canonical" href="https://rill.baltavista.com/reference/">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Rill">
<meta property="og:url" content="https://rill.baltavista.com/reference/">
<meta property="og:title" content="The Rill Language Reference">
<meta property="og:description" content="The whole of Rill, chapter by chapter, with the programs that show each construct.">
<meta property="og:image" content="https://rill.baltavista.com/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="The Rill Language Reference">
<meta name="twitter:description" content="The whole of Rill, chapter by chapter, with the programs that show each construct.">
<meta name="twitter:image" content="https://rill.baltavista.com/og.png">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><text y='26' font-size='26'>🐦</text></svg>">
<link rel="stylesheet" href="reference.css">
</head>
<body>
<div class="top"></div>
<div class="wrap">
  <div class="bar">
    <a href="/" class="name">Rill</a>
    <span class="ver">v{short}</span>
    <span class="crumb">Reference</span>
  </div>
  <div class="page">
    <aside>
      <p class="side-head">Site</p>
      <nav class="site">
        <a href="/#look">The language</a>
        <a href="/#models">Models</a>
        <a href="/#numbers">Numbers</a>
        <a href="/#missing">What it lacks</a>
        <a href="/#book">The book</a>
        <a href="/reference/" class="here">Reference</a>
        <a href="/changelog.html">What's changed</a>
        <a href="/#download">Download</a>
        <a href="https://rillsdl.baltavista.com/">The editor</a>
        <a href="/#made">Made with Rill</a>
      </nav>
    </aside>
  <main class="index">
    <p class="chapter-no">Version {short}</p>
    <h1>The Rill Language Reference</h1>
{intro}
    <ol class="toc">
{toc}
    </ol>
  </main>
  </div>
  <footer>
    <p class="mono">Rill v{short} · The Rill Language Reference · generated from LANGUAGE.md, which every construct's test keeps honest</p>
    <p class="mono">Rill is designed and written by <a href="mailto:barisakin@gmail.com"><b>Barış Akın</b></a> with Anthropic's AI models.</p>
  </footer>
</div>
</body>
</html>
"""


# ---- the standard library ----------------------------------------------------------

FOLDERS = {"text": "Text", "data": "Data formats", "coll": "Collections", "num": "Numbers", "web": "The web",
           "net": "The network", "db": "Databases", "cli": "The command line", "crypto": "Cryptography",
           "random": "Randomness", "time": "Time"}


def modules():
    """Every module of lib/ with its documentation, as `rill doc` writes it."""
    import tempfile
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="rilldoc-"))
    subprocess.run([str(RILL), "doc", str(LIB), "-o", str(tmp)], check=True, capture_output=True)
    out = []
    for folder in sorted(LIB.iterdir()):
        if not folder.is_dir() or folder.name not in FOLDERS:
            continue
        for f in sorted(folder.glob("*.rill")):
            if f.stem.startswith(("bench", "wt_")):
                continue
            doc = tmp / f"{f.stem}.md"
            if not doc.exists():
                continue
            md = doc.read_text()
            md = re.sub(r"^# .*\n", "", md, count=1).strip()
            out.append({"folder": folder.name, "name": f.stem, "path": f"{folder.name}/{f.stem}",
                        "file": f"lib-{folder.name}-{f.stem}.html", "md": md, "blurb": first_sentence(md)})
    return out


def module_pages(mods, chs, side):
    for m in mods:
        frag = render_code(pandoc(m["md"]))
        own = [(m2.group(1), m2.group(2)) for m2 in re.finditer(r'<h2 id="([^"]+)"[^>]*>(.*?)</h2>', frag)]
        own_html = ""
        if own:
            own_html = '<p class="side-head">On this page</p>\n      <ul class="sections">\n' + "\n".join(
                f'        <li><a href="#{sid}">{html.escape(t)}</a></li>' for sid, t in own) + "\n      </ul>"
        modlist = "\n".join(
            f'        <li{" class=\"current\"" if x is m else ""}><a href="{x["file"]}">{x["path"]}</a></li>' for x in mods if x["folder"] == m["folder"])
        i = mods.index(m)
        prev = f'<a class="prev" href="{mods[i-1]["file"]}">← {mods[i-1]["path"]}</a>' if i > 0 else '<a class="prev" href="lib.html">← The standard library</a>'
        nxt = f'<a class="next" href="{mods[i+1]["file"]}">{mods[i+1]["path"]} →</a>' if i + 1 < len(mods) else '<a class="next" href="lib.html">The standard library →</a>'
        head = f'<p class="import">Imported as <code>import "{m["path"]}" as {m["name"]}</code>, its names are then <code>{m["name"]}.…</code>. Every signature below is the one the checker infers.</p>'
        page = PAGE.format(title=html.escape(m["path"]), description=html.escape(m["blurb"]), file=m["file"], short=SHORT,
                           chapters=side, modules=modlist, own=own_html, num=f"Standard library · {FOLDERS[m['folder']]}",
                           body=head + frag, prev=prev, next=nxt, mainclass=' class="module"')
        (OUT / m["file"]).write_text(page)
    # the library's own contents page
    groups = []
    for folder in FOLDERS:
        mine = [m for m in mods if m["folder"] == folder]
        if not mine:
            continue
        items = "\n".join(f'      <li><a href="{m["file"]}"><span class="no">{m["name"]}</span><b>{m["path"]}</b><span class="blurb">{html.escape(m["blurb"])}</span></a></li>' for m in mine)
        groups.append(f'<h3 id="{folder}">{FOLDERS[folder]} <code class="folder">lib/{folder}</code></h3>\n<ol class="toc">\n{items}\n</ol>')
    intro = (f"<p>{len(mods)} modules, all written in Rill, imported by their path under <code>lib/</code>: "
             '<code>import "data/json" as json</code>. The compiler finds them beside its own <code>bin/</code> in a '
             "packaged toolchain, or through <code>RILL_PATH</code>. These pages are what <code>rill doc lib</code> "
             "writes: the comment above each declaration, and the signature the checker infers for it.</p>")
    page = PAGE.format(title="The standard library", description=f"The {len(mods)} modules of Rill's standard library, documented from their own comments and the checker's signatures.",
                       file="lib.html", short=SHORT, chapters=side, modules="\n".join(f'        <li><a href="#{f}">{FOLDERS[f]}</a></li>' for f in FOLDERS if any(m["folder"] == f for m in mods)),
                       own="", num="Standard library", body=intro + "\n".join(groups), mainclass="",
                       prev=f'<a class="prev" href="{chs[-1]["file"]}">← {label(chs[-1])}</a>', next='<a class="next" href="index.html">Contents →</a>')
    (OUT / "lib.html").write_text(page)


def first_sentence(md):
    for para in md.split("\n\n"):
        p = para.strip()
        if p and not p.startswith(("#", "```", "|", "-", "*", "<")):
            text = re.sub(r"`([^`]*)`", r"\1", p.replace("\n", " "))
            text = re.sub(r"\*\*([^*]*)\*\*", r"\1", text)
            m = re.match(r"(.+?[.!?])(\s|$)", text)
            return (m.group(1) if m else text)[:220].rstrip(":")
    return ""


def build():
    OUT.mkdir(parents=True, exist_ok=True)
    intro, chs = chapters()
    for i, ch in enumerate(chs):
        ch["file"] = ch["slug"] + ".html"
        ch["blurb"] = first_sentence(ch["md"])
    mods = modules()
    folder_list = "\n".join(f'        <li><a href="lib.html#{f}">{FOLDERS[f]}</a></li>' for f in FOLDERS if any(m["folder"] == f for m in mods))
    for i, ch in enumerate(chs):
        fragment = render_code(pandoc(ch["md"]))
        # a link to another chapter's section, `#characters`, lives on that chapter's page
        fragment = re.sub(r'href="#([a-z0-9-]+)"', lambda m: f'href="{page_of(chs, m.group(1))}"' if page_of(chs, m.group(1)).endswith(".html") and m.group(1)[0].isdigit() else f'href="{page_of(chs, m.group(1))}#{m.group(1)}"', fragment)
        own = sections_of(fragment)
        side = "\n".join(
            f'        <li{" class=\"current\"" if c is ch else ""}><a href="{c["file"]}"><span class="no">{c["num"] or "→"}</span>{html.escape(c["title"])}</a></li>'
            for c in chs)
        own_html = ""
        if own:
            own_html = '<p class="side-head">On this page</p>\n      <ul class="sections">\n' + "\n".join(
                f'        <li><a href="#{sid}">{html.escape(stitle)}</a></li>' for sid, stitle in own) + "\n      </ul>"
        prev = f'<a class="prev" href="{chs[i-1]["file"]}">← {label(chs[i-1])}</a>' if i > 0 else '<a class="prev" href="index.html">← Contents</a>'
        nxt = f'<a class="next" href="{chs[i+1]["file"]}">{label(chs[i+1])} →</a>' if i + 1 < len(chs) else '<a class="next" href="index.html">Contents →</a>'
        if i + 1 == len(chs):
            nxt = '<a class="next" href="lib.html">The standard library →</a>'
        page = PAGE.format(title=html.escape(ch["title"]), description=html.escape(ch["blurb"]), file=ch["file"], short=SHORT,
                           chapters=side, modules=folder_list, own=own_html, num=f"Chapter {ch['num']}" if ch["num"] else "Start here", body=fragment, prev=prev, next=nxt, mainclass="")
        (OUT / ch["file"]).write_text(page)
    side_plain = "\n".join(f'        <li><a href="{c["file"]}"><span class="no">{c["num"] or "→"}</span>{html.escape(c["title"])}</a></li>' for c in chs)
    module_pages(mods, chs, side_plain)
    toc = "\n".join(
        f'      <li><a href="{c["file"]}"><span class="no">{c["num"] or "→"}</span><b>{html.escape(c["title"])}</b><span class="blurb">{html.escape(c["blurb"])}</span></a></li>'
        for c in chs)
    toc += f'\n      <li><a href="lib.html"><span class="no">lib</span><b>The standard library</b><span class="blurb">{len(mods)} modules — text, data formats, collections, numbers, the web, the network, databases, the command line, cryptography, randomness, time — each documented from its own comments.</span></a></li>'
    (OUT / "index.html").write_text(INDEX.format(short=SHORT, intro=pandoc(intro), toc=toc))
    print(f"{len(chs)} chapters and {len(mods)} modules written to {OUT.relative_to(ROOT)}/")


def label(c):
    return f"{c['num']}. {html.escape(c['title'])}" if c["num"] else html.escape(c["title"])


_ids = {}


def page_of(chs, anchor):
    """The chapter whose page carries a heading with this id."""
    if not _ids:
        for c in chs:
            # the chapter itself, as GitHub names it: `#7-concurrency`
            _ids[f"{c['num']}-" + re.sub(r"[^a-z0-9]+", "-", c["title"].lower()).strip("-")] = c["file"]
            for m in re.finditer(r"^#{2,4} (.+)$", c["md"], re.M):
                title = m.group(1).strip()
                # pandoc's id: lowercase, punctuation dropped, spaces to hyphens
                sid = re.sub(r"[^a-z0-9 _-]", "", re.sub(r"`", "", title).lower()).strip().replace(" ", "-")
                sid = re.sub(r"^[^a-z]+", "", sid)
                _ids.setdefault(sid, c["file"])
    return _ids.get(anchor, "")


if __name__ == "__main__":
    build()
