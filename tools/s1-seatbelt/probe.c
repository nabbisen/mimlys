// SPIKE S1 (cycle 2; branch s1-macos-seatbelt only, never merged): what Seatbelt
// does on macOS. One binary, many modes; every line it prints starts with "S1 ".
// Build: clang -O0 -Wno-deprecated-declarations -o probe probe.c
#define _DARWIN_C_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <libkern/OSCacheControl.h>
#include <libproc.h>
#include <netdb.h>
#include <netinet/in.h>
#include <pthread.h>
#include <sandbox.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static void say(const char *key, const char *fmt, ...) {
    va_list ap;
    printf("S1 %s: ", key);
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
    fflush(stdout);
}

static char *slurp(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) return NULL;
    char *buf = malloc(1 << 16);
    size_t n = fread(buf, 1, (1 << 16) - 1, f);
    buf[n] = 0;
    fclose(f);
    return buf;
}

static int apply_text(const char *text, const char *label) {
    char *err = NULL;
    int rc = sandbox_init(text, 0, &err);
    say(label, "sandbox_init rc=%d err=%s", rc, err ? err : "(none)");
    if (err) sandbox_free_error(err);
    return rc;
}

static int apply_file(const char *path, const char *label) {
    char *text = slurp(path);
    if (!text) {
        say(label, "cannot read profile %s: %s", path, strerror(errno));
        return -2;
    }
    return apply_text(text, label);
}

static void report_open(const char *key, const char *path) {
    int fd = open(path, O_RDONLY);
    say(key, "open(%s) -> %s", path, fd >= 0 ? "ok" : strerror(errno));
    if (fd >= 0) close(fd);
}

static void report_create(const char *key, const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT, 0600);
    say(key, "create(%s) -> %s", path, fd >= 0 ? "ok" : strerror(errno));
    if (fd >= 0) close(fd);
}

// ---- Q1 ---------------------------------------------------------------
static int mode_selftest(char **a) {  // selftest <profile> <ws>
    report_open("q1.pre", "/private/etc/hosts");
    apply_text("(version 1)(deny defaultt)", "q1.syntax_error_profile");
    if (apply_file(a[0], "q1.apply") != 0) return 77;
    report_open("q1.post.etc_hosts(denied expected)", "/private/etc/hosts");
    report_open("q1.post.usr_lib_dyld(allowed expected)", "/usr/lib/dyld");
    char p[1024];
    snprintf(p, sizeof p, "%s/q1.txt", a[1]);
    report_create("q1.post.ws_write(allowed expected)", p);
    snprintf(p, sizeof p, "%s/../q1-outside.txt", a[1]);
    report_create("q1.post.outside_write(denied expected)", p);
    return 0;
}

// ---- Q2 ---------------------------------------------------------------
static void pair(const char *label) {
    char k[128];
    snprintf(k, sizeof k, "%s.usr_share(A allows, B denies)", label);
    report_open(k, "/usr/share/misc/magic");
    snprintf(k, sizeof k, "%s.etc_hosts(A denies, B allows)", label);
    report_open(k, "/private/etc/hosts");
}

static int mode_stack(char **a) {  // stack <profileA> <profileB> <self>
    if (apply_file(a[0], "q2.applyA") != 0) return 77;
    pair("q2.afterA");
    int rc = apply_file(a[1], "q2.applyB_inproc");
    say("q2.inproc", "second sandbox_init rc=%d", rc);
    pair("q2.inproc.afterB");
    pid_t c = fork();
    if (c == 0) {
        int r = apply_file(a[1], "q2.applyB_child");
        say("q2.child", "second sandbox_init in forked child rc=%d", r);
        pair("q2.child.afterB");
        _exit(0);
    }
    waitpid(c, NULL, 0);
    c = fork();
    if (c == 0) {
        execl("/usr/bin/sandbox-exec", "sandbox-exec", "-f", a[1], a[2], "pair", "q2.sandbox_exec_child", (char *)NULL);
        say("q2.sandbox_exec", "exec failed: %s", strerror(errno));
        _exit(99);
    }
    int st = 0;
    waitpid(c, &st, 0);
    say("q2.sandbox_exec", "child exit status=%d signal=%d", WIFEXITED(st) ? WEXITSTATUS(st) : -1, WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    return 0;
}

// ---- Q3 ---------------------------------------------------------------
static pid_t read_pidfile(const char *path, int wait_s) {
    for (int i = 0; i < wait_s * 20; i++) {
        FILE *f = fopen(path, "r");
        if (f) {
            int p = 0;
            int n = fscanf(f, "%d", &p);
            fclose(f);
            if (n == 1 && p > 0) return p;
        }
        usleep(50000);
    }
    return -1;
}

static void write_pidfile(const char *path, pid_t pid) {
    char tmp[1100];
    snprintf(tmp, sizeof tmp, "%s.tmp", path);
    FILE *f = fopen(tmp, "w");
    if (!f) {
        say("q3.pidfile", "cannot write %s: %s", tmp, strerror(errno));
        return;
    }
    fprintf(f, "%d\n", pid);
    fclose(f);
    rename(tmp, path);
}

static void probe_pid(const char *key, pid_t pid) {
    struct proc_bsdinfo bi;
    memset(&bi, 0, sizeof bi);
    int n = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &bi, sizeof bi);
    if (n > 0)
        say(key, "pid %d: proc_pidinfo ok ppid=%u", pid, bi.pbi_ppid);
    else
        say(key, "pid %d: proc_pidinfo failed: %s", pid, strerror(errno));
    int r = kill(pid, 0);
    say(key, "pid %d: kill(pid,0) -> %s", pid, r == 0 ? "ok (signalable)" : strerror(errno));
}

static int mode_q3(char **a) {  // q3 <profile> <tag> <ws> <othertag> <outsidepid>
    const char *tag = a[1], *ws = a[2], *other = a[3];
    pid_t outside = atoi(a[4]);
    if (apply_file(a[0], "q3.apply") != 0) return 77;
    int p[2];
    if (pipe(p) != 0) return 2;
    pid_t t = fork();
    if (t < 0) {
        say("q3.fork", "fork failed: %s", strerror(errno));
        return 3;
    }
    if (t == 0) {
        close(p[0]);
        setsid();
        pid_t g = fork();
        if (g == 0) {
            close(p[1]);
            for (;;) pause();
        }
        write(p[1], &g, sizeof g);
        _exit(0);
    }
    close(p[1]);
    pid_t g = 0;
    read(p[0], &g, sizeof g);
    waitpid(t, NULL, 0);
    usleep(400000);  // the tool has exited; the orphan is re-parented
    char path[1100];
    snprintf(path, sizeof path, "%s/%s.pid", ws, tag);
    write_pidfile(path, g);
    say("q3.orphan", "tag=%s orphan pid=%d (tool pid %d has exited)", tag, g, t);
    probe_pid("q3.orphan", g);
    // listing
    int n = proc_listallpids(NULL, 0);
    say("q3.listallpids", "size query -> %d (errno %s)", n, n <= 0 ? strerror(errno) : "-");
    if (n > 0) {
        pid_t *pids = calloc(n + 256, sizeof(pid_t));
        int m = proc_listallpids(pids, (n + 256) * (int)sizeof(pid_t));
        int found = 0;
        for (int i = 0; i < m; i++)
            if (pids[i] == g) found = 1;
        say("q3.listallpids", "returned %d pids; contains the orphan: %s", m, found ? "yes" : "no");
    }
    pid_t kids[64];
    int kb = proc_listchildpids(getpid(), kids, sizeof kids);
    say("q3.listchildpids(self)", "-> %d (%s)", kb, kb < 0 ? strerror(errno) : "ok");
    kb = proc_listchildpids(1, kids, sizeof kids);
    say("q3.listchildpids(1)", "-> %d (%s)", kb, kb < 0 ? strerror(errno) : "ok");
    // outside process and kill(-1, 0)
    probe_pid("q3.outside", outside);
    int r = kill(-1, 0);
    say("q3.kill(-1,0)", "-> %s", r == 0 ? "ok (some process is signalable)" : strerror(errno));
    // the other worker's orphan (same profile text, applied independently)
    char opath[1100];
    snprintf(opath, sizeof opath, "%s/%s.pid", ws, other);
    pid_t og = read_pidfile(opath, 8);
    if (og > 0) probe_pid("q3.other_worker_orphan", og);
    else say("q3.other_worker_orphan", "no pid file from %s", other);
    // barrier: do not kill before the other has looked
    snprintf(path, sizeof path, "%s/%s.done", ws, tag);
    write_pidfile(path, 1);
    snprintf(opath, sizeof opath, "%s/%s.done", ws, other);
    read_pidfile(opath, 8);
    r = kill(g, SIGKILL);
    say("q3.kill_own_orphan", "kill(%d,SIGKILL) -> %s", g, r == 0 ? "ok" : strerror(errno));
    usleep(400000);
    r = kill(g, 0);
    say("q3.kill_own_orphan", "after: kill(%d,0) -> %s", g, r == 0 ? "STILL ALIVE" : strerror(errno));
    return 0;
}

// ---- Q4 ---------------------------------------------------------------
static void jit_try(int mapjit) {
    size_t len = 16384;
    void *p;
    if (mapjit) {
#ifdef MAP_JIT
        p = mmap(0, len, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
#else
        p = MAP_FAILED;
#endif
    } else {
        p = mmap(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    }
    if (p == MAP_FAILED) {
        say("q4.step", "mmap failed: %s", strerror(errno));
        _exit(3);
    }
#if defined(__arm64__) || defined(__aarch64__)
    unsigned int insn = 0xd65f03c0;  // ret
    if (mapjit) pthread_jit_write_protect_np(0);
    memcpy(p, &insn, 4);
    if (mapjit) pthread_jit_write_protect_np(1);
#else
    unsigned char insn = 0xc3;  // ret
    memcpy(p, &insn, 1);
#endif
    if (!mapjit && mprotect(p, len, PROT_READ | PROT_EXEC) != 0) {
        say("q4.step", "mprotect(RX) failed: %s", strerror(errno));
        _exit(4);
    }
    sys_icache_invalidate(p, 4);
    ((void (*)(void))p)();
    say("q4.step", "generated code ran");
    _exit(0);
}

static int mode_jit(char **a) {  // jit <profile>
    if (apply_file(a[0], "q4.apply") != 0) return 77;
    for (int mapjit = 0; mapjit < 2; mapjit++) {
        pid_t c = fork();
        if (c == 0) jit_try(mapjit);
        int st = 0;
        waitpid(c, &st, 0);
        say("q4.result", "%s: exit=%d signal=%d", mapjit ? "MAP_JIT" : "mmap RW then mprotect RX", WIFEXITED(st) ? WEXITSTATUS(st) : -1, WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    }
    return 0;
}

// ---- Q6 ---------------------------------------------------------------
static int mode_net(char **a) {  // net <profile> <port> <ws>
    int port = atoi(a[1]);
    const char *ws = a[2];
    if (apply_file(a[0], "q6.apply") != 0) return 77;
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) say("q6.tcp_socket", "socket() failed: %s", strerror(errno));
    else {
        struct sockaddr_in sa;
        memset(&sa, 0, sizeof sa);
        sa.sin_family = AF_INET;
        sa.sin_port = htons(port);
        inet_pton(AF_INET, "127.0.0.1", &sa.sin_addr);
        int r = connect(s, (struct sockaddr *)&sa, sizeof sa);
        say("q6.tcp_connect_loopback", "-> %s", r == 0 ? "CONNECTED" : strerror(errno));
        close(s);
    }
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    say("q6.udp_socket", "-> %s", u >= 0 ? "created" : strerror(errno));
    if (u >= 0) close(u);
    const char *names[] = {"example.com", "localhost"};
    for (int i = 0; i < 2; i++) {
        struct addrinfo hints, *res = NULL;
        memset(&hints, 0, sizeof hints);
        hints.ai_socktype = SOCK_STREAM;
        int r = getaddrinfo(names[i], "443", &hints, &res);
        say("q6.getaddrinfo", "%s -> %s", names[i], r == 0 ? "RESOLVED" : gai_strerror(r));
        if (res) freeaddrinfo(res);
    }
    int sp[2];
    int r = socketpair(AF_UNIX, SOCK_STREAM, 0, sp);
    say("q6.socketpair", "-> %s", r == 0 ? "ok" : strerror(errno));
    // a Unix socket inside the workspace: bind, listen, connect, accept
    struct sockaddr_un su;
    memset(&su, 0, sizeof su);
    su.sun_family = AF_UNIX;
    snprintf(su.sun_path, sizeof su.sun_path, "%s/s1.sock", ws);
    unlink(su.sun_path);
    int ls = socket(AF_UNIX, SOCK_STREAM, 0);
    r = bind(ls, (struct sockaddr *)&su, sizeof su);
    say("q6.unix_bind_in_ws", "-> %s", r == 0 ? "ok" : strerror(errno));
    if (r == 0) {
        listen(ls, 1);
        int cs = socket(AF_UNIX, SOCK_STREAM, 0);
        r = connect(cs, (struct sockaddr *)&su, sizeof su);
        say("q6.unix_connect_in_ws", "-> %s", r == 0 ? "ok" : strerror(errno));
    }
    int ms = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un sm;
    memset(&sm, 0, sizeof sm);
    sm.sun_family = AF_UNIX;
    strcpy(sm.sun_path, "/private/var/run/mDNSResponder");
    r = connect(ms, (struct sockaddr *)&sm, sizeof sm);
    say("q6.unix_connect_mDNSResponder", "-> %s", r == 0 ? "CONNECTED" : strerror(errno));
    return 0;
}

// ---- Q9 ---------------------------------------------------------------
static void procargs(const char *key, pid_t pid) {
    int mib[3] = {CTL_KERN, KERN_PROCARGS2, pid};
    size_t sz = 0;
    int rc = sysctl(mib, 3, NULL, &sz, NULL, 0);
    if (rc != 0) {
        say(key, "pid %d: KERN_PROCARGS2 size query failed: %s", pid, strerror(errno));
        return;
    }
    char *buf = malloc(sz + 1);
    rc = sysctl(mib, 3, buf, &sz, NULL, 0);
    if (rc != 0) {
        say(key, "pid %d: KERN_PROCARGS2 read failed: %s", pid, strerror(errno));
        return;
    }
    int found = memmem(buf, sz, "S1_SECRET=", 10) != NULL;
    say(key, "pid %d: KERN_PROCARGS2 read %zu bytes; environment shows S1_SECRET: %s", pid, sz, found ? "YES" : "no");
}

static int mode_q9(char **a) {  // q9 <profile|-> <runtimepid>
    pid_t runtime = atoi(a[1]);
    if (strcmp(a[0], "-") != 0 && apply_file(a[0], "q9.apply") != 0) return 77;
    pid_t c = fork();
    if (c == 0) {
        size_t mb = 64;
        char *m = malloc(mb << 20);
        for (size_t i = 0; i < (mb << 20); i += 4096) m[i] = 1;
        sleep(4);
        _exit(0);
    }
    usleep(1500000);
    struct proc_taskinfo ti;
    memset(&ti, 0, sizeof ti);
    int n = proc_pidinfo(c, PROC_PIDTASKINFO, 0, &ti, sizeof ti);
    if (n > 0)
        say("q9.child_taskinfo", "pid %d: resident=%llu MiB virtual=%llu MiB", c, (unsigned long long)ti.pti_resident_size >> 20, (unsigned long long)ti.pti_virtual_size >> 20);
    else
        say("q9.child_taskinfo", "pid %d: proc_pidinfo(TASKINFO) failed: %s", c, strerror(errno));
    memset(&ti, 0, sizeof ti);
    n = proc_pidinfo(runtime, PROC_PIDTASKINFO, 0, &ti, sizeof ti);
    say("q9.outside_taskinfo", "pid %d: %s", runtime, n > 0 ? "READABLE" : strerror(errno));
    procargs("q9.procargs_runtime", runtime);
    procargs("q9.procargs_child", c);
    procargs("q9.procargs_self", getpid());
    waitpid(c, NULL, 0);
    return 0;
}

// ---- apply & exec ----------------------------------------------------------
static int mode_apply(int argc, char **argv) {  // apply <profile> -- cmd args...
    if (argc < 5 || strcmp(argv[3], "--") != 0) return 2;
    if (apply_file(argv[2], "apply") != 0) return 77;
    execvp(argv[4], argv + 4);
    say("apply", "exec %s failed: %s", argv[4], strerror(errno));
    return 98;
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    const char *m = argv[1];
    char **a = argv + 2;
    if (!strcmp(m, "selftest")) return mode_selftest(a);
    if (!strcmp(m, "stack")) return mode_stack(a);
    if (!strcmp(m, "pair")) {
        pair(a[0]);
        return 0;
    }
    if (!strcmp(m, "q3")) return mode_q3(a);
    if (!strcmp(m, "jit")) return mode_jit(a);
    if (!strcmp(m, "net")) return mode_net(a);
    if (!strcmp(m, "q9")) return mode_q9(a);
    if (!strcmp(m, "apply")) return mode_apply(argc, argv);
    return 2;
}
