/* trace-pc coverage runtime for Rust windows-gnu builds (see scripts/wincov.sh).
   __sanitizer_cov_trace_pc is called at the start of every basic block by code compiled with
   -C passes=sancov-module -C llvm-args=-sanitizer-coverage-trace-pc; it remembers the return
   address (as an offset from the image base). At process exit the offsets reached are appended,
   one hex number per line, to the file named by RUNGUI_COV_OUT. Only kernel32 is used: Rust
   links without the C runtime's I/O. */
#include <stdint.h>
#define SPAN (16u << 20)
static uint8_t seen[SPAN];
extern char __ImageBase;

void __sanitizer_cov_trace_pc(void) {
  uintptr_t rva = (uintptr_t)__builtin_return_address(0) - (uintptr_t)&__ImageBase;
  if (rva < SPAN) seen[rva] = 1;
}

__declspec(dllimport) unsigned long __stdcall GetEnvironmentVariableA(const char *, char *, unsigned long);
__declspec(dllimport) void *__stdcall CreateFileA(const char *, unsigned long, unsigned long, void *, unsigned long, unsigned long, void *);
__declspec(dllimport) int __stdcall WriteFile(void *, const void *, unsigned long, unsigned long *, void *);
__declspec(dllimport) int __stdcall CloseHandle(void *);
__declspec(dllimport) unsigned long __stdcall SetFilePointer(void *, long, long *, unsigned long);

__attribute__((destructor)) static void dump(void) {
  char path[1024];
  unsigned long n = GetEnvironmentVariableA("RUNGUI_COV_OUT", path, sizeof path);
  if (n == 0 || n >= sizeof path) return;
  /* GENERIC_WRITE, share read/write, OPEN_ALWAYS */
  void *f = CreateFileA(path, 0x40000000, 3, 0, 4, 0x80, 0);
  if (f == (void *)-1) return;
  SetFilePointer(f, 0, 0, 2); /* FILE_END: append */
  static char buf[1 << 16];
  unsigned long used = 0, w;
  for (uint32_t i = 0; i < SPAN; i++) {
    if (!seen[i]) continue;
    char tmp[16];
    int len = 0;
    uint32_t v = i;
    do { tmp[len++] = "0123456789abcdef"[v & 15]; v >>= 4; } while (v);
    if (used + len + 1 > sizeof buf) { WriteFile(f, buf, used, &w, 0); used = 0; }
    while (len) buf[used++] = tmp[--len];
    buf[used++] = '\n';
  }
  if (used) WriteFile(f, buf, used, &w, 0);
  CloseHandle(f);
}
