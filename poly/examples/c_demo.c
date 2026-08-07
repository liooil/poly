/* Poly C demo — compiled in-process with embedded TinyCC (bun:ffi cc).
   The entry file must define `int main(void)`; its return value becomes
   the process exit code. */
#include <stdio.h>

int main(void) {
  printf("Hello from TinyCC\n");
  return 7;
}
