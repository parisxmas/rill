# Rill on an ESP32

The RISC-V ESP32s — **C3** (RV32IMC), **C6**, **H2**, **P4** (RV32IMAC as
Rill uses them) — run Rill programs as ESP-IDF applications. The Xtensa
ones (ESP32, S2, S3) do not: the LLVM Rill is built on has no Xtensa backend.

```
. ~/source/esp/esp-idf/export.sh
targets/esp32/build.sh hello.rill esp32c3            # build
targets/esp32/build.sh hello.rill esp32c3 flash monitor   # on a board
```

`rill build --target esp32c3` writes an ELF32 RISC-V object — soft-float
`ilp32`, as ESP-IDF builds — and `build.sh` puts it and the runtime for the
chip (`cargo build --release -p rill_runtime --target
riscv32imc-unknown-none-elf`, or `riscv32imac` for the others) in `main/lib`
as two archives, then runs `idf.py`. `main/app.c` is the whole of the glue:
`app_main` calls the program's `main`, and `rill_host_write` hands the
runtime's standard output and error to ESP-IDF's streams, whose descriptors
are not 1 and 2.

To run it without a board, Espressif's QEMU (`python
$IDF_PATH/tools/idf_tools.py install qemu-riscv32`) runs the C3:

```
cd targets/esp32/build
python -m esptool --chip esp32c3 merge_bin --fill-flash-size 4MB -o flash.bin @flash_args
qemu-system-riscv32 -nographic -machine esp32c3 -drive file=flash.bin,if=mtd,format=raw
```

What a program may use: everything that needs no operating system —
data types, closures, lists, maps, strings, buffers, floats (in software),
`println`. Not strands, channels, sockets or files; a chip runs one worker.
The runtime keeps chip-sized buffers (`OUT_CAP`, `POOL_CHUNK`) there.

What it costs, measured on the C3 with the program in `hello.rill` style
(shapes, recursion, a sort, a map, printing): an empty ESP-IDF app is
174 KB; the same program in C, 176 KB; in Rill, 214 KB — the runtime 19 KB,
the program 5 KB, and what they draw from the C library (`strtod`, `%e`)
the rest. RAM: 3 KB of runtime statics, and the heap as the program uses it.

`../riscv32-virt` runs the same object on QEMU's generic `virt` machine with
no ESP-IDF at all — a start, the system calls newlib wants, and a linker
script — which is how the examples are checked on RV32.
