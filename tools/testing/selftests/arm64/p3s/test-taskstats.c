// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <errno.h>
#include <linux/genetlink.h>
#include <linux/taskstats.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#define P3S_4K 0x40000000UL
static void check(int ok, const char *why)
{
	if (!ok) { perror(why); exit(1); }
}
static int request(int fd, int family, int cmd, int type, const void *data,
		   size_t size, unsigned char *reply)
{
	unsigned char buf[512] = {0};
	struct nlmsghdr *n = (void *)buf;
	struct genlmsghdr *g = (void *)(n + 1);
	struct nlattr *a = (void *)(g + 1);
	struct sockaddr_nl to = { .nl_family = AF_NETLINK };
	int len;

	n->nlmsg_len = NLMSG_HDRLEN + GENL_HDRLEN + NLA_ALIGN(NLA_HDRLEN + size);
	n->nlmsg_type = family; n->nlmsg_flags = NLM_F_REQUEST; n->nlmsg_seq = cmd;
	g->cmd = cmd; g->version = 1;
	a->nla_type = type; a->nla_len = NLA_HDRLEN + size;
	memcpy((char *)a + NLA_HDRLEN, data, size);
	check(sendto(fd, buf, n->nlmsg_len, 0, (void *)&to, sizeof(to)) == (int)n->nlmsg_len, "netlink send");
	len = recv(fd, reply, 16384, 0); check(len >= (int)NLMSG_HDRLEN, "netlink recv");
	n = (void *)reply;
	if (n->nlmsg_type == NLMSG_ERROR) {
		errno = -((struct nlmsgerr *)NLMSG_DATA(n))->error;
		if (errno == EPERM || errno == EACCES || errno == ENOENT) {
			puts("SKIP: taskstats requires NET_ADMIN and enabled kernel support"); exit(4);
		}
		check(0, "netlink error");
	}
	return len;
}
static void stats(struct taskstats *out)
{
	unsigned char buf[16384];
	struct nlattr *a, *b;
	struct timeval tv = { .tv_sec = 3 };
	uint16_t family = 0;
	uint32_t pid = getpid();
	int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC), len, left;
	const char name[] = TASKSTATS_GENL_NAME;

	check(fd >= 0, "netlink socket");
	check(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)) == 0, "recv timeout");
	len = request(fd, GENL_ID_CTRL, CTRL_CMD_GETFAMILY, CTRL_ATTR_FAMILY_NAME, name, sizeof(name), buf);
	for (int off = NLMSG_HDRLEN + GENL_HDRLEN; off + NLA_HDRLEN <= len; off += NLA_ALIGN(a->nla_len)) {
		a = (void *)(buf + off); check(a->nla_len >= NLA_HDRLEN && off + a->nla_len <= len, "family attr");
		if (a->nla_type == CTRL_ATTR_FAMILY_ID) memcpy(&family, (char *)a + NLA_HDRLEN, sizeof(family));
	}
	check(family != 0, "taskstats family");
	len = request(fd, family, TASKSTATS_CMD_GET, TASKSTATS_CMD_ATTR_PID, &pid, sizeof(pid), buf);
	for (int off = NLMSG_HDRLEN + GENL_HDRLEN; off + NLA_HDRLEN <= len; off += NLA_ALIGN(a->nla_len)) {
		a = (void *)(buf + off); check(a->nla_len >= NLA_HDRLEN && off + a->nla_len <= len, "taskstats attr");
		if ((a->nla_type & NLA_TYPE_MASK) != TASKSTATS_TYPE_AGGR_PID) continue;
		left = a->nla_len - NLA_HDRLEN;
		for (int pos = 0; pos + NLA_HDRLEN <= left; pos += NLA_ALIGN(b->nla_len)) {
			b = (void *)((char *)a + NLA_HDRLEN + pos);
			check(b->nla_len >= NLA_HDRLEN && pos + b->nla_len <= left, "nested attr");
			if ((b->nla_type & NLA_TYPE_MASK) != TASKSTATS_TYPE_STATS) continue;
			size_t bytes = b->nla_len - NLA_HDRLEN;

			memset(out, 0, sizeof(*out));
			memcpy(out, (char *)b + NLA_HDRLEN, bytes < sizeof(*out) ? bytes : sizeof(*out));
			close(fd); return;
		}
	}
	check(0, "no taskstats payload");
}
static double cpu_seconds(void)
{
	struct timespec t;
	check(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &t) == 0, "clock");
	return t.tv_sec + t.tv_nsec * 1e-9;
}
int main(int argc, char **argv)
{
	int after = argc == 2 && !strcmp(argv[1], "--after");
	size_t size = (after ? 16UL : 64UL) << 20, page = sysconf(_SC_PAGESIZE);
	volatile unsigned char *p = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
	struct taskstats s;
	double end, usecs, avg, vavg;

	setvbuf(stdout, NULL, _IONBF, 0);
	check(p != MAP_FAILED, "mmap"); (void)madvise((void *)p, size, MADV_NOHUGEPAGE);
	for (size_t i = 0; i < size; i += page) p[i] = 0x19;
	end = cpu_seconds() + 1;
	while (cpu_seconds() < end) __asm__ volatile("" ::: "memory");
	usleep(20000); /* force accounting of the preceding CPU interval */
	if (!after) {
		int old = personality(0xffffffffUL);
		stats(&s); /* fail/skip before changing geometry if netlink is unavailable */
		printf("phase1 page=%zu RSS peak=%llu KiB VM peak=%llu KiB\n", page, (unsigned long long)s.hiwater_rss, (unsigned long long)s.hiwater_vm);
		check(s.hiwater_rss > 56 * 1024 && s.hiwater_rss < 80 * 1024, "first peak");
		check(old >= 0 && personality((unsigned int)old ^ P3S_4K) >= 0, "toggle geometry");
		execl("/proc/self/exe", argv[0], "--after", NULL); check(0, "exec");
	}
	stats(&s);
	usecs = s.ac_utime + s.ac_stime;
	check(usecs > 1000000, "CPU time survived exec");
	avg = s.coremem / usecs;
	vavg = s.virtmem / usecs;
	printf("phase2 page=%zu peak=%llu KiB CPU=%.0f us coremem=%llu MiB-us avg=%.2f MiB\n", page, (unsigned long long)s.hiwater_rss, usecs, (unsigned long long)s.coremem, avg);
	check(s.hiwater_rss > 12 * 1024 && s.hiwater_rss < 32 * 1024, "second peak");
	check(s.hiwater_vm > 16 * 1024 && s.hiwater_vm < 32 * 1024, "VM peak");
	check(avg > 32 && avg < 52, "RSS integral units across exec");
	check(vavg > 32 && vavg < 52, "VM integral units across exec");
	puts("PASS: taskstats RSS/VM peaks and memory integral across geometry-changing exec");
	return 0;
}
