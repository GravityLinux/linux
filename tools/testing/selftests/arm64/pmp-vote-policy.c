// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <assert.h>
#include <stdio.h>
#include "../../../../drivers/pmdomain/apple/pmp-vote-policy.h"
int main(void)
{
	struct apple_pmp_vote_policy p = {0};

	assert(apple_pmp_vote_cpu_input(49, 50) == 0);
	assert(apple_pmp_vote_cpu_input(50, 50) == APPLE_PMP_VOTE_CPU);
	assert(apple_pmp_vote_cpu_input(100, 50) == APPLE_PMP_VOTE_CPU);

	assert(apple_pmp_vote_decide(&p, 0, 0, 80, 0x35) == 0);
	assert(apple_pmp_vote_decide(&p, 10, 0, 80, 0x35) == 0);
	assert(apple_pmp_vote_decide(&p, 20, APPLE_PMP_VOTE_CPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 99, 0, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 100, 0, 80, 0x35) == 0);
	/* Activity on another sampled P-core extends the shared hold. */
	assert(apple_pmp_vote_decide(&p, 200, APPLE_PMP_VOTE_CPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 260, APPLE_PMP_VOTE_CPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 339, 0, 80, 0x25) == 0x25);
	assert(apple_pmp_vote_decide(&p, 340, 0, 80, 0x35) == 0);
	/* A deferrable sample may arrive much later; it must release the vote. */
	assert(apple_pmp_vote_decide(&p, 10000, 0, 80, 0x35) == 0);
	/* A GPU producer can feed the same hold/output without a CPU load. */
	assert(apple_pmp_vote_decide(&p, 11000, APPLE_PMP_VOTE_GPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 11020,
		APPLE_PMP_VOTE_CPU | APPLE_PMP_VOTE_GPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 11099, 0, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 11100, 0, 80, 0x35) == 0);
	assert(apple_pmp_vote_valid(0) && apple_pmp_vote_valid(0x15));
	assert(apple_pmp_vote_valid(0x25) && apple_pmp_vote_valid(0x35));
	assert(!apple_pmp_vote_valid(0x36) && !apple_pmp_vote_valid(0x40));
	assert(!apple_pmp_vote_valid(0x100));
	puts("policy: threshold, shared hysteresis, expiry and vote range PASS");
}
