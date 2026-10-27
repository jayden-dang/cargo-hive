// Spike 4: record every file rustc (and the proc-macro dylibs it loads) opens.
// Injected with DYLD_INSERT_LIBRARIES; dyld's __interpose table rebinds the
// symbols in every image, including dylibs dlopen'd later, but not in this one,
// so the logging below can call the real open() without recursing.
#include <fcntl.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int log_fd = -1;

static void record(const char *kind, int dirfd, const char *path) {
  if (log_fd < 0) {
    const char *f = getenv("HIVE_TRACE_FILE");
    if (!f) return;
    log_fd = open(f, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0644);
    if (log_fd < 0) return;
  }
  char cwd[1024] = "";
  if (path[0] != '/' && dirfd == AT_FDCWD) getcwd(cwd, sizeof cwd);
  // One write() per line so concurrent rustc processes don't interleave.
  char line[4096];
  int n = 0;
  n += strlcpy(line + n, kind, sizeof line - n);
  n += strlcpy(line + n, "\t", sizeof line - n);
  if (cwd[0]) { n += strlcpy(line + n, cwd, sizeof line - n); n += strlcpy(line + n, "/", sizeof line - n); }
  n += strlcpy(line + n, path, sizeof line - n);
  if (n > (int)sizeof line - 2) n = sizeof line - 2;
  line[n++] = '\n';
  write(log_fd, line, n);
}

static int hive_open(const char *path, int flags, ...) {
  mode_t mode = 0;
  if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap); }
  if (path) record((flags & (O_WRONLY | O_RDWR)) ? "write" : "read", AT_FDCWD, path);
  return open(path, flags, mode);
}

static int hive_openat(int dirfd, const char *path, int flags, ...) {
  mode_t mode = 0;
  if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap); }
  if (path) record((flags & (O_WRONLY | O_RDWR)) ? "write" : "read", dirfd, path);
  return openat(dirfd, path, flags, mode);
}

__attribute__((used)) static struct { const void *replacement, *replacee; } interposers[]
    __attribute__((section("__DATA,__interpose"))) = {
        {(const void *)hive_open, (const void *)open},
        {(const void *)hive_openat, (const void *)openat},
};
