/* The few system calls newlib and the Rill runtime want, on QEMU's `virt`
   machine: standard output and error go to its 16550 UART, the heap is the
   memory after the image, and an exit powers the machine off through the
   SiFive test device with the program's status. */
#include <errno.h>
#include <stddef.h>
#include <sys/stat.h>
#include <reent.h>

#define UART0 ((volatile unsigned char *)0x10000000)
#define TEST_DEV ((volatile unsigned int *)0x100000)

extern char __heap_start[], __heap_end[];
static char *brk = __heap_start;

int _write(int fd, const void *buf, size_t n) {
    const unsigned char *p = buf;
    for (size_t i = 0; i < n; i++) {
        while ((UART0[5] & 0x20) == 0) {}
        UART0[0] = p[i];
    }
    return (int)n;
}

/* Where the Rill runtime sends standard output and error. */
int rill_host_write(int fd, const void *buf, size_t n) { return _write(fd, buf, n); }

int _read(int fd, void *buf, size_t n) { return 0; }
int _close(int fd) { return -1; }
int _open(const char *path, int flags, int mode) { errno = ENOENT; return -1; }
long _lseek(int fd, long off, int whence) { return -1; }
int _isatty(int fd) { return fd < 3; }
int _fstat(int fd, struct stat *st) { st->st_mode = S_IFCHR; return 0; }
int _getpid(void) { return 1; }
int _kill(int pid, int sig) { errno = EINVAL; return -1; }

void *_sbrk(ptrdiff_t incr) {
    char *old = brk;
    if (brk + incr > __heap_end) { errno = ENOMEM; return (void *)-1; }
    brk += incr;
    return old;
}

/* Espressif's newlib asks the system for its reentrancy state, which
   ESP-IDF keeps per task; one program on one core has one. */
struct _reent *__getreent(void) { return _GLOBAL_REENT; }

void _exit(int code) {
    /* 0x5555 is a pass, (code << 16) | 0x3333 a failure with that code */
    *TEST_DEV = code == 0 ? 0x5555 : ((unsigned)code << 16) | 0x3333;
    for (;;) {}
}
