// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
#include "afk-fragment.h"

static void boundary_tests(void)
{
	const u32 sizes[] = {
		16, 1028, 1029, 3072, 16384, 262280, AFK_COMPACT_MESSAGE_MAX
	};
	for (size_t i = 0; i < sizeof(sizes) / sizeof(*sizes); i++) {
		struct afk_fragment_state s = { 0 };
		u8 seq = 254;
		while (s.received < sizes[i]) {
			u32 n = sizes[i] - s.received;
			if (n > AFK_COMPACT_FRAGMENT_SIZE)
				n = AFK_COMPACT_FRAGMENT_SIZE;
			u32 next = s.received + n;
			assert(afk_fragment_accept(&s, 5, seq++, sizes[i], n) ==
			       (next == sizes[i]));
		}
	}
	struct afk_fragment_state s = { 0 }, started, unchanged;
	assert(afk_fragment_accept(&s, 5, 255, 2048, 1028) == 0);
	started = s;
	assert(afk_fragment_accept(&s, 5, 255, 2048, 1020) ==
	       -EPROTO); // duplicate
	assert(!memcmp(&s, &started, sizeof(s)));
	assert(afk_fragment_accept(&s, 5, 1, 2048, 1020) == -EPROTO); // missing
	assert(afk_fragment_accept(&s, 7, 0, 2048, 1020) ==
	       -EPROTO); // interleaved
	assert(afk_fragment_accept(&s, 5, 0, 2049, 1020) ==
	       -EPROTO); // changed total
	assert(afk_fragment_accept(&s, 5, 0, 2048, 1021) ==
	       -EMSGSIZE); // overrun
	assert(afk_fragment_accept(&s, 5, 0, 2048, 1020) == 1); // wrap
	memset(&s, 0, sizeof(s));
	unchanged = s;
	assert(afk_fragment_accept(&s, 5, 0, AFK_COMPACT_MESSAGE_MAX + 1,
				   1028) == -EMSGSIZE);
	assert(afk_fragment_accept(&s, 5, 0, 15, 15) == -EMSGSIZE);
	assert(afk_fragment_accept(&s, 5, 0, 16, 0) == -EMSGSIZE);
	assert(afk_fragment_accept(&s, 5, 0, 16, 17) == -EMSGSIZE);
	assert(!memcmp(&s, &unchanged, sizeof(s)));
	puts("fragment bounds, ordering, truncation, sequence wrap and completion passed");
}

static u32 le32(const u8 *p)
{
	return (u32)p[0] | (u32)p[1] << 8 | (u32)p[2] << 16 | (u32)p[3] << 24;
}

/* Optional independent native trace: each line is direction + hex ring entry,
 * including the queue channel/type words before the compact fragment header.
 */
static void native_trace(const char *path)
{
	FILE *f = fopen(path, "r");
	assert(f);
	char *line = NULL;
	size_t cap = 0;
	struct afk_fragment_state states[2] = { { 0 }, { 0 } };
	u8 *messages[2] = { calloc(1, AFK_COMPACT_MESSAGE_MAX),
			    calloc(1, AFK_COMPACT_MESSAGE_MAX) };
	assert(messages[0] && messages[1]);
	unsigned completed = 0, large_replies = 0;
	while (getline(&line, &cap, f) >= 0) {
		int dir = line[0] == '<';
		size_t n = strcspn(line + 2, "\r\n") / 2;
		u8 *b = malloc(n);
		assert(b && n >= 16);
		for (size_t i = 0; i < n; i++) {
			unsigned v;
			assert(sscanf(line + 2 + i * 2, "%2x", &v) == 1);
			b[i] = v;
		}
		struct afk_fragment_state *s = &states[dir];
		u32 total = le32(b + 12), offset = s->received;
		if (!offset && total == 8 && n == 24) {
			free(b);
			completed++;
			continue;
		}
		int ret = afk_fragment_accept(s, b[10] | (u16)b[11] << 8, b[8],
					      total, n - 16);
		assert(ret >= 0);
		memcpy(messages[dir] + offset, b + 16, n - 16);
		if (ret == 1) {
			const u8 *m = messages[dir];
			if (dir && total >= 140 && le32(m + 36) == 0x69706378 &&
			    m[26] == 1 && le32(m + 28) == 16) {
				u32 used = le32(m + 120);
				assert(total >= 140 && used <= total - 136 &&
				       used > 3072);
				assert(le32(m + 136) == 0xd3);
				printf("native GetElements: %u wire bytes, %u object bytes, %u elements\n",
				       total, used, le32(m + 140) & 0xffffff);
				large_replies++;
			}
			completed++;
			memset(s, 0, sizeof(*s));
		}
		free(b);
	}
	assert(!states[0].received && !states[1].received && large_replies);
	printf("native capture passed: %u complete messages\n", completed);
	free(line);
	free(messages[0]);
	free(messages[1]);
	fclose(f);
}
int main(int argc, char **argv)
{
	boundary_tests();
	if (argc == 2)
		native_trace(argv[1]);
	return 0;
}
