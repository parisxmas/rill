#!/bin/sh
# Starts the editor's server over /models, seeding it with the examples
# the first time the volume is empty, and passes every argument on —
# `--allow files,net` and the like.
set -eu
if [ -z "$(ls -A /models 2>/dev/null)" ]; then
  cp /app/examples/*.rill /models/
fi
# The examples are everyone's starting point: edited and run as they
# stand, never saved over. With no accounts yet nothing is saved at all
# (`--readonly`); drop that flag to let a copy be kept under another name.
protect=$(cd /app/examples && ls *.rill | tr '\n' ',' | sed 's/,$//')
cd /tmp
exec /app/bin/server --host 0.0.0.0 --port 8765 --root /models --static /app/editors/web/static --rill /app/bin/rill --protect "$protect" --readonly "$@"
