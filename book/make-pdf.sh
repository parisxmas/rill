#!/bin/sh
# Build book/build/rill.pdf from the chapter files.
#
#   sh book/make-pdf.sh
#
# Needs pandoc and a TeX installation with xelatex. TinyTeX is enough:
#   curl -sL "https://yihui.org/tinytex/install-bin-unix.sh" | sh
#   ~/Library/TinyTeX/bin/*/tlmgr install mdframed needspace titling newunicodechar
set -e
cd "$(dirname "$0")/.."
[ -d "$HOME/Library/TinyTeX/bin" ] && PATH="$PATH:$(echo "$HOME"/Library/TinyTeX/bin/*)"
export PATH

# pandoc runs xelatex in a temporary directory, so a relative path to the cover
# image does not resolve; this is how the art directory is put on its search path.
TEXINPUTS="$PWD/book/art:$TEXINPUTS"
export TEXINPUTS

python3 book/build/assemble.py

pandoc book/build/book.md -o book/build/rill.pdf \
  --pdf-engine=xelatex --toc --toc-depth=2 --top-level-division=chapter --number-sections \
  -V documentclass=book -V classoption=oneside -V papersize=a4 \
  -V geometry:"margin=2.6cm,top=2.4cm,bottom=2.6cm" \
  -V mainfont="Palatino" -V sansfont="Helvetica Neue" -V monofont="Menlo" \
  -V fontsize=10pt -V linkcolor=black -V urlcolor=black -V toccolor=black \
  -V title="Functional Programming with Rill" \
  -V author="Writing programs out of values instead of commands" \
  -V date="Version 0.2 · every example compiled and run" \
  -H book/build/head.tex

echo "book/build/rill.pdf"
