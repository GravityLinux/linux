/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef __APPLE_DCP_AFK_FRAGMENT_H__
#define __APPLE_DCP_AFK_FRAGMENT_H__

#include <linux/errno.h>
#include <linux/types.h>

/* Native 26.6 AV traffic: repeated 8-byte header, then up to 1028 bytes
 * of the logical message (timestamp, subheader, descriptor and payload).
 * The total length is repeated and the endpoint sequence advances on every
 * fragment, including 255 -> 0. Messages are not interleaved on an endpoint.
 */
#define AFK_COMPACT_FRAGMENT_SIZE 1028U
#define AFK_COMPACT_MESSAGE_MAX (1024U * 1024U)

struct afk_fragment_state {
	u32 total;
	u32 received;
	u16 intf_id;
	u8 next_seq;
};

/* Validate before allocating/copying. Returns 1 on completion, 0 while
 * collecting, or an error. The caller discards the whole message on error.
 */
static inline int afk_fragment_accept(struct afk_fragment_state *s,
				    u16 intf_id, u8 seq, u32 total, u32 size)
{
	if (total < 16 || total > AFK_COMPACT_MESSAGE_MAX || !size || size > total)
		return -EMSGSIZE;
	if (s->received && (s->total != total || s->intf_id != intf_id ||
			    s->next_seq != seq))
		return -EPROTO;
	if (size > total - s->received)
		return -EMSGSIZE;

	s->total = total;
	s->intf_id = intf_id;
	s->next_seq = seq + 1;
	s->received += size;
	return s->received == total;
}

#endif
