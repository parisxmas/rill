#!/usr/bin/env python3
"""Join the chapter files into one document for the PDF build.

Three things happen here that only matter in print. Cross-file links become
plain prose, because in one volume there is no other file to point at. The
manual numbers in the headings are stripped, because LaTeX numbers them and
otherwise every heading reads "1.1 1.1". And horizontal rules are dropped,
since a rule between every section is a page full of lines.
"""
import pathlib, re

BOOK = pathlib.Path(__file__).resolve().parent.parent
ORDER = ["ch01-what-functional-programming-is", "ch02-why-rill", "ch03-first-programs",
         "ch04-functions-are-values", "ch05-data-and-pattern-matching",
         "ch06-lists-and-the-prelude", "ch07-loops-are-recursion", "ch08-traits",
         "ch09-small-algorithms", "ch10-sorting", "ch11-trees", "ch12-parsing",
         "ch13-graphs-and-grids", "ch14-dynamic-programming",
         "ch15-memory-without-a-collector", "ch16-strands-and-channels", "ch17-machines"]
PARTS = {1: "Thinking Functionally", 3: "The Language", 9: "Algorithms", 15: "Underneath"}

out = []
for i, name in enumerate(ORDER, 1):
    s = (BOOK / f"{name}.md").read_text()
    s = re.sub(r"\[([^\]]+)\]\(ch\d[^)]*\.md\)", r"\1", s)
    s = re.sub(r"\[([^\]]+)\]\(\.\./([^)]*)\)", r"`\2`", s)
    s = s.replace("\n---\n", "\n")
    s = re.sub(r"^# Chapter \d+\.\s*", "# ", s, flags=re.M)
    s = re.sub(r"^## \d+\.\d+\s+", "## ", s, flags=re.M)
    s = re.sub(r"^### \d+\.\d+\.\d+\s+", "### ", s, flags=re.M)
    if i in PARTS:
        out.append(f"\n\\part{{{PARTS[i]}}}\n")
    out.append(s.strip())
(BOOK / "build/book.md").write_text("\n\n".join(out))
print(f"assembled {len(ORDER)} chapters")
