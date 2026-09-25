#!/bin/sh
# Builds a Rill program into an image for QEMU's `virt` RISC-V machine and
# runs it: `targets/riscv32-virt/build.sh prog.rill [esp32c3|esp32c6]`.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
src=$1
chip=${2:-esp32c3}
case $chip in
  esp32c3) rust=riscv32imc-unknown-none-elf; march=rv32imc_zicsr_zifencei ;;
  *) rust=riscv32imac-unknown-none-elf; march=rv32imac_zicsr_zifencei ;;
esac
gcc=${RISCV_GCC:-$(ls -d "$HOME"/.espressif/tools/riscv32-esp-elf/*/riscv32-esp-elf/bin/riscv32-esp-elf-gcc | tail -1)}
out=${OUT:-/tmp/rill-riscv}
mkdir -p "$out"
name=$(basename "$src" .rill)
runtime=${RILL_TARGET_DIR:-$root/target}/$rust/release/librill_runtime.a
[ -f "$runtime" ] || (cd "$root" && cargo build --release -p rill_runtime --target "$rust")
"$root/target/release/rill" build --target "$chip" "$src" -o "$out/$name" >/dev/null
"$gcc" -march=$march -mabi=ilp32 -nostartfiles -static -T "$here/link.ld" \
  "$here/crt0.S" "$here/syscalls.c" "$out/$name.o" "$runtime" -lm -lc -lgcc \
  -Wl,--gc-sections -o "$out/$name.elf"
qemu-system-riscv32 -machine virt -bios none -nographic -m 128M -kernel "$out/$name.elf"
