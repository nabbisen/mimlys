// SPIKE S1b (branch s1b-macos-seatbelt only, never merged): the follow-up probes B1, B2, B4
// (and a name-resolution helper for B5). Every line it prints starts with "S1b ".
// Build: clang -O0 -Wno-deprecated-declarations -o probeb probe_b.c
// The S1 probe (probe.c, mode `apply`) is still used for B3 and B5.
#define _DARWIN_C_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <sandbox.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void say(const char *key, const char *fmt, ...) {
    va_list ap;
    printf("S1b %s: ", key);
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

// applies the profile text of a file; returns sandbox_init's rc (-2: unreadable file)
static int apply_file(const char *path, const char *label) {
    char *text = slurp(path);
    if (!text) {
        say(label, "cannot read profile %s: %s", path, strerror(errno));
        return -2;
    }
    char *err = NULL;
    int rc = sandbox_init(text, 0, &err);
    say(label, "sandbox_init rc=%d err=%s", rc, err ? err : "(none)");
    if (err) sandbox_free_error(err);
    return rc;
}

// 1 when the profile is effective: a read the base profile denies fails
static int selfcheck(const char *label) {
    int fd = open("/private/etc/hosts", O_RDONLY);
    if (fd >= 0) {
        close(fd);
        say(label, "SELF-CHECK FAILED: /private/etc/hosts is readable, the profile is not effective");
        return 0;
    }
    say(label, "self-check ok: open(/private/etc/hosts) -> %s", strerror(errno));
    return 1;
}

static void pause_forever(void) {
    for (;;) pause();
}

typedef struct {
    int tag;
    pid_t pid;
} rec_t;
enum { TAG_G1 = 1, TAG_C = 2, TAG_G2 = 3, TAG_W2G = 9 };

static int send_rec(int fd, int tag, pid_t pid) {
    rec_t r;
    r.tag = tag;
    r.pid = pid;
    return write(fd, &r, sizeof r) == (ssize_t)sizeof r ? 0 : -1;
}

static int read_recs(int fd, rec_t *out, int want, int timeout_s) {
    int got = 0;
    time_t end = time(NULL) + timeout_s;
    while (got < want && time(NULL) < end) {
        struct pollfd p;
        p.fd = fd;
        p.events = POLLIN;
        p.revents = 0;
        int pr = poll(&p, 1, 200);
        if (pr > 0) {
            rec_t r;
            ssize_t n = read(fd, &r, sizeof r);
            if (n == (ssize_t)sizeof r) out[got++] = r;
            else if (n == 0) break;
        }
    }
    return got;
}

typedef struct {
    pid_t t, c, g1, g2;  // tool, its child, orphan of the tool (tool alive), orphan of a tool that has exited
} tree_t;

// The tool tree: T (setsid, alive) with a child C and a double-forked orphan G1 re-parented to
// launchd; and G2, the orphan of a tool T2 that has exited (reaped here). Called by the process
// that stands for the worker (already inside the profile).
static int build_tree(tree_t *tr) {
    memset(tr, 0, sizeof *tr);
    int pp[2];
    if (pipe(pp) != 0) return -1;
    pid_t t = fork();
    if (t < 0) {
        say("tree.fork", "fork failed: %s", strerror(errno));
        return -1;
    }
    if (t == 0) {
        setsid();
        pid_t c = fork();
        if (c == 0) pause_forever();
        send_rec(pp[1], TAG_C, c);
        pid_t x = fork();
        if (x == 0) {
            pid_t g1 = fork();
            if (g1 == 0) pause_forever();
            send_rec(pp[1], TAG_G1, g1);
            _exit(0);
        }
        waitpid(x, NULL, 0);
        pause_forever();
    }
    tr->t = t;
    pid_t t2 = fork();
    if (t2 == 0) {
        setsid();
        pid_t g2 = fork();
        if (g2 == 0) pause_forever();
        send_rec(pp[1], TAG_G2, g2);
        _exit(0);
    }
    waitpid(t2, NULL, 0);
    rec_t rs[3];
    int n = read_recs(pp[0], rs, 3, 10);
    for (int i = 0; i < n; i++) {
        if (rs[i].tag == TAG_G1) tr->g1 = rs[i].pid;
        if (rs[i].tag == TAG_C) tr->c = rs[i].pid;
        if (rs[i].tag == TAG_G2) tr->g2 = rs[i].pid;
    }
    usleep(500000);  // let the intermediate processes exit and the orphans be re-parented
    return n == 3 ? 0 : -1;
}

static void describe(const char *key, const char *name, pid_t pid) {
    struct proc_bsdinfo bi;
    memset(&bi, 0, sizeof bi);
    int n = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &bi, sizeof bi);
    int k = kill(pid, 0);
    int ke = errno;
    if (n > 0)
        say(key, "%s pid %d: proc_pidinfo ok ppid=%u status=%u; kill(pid,0) -> %s", name, pid, bi.pbi_ppid, bi.pbi_status, k == 0 ? "ok" : strerror(ke));
    else
        say(key, "%s pid %d: proc_pidinfo failed (%s); kill(pid,0) -> %s", name, pid, strerror(errno), k == 0 ? "ok" : strerror(ke));
}

// ---- B1 ---------------------------------------------------------------------
// The worker stand-in. Sends kill(-1, SIGKILL) only when the profile is effective AND signalling
// every guard (launchd, the parent outside the sandbox, a bystander, the ancestors of the job
// shell and the runner agent) fails EPERM.
static int b1_worker(const char *prof, const char *guards, const char *label) {
    char key[64];
    snprintf(key, sizeof key, "b1.%s", label);
    if (apply_file(prof, key) != 0) return 77;
    if (!selfcheck(key)) {
        say(key, "ABORT: nothing was sent");
        return 78;
    }
    int ok = 1;
    char *copy = strdup(guards);
    char *save = NULL;
    for (char *tok = strtok_r(copy, ",", &save); tok; tok = strtok_r(NULL, ",", &save)) {
        int pid = atoi(tok);
        const char *name = strchr(tok, ':') ? strchr(tok, ':') + 1 : "?";
        if (pid <= 0) continue;
        // ancestors of the job shell and the runner agent are soft guards: one that has exited is skipped
        int soft = !strncmp(name, "ancestor_", 9) || !strncmp(name, "runner", 6);
        errno = 0;
        int r = kill(pid, 0);
        int e = errno;
        say(key, "precheck kill(%d,0) [%s] -> %s", pid, name, r == 0 ? "0 (NOT REFUSED)" : strerror(e));
        if (r == 0 || (e != EPERM && !(soft && e == ESRCH))) ok = 0;
        if (!strcmp(name, "bystander")) {
            errno = 0;
            r = kill(pid, SIGCONT);  // a real, harmless signal (the target is running)
            e = errno;
            say(key, "precheck kill(%d,SIGCONT) [%s] -> %s", pid, name, r == 0 ? "0 (NOT REFUSED)" : strerror(e));
            if (r == 0 || e != EPERM) ok = 0;
        }
    }
    free(copy);
    pid_t parent = getppid();
    errno = 0;
    int r = kill(parent, 0);
    int e = errno;
    say(key, "precheck kill(%d,0) [parent outside the sandbox] -> %s", parent, r == 0 ? "0 (NOT REFUSED)" : strerror(e));
    if (r == 0 || e != EPERM) ok = 0;
    errno = 0;
    r = kill(parent, SIGCONT);  // a real, harmless signal (the parent is running)
    e = errno;
    say(key, "precheck kill(%d,SIGCONT) [parent outside the sandbox] -> %s", parent, r == 0 ? "0 (NOT REFUSED)" : strerror(e));
    if (r == 0 || e != EPERM) ok = 0;
    errno = 0;
    r = kill(1, 0);
    e = errno;
    say(key, "precheck kill(1,0) [launchd] -> %s", r == 0 ? "0 (NOT REFUSED)" : strerror(e));
    if (r == 0 || e != EPERM) ok = 0;

    tree_t tr;
    int tb = build_tree(&tr);
    say(key, "tree built=%s pids: T=%d C=%d G1=%d G2=%d", tb == 0 ? "yes" : "NO", tr.t, tr.c, tr.g1, tr.g2);
    describe(key, "before: tool T", tr.t);
    describe(key, "before: child C of T", tr.c);
    describe(key, "before: orphan G1 (tool alive)", tr.g1);
    describe(key, "before: orphan G2 (tool exited)", tr.g2);

    if (!ok || tb != 0) {
        say(key, "PRECHECK_FAILED or tree missing: kill(-1,SIGKILL) NOT SENT");
        if (tr.t > 0) kill(tr.t, SIGKILL);
        if (tr.c > 0) kill(tr.c, SIGKILL);
        if (tr.g1 > 0) kill(tr.g1, SIGKILL);
        if (tr.g2 > 0) kill(tr.g2, SIGKILL);
        return 0;
    }
    say(key, "precheck passed: sending kill(-1,SIGKILL) from the stand-in (pid %d)", getpid());
    errno = 0;
    r = kill(-1, SIGKILL);
    e = errno;
    say(key, "kill(-1,SIGKILL) -> %s", r == 0 ? "0" : strerror(e));
    // reap T (our child) and wait for the others to go
    int st = 0, reaped = 0;
    for (int i = 0; i < 60 && !reaped; i++) {
        if (waitpid(tr.t, &st, WNOHANG) == tr.t) reaped = 1;
        else usleep(100000);
    }
    say(key, "tool T reaped=%s signal=%d", reaped ? "yes" : "NO", reaped && WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    pid_t pids[3] = {tr.c, tr.g1, tr.g2};
    const char *names[3] = {"child C", "orphan G1", "orphan G2"};
    for (int i = 0; i < 3; i++) {
        int gone = 0;
        for (int k = 0; k < 40 && !gone; k++) {
            errno = 0;
            if (kill(pids[i], 0) != 0 && errno == ESRCH) gone = 1;
            else usleep(100000);
        }
        errno = 0;
        int kk = kill(pids[i], 0);
        say(key, "after: %s pid %d: %s (kill(pid,0) -> %s)", names[i], pids[i], gone ? "GONE" : "STILL THERE", kk == 0 ? "ok" : strerror(errno));
    }
    say(key, "after: the stand-in (pid %d) is alive (this line is printed by it)", getpid());
    // cleanup of anything left
    kill(tr.t, SIGKILL);
    for (int i = 0; i < 3; i++) kill(pids[i], SIGKILL);
    return 0;
}

static int mode_b1(char **a) {  // b1 <profile> <ws> <guards> <label>
    const char *prof = a[0], *guards = a[2], *label = a[3];
    // a second worker in a different sandbox (the same profile text applied independently)
    int p2[2];
    if (pipe(p2) != 0) return 2;
    pid_t w2 = fork();
    if (w2 == 0) {
        if (apply_file(prof, "b1.w2") != 0) _exit(77);
        pid_t g = fork();
        if (g == 0) pause_forever();
        send_rec(p2[1], TAG_W2G, g);
        pause_forever();
    }
    rec_t r;
    pid_t w2g = read_recs(p2[0], &r, 1, 10) == 1 ? r.pid : -1;
    say("b1.main", "label=%s main pid %d; second worker (own sandbox) pid %d with child %d", label, getpid(), w2, w2g);
    pid_t w = fork();
    if (w == 0) _exit(b1_worker(prof, guards, label));
    int st = 0;
    waitpid(w, &st, 0);
    say("b1.main", "stand-in pid %d exit=%d signal=%d", w, WIFEXITED(st) ? WEXITSTATUS(st) : -1, WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    usleep(500000);
    int a1 = kill(w2, 0) == 0, a2 = w2g > 0 && kill(w2g, 0) == 0;
    say("b1.main", "second worker (other sandbox) alive after the send: %s; its child alive: %s", a1 ? "yes" : "NO", a2 ? "yes" : "NO");
    say("b1.main", "main process (the stand-in's parent, outside the sandbox) is alive: pid %d", getpid());
    kill(w2, SIGKILL);
    if (w2g > 0) kill(w2g, SIGKILL);
    waitpid(w2, NULL, 0);
    return 0;
}

// ---- B2 ---------------------------------------------------------------------
static int is_member(const tree_t *tr, pid_t self, pid_t p) {
    return p == self || p == tr->t || p == tr->c || p == tr->g1 || p == tr->g2;
}

static int mode_b2(char **a) {  // b2 <profile> <label>
    const char *label = a[1];
    char key[64];
    snprintf(key, sizeof key, "b2.%s", label);
    int rc = apply_file(a[0], key);
    if (rc != 0) {
        say(key, "RESULT: the profile was not accepted (rc=%d): that is the answer for this form", rc);
        return 0;
    }
    if (!selfcheck(key)) return 78;
    tree_t tr;
    int tb = build_tree(&tr);
    pid_t self = getpid();
    say(key, "tree built=%s: members are self=%d T=%d C=%d G1=%d G2=%d (5 expected)", tb == 0 ? "yes" : "NO", self, tr.t, tr.c, tr.g1, tr.g2);
    errno = 0;
    int n = proc_listallpids(NULL, 0);
    say(key, "proc_listallpids size query -> %d (%s)", n, n <= 0 ? strerror(errno) : "ok");
    if (n > 0) {
        pid_t *pids = calloc(n + 512, sizeof(pid_t));
        int m = proc_listallpids(pids, (n + 512) * (int)sizeof(pid_t));
        int present = 0, ok0 = 0, oki = 0, okboth_nonmember = 0;
        unsigned long long rss = 0;
        for (int i = 0; i < m; i++) {
            if (is_member(&tr, self, pids[i])) present++;
            int k = kill(pids[i], 0) == 0;
            struct proc_bsdinfo bi;
            int pi = proc_pidinfo(pids[i], PROC_PIDTBSDINFO, 0, &bi, sizeof bi) > 0;
            if (k) ok0++;
            if (pi) {
                oki++;
                struct proc_taskinfo ti;
                memset(&ti, 0, sizeof ti);
                if (proc_pidinfo(pids[i], PROC_PIDTASKINFO, 0, &ti, sizeof ti) > 0) rss += ti.pti_resident_size;
            }
            if ((k || pi) && !is_member(&tr, self, pids[i])) okboth_nonmember++;
        }
        say(key, "proc_listallpids returned %d pids; members among them: %d of 5", m, present);
        say(key, "membership oracle over the list: kill(pid,0)==0 for %d pids; proc_pidinfo(BSDINFO) ok for %d pids; non-members that pass either: %d", ok0, oki, okboth_nonmember);
        say(key, "resident size summed over the pids proc_pidinfo can read: %llu KiB", rss >> 10);
    }
    pid_t kids[64];
    int kc = proc_listchildpids(self, kids, sizeof kids);
    say(key, "proc_listchildpids(self) -> %d", kc);
    kc = proc_listchildpids(tr.t, kids, sizeof kids);
    say(key, "proc_listchildpids(T) -> %d", kc);
    pid_t buf[4096];
    errno = 0;
    int pg = proc_listpids(PROC_PGRP_ONLY, (uint32_t)getpgrp(), buf, sizeof buf);
    say(key, "proc_listpids(PROC_PGRP_ONLY, own pgrp) -> %d bytes (%s)", pg, pg < 0 ? strerror(errno) : "ok");
    errno = 0;
    int pa = proc_listpids(PROC_ALL_PIDS, 0, buf, sizeof buf);
    say(key, "proc_listpids(PROC_ALL_PIDS) -> %d bytes = %d pids (%s; the buffer holds at most 4096 pids)", pa, pa > 0 ? pa / (int)sizeof(pid_t) : 0, pa <= 0 ? strerror(errno) : "ok");
    // the other way to list every process: sysctl kern.proc.all
    int mib[3] = {CTL_KERN, KERN_PROC, KERN_PROC_ALL};
    size_t sz = 0;
    errno = 0;
    int sr = sysctl(mib, 3, NULL, &sz, NULL, 0);
    if (sr != 0) {
        say(key, "sysctl KERN_PROC_ALL size query failed: %s", strerror(errno));
    } else {
        size_t cap = sz + 64 * sizeof(struct kinfo_proc);
        struct kinfo_proc *kp = malloc(cap);
        size_t got = cap;
        errno = 0;
        sr = sysctl(mib, 3, kp, &got, NULL, 0);
        if (sr != 0) {
            say(key, "sysctl KERN_PROC_ALL read failed: %s", strerror(errno));
        } else {
            size_t cnt = got / sizeof(struct kinfo_proc), mem = 0, non = 0;
            char sample[256] = "";
            for (size_t i = 0; i < cnt; i++) {
                pid_t p = kp[i].kp_proc.p_pid;
                if (is_member(&tr, self, p)) mem++;
                else {
                    non++;
                    if (strlen(sample) < 150) {
                        char one[48];
                        snprintf(one, sizeof one, " %d=%s", p, kp[i].kp_proc.p_comm);
                        strncat(sample, one, sizeof sample - strlen(sample) - 1);
                    }
                }
            }
            say(key, "sysctl KERN_PROC_ALL: %zu entries; members %zu; non-members %zu (names visible, e.g.:%s)", cnt, mem, non, sample);
        }
    }
    kill(tr.t, SIGKILL);
    kill(tr.c, SIGKILL);
    kill(tr.g1, SIGKILL);
    kill(tr.g2, SIGKILL);
    return 0;
}

// ---- B4 ---------------------------------------------------------------------
static int mode_b4(char **a) {  // b4 <profile|-> <port> <tag>
    int port = atoi(a[1]);
    const char *tag = a[2];
    if (strcmp(a[0], "-") != 0 && apply_file(a[0], "b4.apply") != 0) return 77;
    if (strcmp(a[0], "-") != 0) selfcheck("b4");
    char payload[128];
    snprintf(payload, sizeof payload, "S1b-%s", tag);
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof sa);
    sa.sin_family = AF_INET;
    sa.sin_port = htons(port);
    inet_pton(AF_INET, "127.0.0.1", &sa.sin_addr);
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    if (u < 0) {
        say("b4.udp_socket", "-> %s", strerror(errno));
    } else {
        say("b4.udp_socket", "-> created");
        errno = 0;
        ssize_t r = sendto(u, payload, strlen(payload), 0, (struct sockaddr *)&sa, sizeof sa);
        say("b4.sendto_loopback_v4", "-> %s", r >= 0 ? "SENT (rc>=0)" : strerror(errno));
        int c = connect(u, (struct sockaddr *)&sa, sizeof sa);
        say("b4.udp_connect_loopback_v4", "-> %s", c == 0 ? "ok" : strerror(errno));
        if (c == 0) {
            errno = 0;
            r = send(u, payload, strlen(payload), 0);
            say("b4.send_after_connect_v4", "-> %s", r >= 0 ? "SENT (rc>=0)" : strerror(errno));
        }
        close(u);
    }
    int u6 = socket(AF_INET6, SOCK_DGRAM, 0);
    if (u6 < 0) {
        say("b4.udp6_socket", "-> %s", strerror(errno));
    } else {
        struct sockaddr_in6 s6;
        memset(&s6, 0, sizeof s6);
        s6.sin6_family = AF_INET6;
        s6.sin6_port = htons(port);
        inet_pton(AF_INET6, "::1", &s6.sin6_addr);
        errno = 0;
        ssize_t r = sendto(u6, payload, strlen(payload), 0, (struct sockaddr *)&s6, sizeof s6);
        say("b4.sendto_loopback_v6", "-> %s", r >= 0 ? "SENT (rc>=0)" : strerror(errno));
        close(u6);
    }
    int ub = socket(AF_INET, SOCK_DGRAM, 0);
    if (ub >= 0) {
        struct sockaddr_in lb;
        memset(&lb, 0, sizeof lb);
        lb.sin_family = AF_INET;
        inet_pton(AF_INET, "127.0.0.1", &lb.sin_addr);
        errno = 0;
        int b = bind(ub, (struct sockaddr *)&lb, sizeof lb);
        say("b4.udp_bind_loopback", "-> %s", b == 0 ? "ok" : strerror(errno));
        close(ub);
    }
    // an address that is not loopback: TEST-NET-1 (RFC 5737), unroutable, nothing answers
    int ux = socket(AF_INET, SOCK_DGRAM, 0);
    if (ux >= 0) {
        struct sockaddr_in tn;
        memset(&tn, 0, sizeof tn);
        tn.sin_family = AF_INET;
        tn.sin_port = htons(9);
        inet_pton(AF_INET, "192.0.2.1", &tn.sin_addr);
        errno = 0;
        ssize_t r = sendto(ux, payload, strlen(payload), 0, (struct sockaddr *)&tn, sizeof tn);
        say("b4.sendto_testnet", "-> %s", r >= 0 ? "SENT (rc>=0)" : strerror(errno));
        close(ux);
    }
    return 0;
}

// ---- B5 helper --------------------------------------------------------------
static int mode_resolve(char **a) {  // resolve <profile|-> <host>
    if (strcmp(a[0], "-") != 0 && apply_file(a[0], "resolve.apply") != 0) return 77;
    struct addrinfo hints, *res = NULL;
    memset(&hints, 0, sizeof hints);
    hints.ai_socktype = SOCK_STREAM;
    int r = getaddrinfo(a[1], "443", &hints, &res);
    say("resolve", "getaddrinfo(%s) -> %s", a[1], r == 0 ? "RESOLVED" : gai_strerror(r));
    if (res) freeaddrinfo(res);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    const char *m = argv[1];
    char **a = argv + 2;
    if (!strcmp(m, "b1") && argc >= 6) return mode_b1(a);
    if (!strcmp(m, "b2") && argc >= 4) return mode_b2(a);
    if (!strcmp(m, "b4") && argc >= 5) return mode_b4(a);
    if (!strcmp(m, "resolve") && argc >= 4) return mode_resolve(a);
    return 2;
}
