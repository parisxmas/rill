#!/bin/sh
# The bundle, in the order the pieces depend on each other.
#
# `pack.rill` writes page.js — world.js, net.js, host.js and boot.js
# concatenated into the one module the page is given — and packs whichever of
# page.js and page.min.js it finds. The minified one is deleted first and made
# in between, so what is packed can only ever have been made from the source
# that is on disk: there is no way to ship yesterday's minified page.
#
# esbuild rather than webpack, and worth saying why: there is one file, no
# module graph left to resolve (pack.rill has already flattened it), and
# nothing to transpile. What is wanted is a minifier — comments out, names
# shortened — and that is one command with no config against a hundred lines
# of one. If a real bundle graph ever comes back, so should webpack.
set -e
cd "$(dirname "$0")"
RILL=../../../target/release/rill

rm -f page.min.js
"$RILL" build world.rill -o world --target wasm32-wasip1
"$RILL" run pack.rill > /dev/null          # writes page.js; the pak it makes here is thrown away
npx --yes esbuild@0.23.1 page.js --minify --format=esm --outfile=page.min.js
"$RILL" run pack.rill | tail -3
"$RILL" build server.rill --target x86_64-unknown-linux-gnu
cp server.o valleyd-x86.o
