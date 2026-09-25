/* The ESP-IDF entry point: the Rill program's `main`, on this task, and
   its status said once it returns. */
#include <stdio.h>

int main(int argc, char **argv);

/* Where the Rill runtime sends standard output and error: ESP-IDF's own
   streams, whose descriptors are not 1 and 2. */
int rill_host_write(int fd, const void *buf, size_t n) {
    FILE *f = fd == 2 ? stderr : stdout;
    size_t w = fwrite(buf, 1, n, f);
    fflush(f);
    return (int)w;
}

void app_main(void) {
    int status = main(0, NULL);
    printf("rill: main returned %d\n", status);
}
