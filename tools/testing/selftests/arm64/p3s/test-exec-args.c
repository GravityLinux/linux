/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Exec into a 4K process with arguments and environment spanning several
 * host pages, and check that every string arrives intact. Google's
 * exec_env_p3s (android-review 4367992) needs the selftests/mm P3S suite.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/personality.h>
#include <sys/wait.h>
#include <unistd.h>

#define ADDR_PAGE_SIZE_4KB 0x40000000UL
#define NR_VARS 64
#define VAR_LEN 1000

static char *make_var(int i)
{
	char *s = malloc(VAR_LEN + 16);
	int n;

	if (!s)
		exit(2);
	n = sprintf(s, "P3S_ARG_%02d=", i);
	memset(s + n, 'a' + i % 26, VAR_LEN);
	s[n + VAR_LEN] = '\0';
	return s;
}

static int child(int argc, char **argv)
{
	int i, bad = 0;

	if (getauxval(AT_PAGESZ) != 4096) {
		fprintf(stderr, "child pages=%lu, expected 4096\n", getauxval(AT_PAGESZ));
		return 1;
	}
	if (argc != NR_VARS + 2)
		bad++;
	for (i = 0; i < NR_VARS; i++) {
		char *want = make_var(i), *name = strchr(want, '=');
		char *got;

		*name = '\0';
		got = getenv(want);
		if (!got || strcmp(got, name + 1))
			bad++;
		*name = '=';
		if (argc > i + 2 && strcmp(argv[i + 2], want))
			bad++;
		free(want);
	}
	printf("%s: %d of %d strings damaged\n", bad ? "FAIL" : "PASS", bad,
	       2 * NR_VARS + 1);
	return !!bad;
}

int main(int argc, char **argv)
{
	char *args[NR_VARS + 3], *env[NR_VARS + 1];
	int i, old;

	if (argc > 1 && !strcmp(argv[1], "--child"))
		return child(argc, argv);

	args[0] = argv[0];
	args[1] = "--child";
	for (i = 0; i < NR_VARS; i++)
		args[i + 2] = env[i] = make_var(i);
	args[NR_VARS + 2] = NULL;
	env[NR_VARS] = NULL;

	old = personality(0xffffffffUL);
	if (old == -1 || personality((unsigned int)old | ADDR_PAGE_SIZE_4KB) == -1) {
		perror("personality: request 4K at next exec");
		return 1;
	}
	execve("/proc/self/exe", args, env);
	perror("execve");
	return 1;
}
