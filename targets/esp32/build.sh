#!/bin/sh
# Builds a Rill program as an ESP-IDF application:
#   targets/esp32/build.sh prog.rill [esp32c3|esp32c6] [idf.py arguments...]
# With no idf.py arguments it builds; `flash monitor` puts it on a board,
# and `qemu monitor` runs it in Espressif's QEMU (ESP32-C3).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
src=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
chip=${2:-esp32c3}
shift 2 2>/dev/null || shift $#
case $chip in
  esp32c3) rust=riscv32imc-unknown-none-elf ;;
  *) rust=riscv32imac-unknown-none-elf ;;
esac
: "${IDF_PATH:?source the export.sh of ESP-IDF first}"
runtime=${RILL_TARGET_DIR:-$root/target}/$rust/release/librill_runtime.a
[ -f "$runtime" ] || (cd "$root" && cargo build --release -p rill_runtime --target "$rust")
mkdir -p "$here/main/lib"
"$root/target/release/rill" build --target "$chip" "$src" -o "$here/main/lib/prog" >/dev/null
rm -f "$here/main/lib/librill_prog.a"
riscv32-esp-elf-ar rcs "$here/main/lib/librill_prog.a" "$here/main/lib/prog.o"
cp "$runtime" "$here/main/lib/librill_runtime.a"
cd "$here"
[ "$(cat build/.chip 2>/dev/null)" = "$chip" ] || { idf.py set-target "$chip"; mkdir -p build; echo "$chip" > build/.chip; }
idf.py "${@:-build}"
