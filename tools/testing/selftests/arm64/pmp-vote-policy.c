// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <assert.h>
#include <stdio.h>
#include "../../../../drivers/pmdomain/apple/pmp-vote-policy.h"
int main(void)
{
	struct apple_pmp_vote_policy p = {0};
	struct apple_pmp_vote_cpu_time t = {0};
	unsigned int sum = 0, i;

	assert(apple_pmp_vote_load(&t, 1000, 2000) == 0);
	assert(apple_pmp_vote_load(&t, 1000, 22000) == 100);
	assert(apple_pmp_vote_load(&t, 21000, 42000) == 0);
	assert(apple_pmp_vote_load(&t, 31000, 62000) == 50);
	assert(apple_pmp_vote_load(&t, 71000, 82000) == 0);
	/* Counter reset, offline/re-online and invalid idle accounting. */
	assert(apple_pmp_vote_load(&t, 0, 100) == 0);
	t.valid = 0;
	assert(apple_pmp_vote_load(&t, 0, 20100) == 0);
	assert(apple_pmp_vote_load(&t, ~0ULL, 40100) == 0);
	assert(!t.valid);
	assert(apple_pmp_vote_load(&t, 0, 60100) == 0);
	assert(apple_pmp_vote_load(&t, 0, 80100) == 100);
	assert(apple_pmp_vote_e_input(99, 100) == 0);
	assert(apple_pmp_vote_e_input(100, 100) == APPLE_PMP_VOTE_E_CPU);
	assert(apple_pmp_vote_e_input(299, 300) == 0);
	assert(apple_pmp_vote_e_input(300, 300) == APPLE_PMP_VOTE_E_CPU);
	/* Equivalent distributed demand and only one shared output/hold. */
	for (i = 0; i < 6; i++)
		sum += 50;
	assert(apple_pmp_vote_e_input(sum, 300) == APPLE_PMP_VOTE_E_CPU);
	assert(apple_pmp_vote_decide(&p, 10, APPLE_PMP_VOTE_E_CPU,
				    80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 89, 0, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 90, 0, 80, 0x35) == 0);
	p = (struct apple_pmp_vote_policy){0};

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
	/* Preserve a complete GPU pulse without adding a sampling period. */
	apple_pmp_vote_note_activity(&p, 12000);
	assert(apple_pmp_vote_decide(&p, 12020, 0, 80, 0x35) == 0x35);
	apple_pmp_vote_note_activity(&p, 11999);
	assert(apple_pmp_vote_decide(&p, 12080, 0, 80, 0x35) == 0);
	/* An in-flight GPU retains the floor even without CPU demand. */
	assert(apple_pmp_vote_decide(&p, 13000, APPLE_PMP_VOTE_GPU, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 14000, APPLE_PMP_VOTE_GPU, 80, 0x35) == 0x35);
	apple_pmp_vote_note_activity(&p, 14010);
	assert(apple_pmp_vote_decide(&p, 14089, 0, 80, 0x35) == 0x35);
	assert(apple_pmp_vote_decide(&p, 14090, 0, 80, 0x35) == 0);
	assert(apple_pmp_vote_valid(0) && apple_pmp_vote_valid(0x15));
	assert(apple_pmp_vote_valid(0x25) && apple_pmp_vote_valid(0x35));
	assert(!apple_pmp_vote_valid(0x36) && !apple_pmp_vote_valid(0x40));
	assert(!apple_pmp_vote_valid(0x100));
	/* Status must never be written back, including on teardown restore. */
	assert(apple_pmp_cpm_control(0x056000fc, 1) == 0xfd);
	assert(apple_pmp_cpm_control(0x078000fd, 0) == 0xfc);
	assert(apple_pmp_cpm_control(0x012000fc, 0) == 0xfc);
	assert(apple_pmp_cpm_control(0x034000fd, 1) == 0xfd);
	assert(apple_pmp_cpm_control(0xf56abcfd, 0) == 0xf00abcfc);
	assert(apple_pmp_cpm_control(0xf78abcfc, 1) == 0xf00abcfd);
	assert(apple_pmp_cpm_control(0x12345678056000fcULL, 1) ==
	       0x12345678000000fdULL);
	puts("E aggregate: equivalence, threshold boundaries, resets and hold PASS");
	puts("CPM: bit0 update, status suppression and control preservation PASS");
	puts("policy: threshold, shared hysteresis, expiry and vote range PASS");
}
