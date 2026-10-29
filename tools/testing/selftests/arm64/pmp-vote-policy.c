// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <assert.h>
#include <stdio.h>
#include "../../../../drivers/pmdomain/apple/pmp-vote-policy.h"
int main(void)
{
	struct apple_pmp_vote_policy p = {0};

	assert(apple_pmp_vote_decide(&p, 0, 0, 50, 80, 0x35) == 0);
	assert(apple_pmp_vote_decide(&p, 10, 49, 50, 80, 0x35) == 0);
	assert(apple_pmp_vote_decide(&p, 20, 50, 50, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 99, 0, 50, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 100, 0, 50, 80, 0x35) == 0);
	/* Activity on another sampled P-core extends the shared hold. */
	assert(apple_pmp_vote_decide(&p, 200, 100, 50, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 260, 70, 50, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 339, 0, 50, 80, 0x25) == 0x25);
	assert(apple_pmp_vote_decide(&p, 340, 0, 50, 80, 0x35) == 0);
	/* A deferrable sample may arrive much later; it must release the vote. */
	assert(apple_pmp_vote_decide(&p, 10000, 0, 50, 80, 0x35) == 0);
	assert(apple_pmp_vote_valid(0) && apple_pmp_vote_valid(0x15));
	assert(apple_pmp_vote_valid(0x25) && apple_pmp_vote_valid(0x35));
	assert(!apple_pmp_vote_valid(0x36) && !apple_pmp_vote_valid(0x40));
	assert(!apple_pmp_vote_valid(0x100));
	puts("policy: threshold, shared hysteresis, expiry and vote range PASS");
}
