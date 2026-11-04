// SPDX-License-Identifier: GPL-2.0-only
/* Small nl80211 smoke-test client for an initramfs without libnl/iw.
 * Build: aarch64-linux-gnu-gcc -static -O2 -Wall -Wextra -o scan-test centauri_scan_test.c
 * Bring the interface up with ip, then: scan-test trigger wlan0
 * Later: scan-test dump wlan0. This uses ordinary nl80211, never driver MMIO.
 */
#include <errno.h>
#include <linux/genetlink.h>
#include <linux/nl80211.h>
#include <net/if.h>
#include <poll.h>
#include <termios.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

struct message {
	struct nlmsghdr header;
	struct genlmsghdr generic;
	unsigned char attributes[4096];
};

static unsigned int sequence;
static int family;
static uint32_t mlme_group;

static bool attr_valid(const struct nlattr *attr, size_t left)
{
	return left >= NLA_HDRLEN && attr->nla_len >= NLA_HDRLEN &&
	       attr->nla_len <= left && (size_t)NLA_ALIGN(attr->nla_len) <= left;
}

static const void *attr_data(const struct nlattr *attr)
{
	return (const unsigned char *)attr + NLA_HDRLEN;
}

static const struct nlattr *attr_find(const void *data, size_t bytes, int type)
{
	const struct nlattr *attr = data;

	while (attr_valid(attr, bytes)) {
		if ((attr->nla_type & NLA_TYPE_MASK) == type)
			return attr;
		bytes -= NLA_ALIGN(attr->nla_len);
		attr = (const void *)((const unsigned char *)attr + NLA_ALIGN(attr->nla_len));
	}
	return NULL;
}

static void init_message(struct message *message, int type, int command, int flags)
{
	memset(message, 0, sizeof(*message));
	message->header.nlmsg_len = NLMSG_LENGTH(GENL_HDRLEN);
	message->header.nlmsg_type = type;
	message->header.nlmsg_flags = NLM_F_REQUEST | flags;
	message->header.nlmsg_seq = ++sequence;
	message->generic.cmd = command;
	message->generic.version = 1;
}

static void add_attr(struct message *message, int type, const void *data, size_t bytes)
{
	size_t length = NLA_HDRLEN + bytes;
	struct nlattr *attr;

	if (message->header.nlmsg_len + NLA_ALIGN(length) > sizeof(*message)) {
		fprintf(stderr, "request too large\n");
		exit(1);
	}
	attr = (void *)((unsigned char *)message + message->header.nlmsg_len);
	attr->nla_type = type;
	attr->nla_len = length;
	memcpy((unsigned char *)attr + NLA_HDRLEN, data, bytes);
	message->header.nlmsg_len += NLA_ALIGN(length);
}

static void print_bss(const struct genlmsghdr *generic, size_t bytes)
{
	const void *attributes = (const unsigned char *)generic + GENL_HDRLEN;
	const struct nlattr *outer, *attr;
	const void *data;
	size_t length;
	uint32_t frequency;
	int32_t signal;

	outer = attr_find(attributes, bytes - GENL_HDRLEN, NL80211_ATTR_BSS);
	if (!outer)
		return;
	data = attr_data(outer);
	length = outer->nla_len - NLA_HDRLEN;
	attr = attr_find(data, length, NL80211_BSS_BSSID);
	if (!attr || attr->nla_len != NLA_HDRLEN + 6)
		return;
	printf("BSS ");
	for (int i = 0; i < 6; i++)
		printf("%s%02x", i ? ":" : "", ((const unsigned char *)attr_data(attr))[i]);
	attr = attr_find(data, length, NL80211_BSS_FREQUENCY);
	if (attr && attr->nla_len == NLA_HDRLEN + sizeof(frequency)) {
		memcpy(&frequency, attr_data(attr), sizeof(frequency));
		printf(" frequency=%u", frequency);
	}
	attr = attr_find(data, length, NL80211_BSS_SIGNAL_MBM);
	if (attr && attr->nla_len == NLA_HDRLEN + sizeof(signal)) {
		memcpy(&signal, attr_data(attr), sizeof(signal));
		printf(" signal_mbm=%d", signal);
	}
	attr = attr_find(data, length, NL80211_BSS_STATUS);
	if (attr && attr->nla_len == NLA_HDRLEN + sizeof(uint32_t)) {
		uint32_t status;

		memcpy(&status, attr_data(attr), sizeof(status));
		printf(" status=%u", status);
	}
	attr = attr_find(data, length, NL80211_BSS_INFORMATION_ELEMENTS);
	if (attr) {
		const unsigned char *ie = attr_data(attr);
		size_t left = attr->nla_len - NLA_HDRLEN;

		while (left >= 2 && (size_t)ie[1] + 2 <= left) {
			if (ie[0] == 0 && ie[1] <= 32) {
				printf(" ssid_hex=");
				for (int i = 0; i < ie[1]; i++)
					printf("%02x", ie[2 + i]);
				break;
			}
			left -= 2 + ie[1];
			ie += 2 + ie[1];
		}
	}
	putchar('\n');
}

static int resolve_family(const struct genlmsghdr *generic, size_t bytes)
{
	const void *attributes = (const unsigned char *)generic + GENL_HDRLEN;
	const struct nlattr *attr, *groups, *group;
	size_t left;
	uint16_t id;

	attr = attr_find(attributes, bytes - GENL_HDRLEN, CTRL_ATTR_FAMILY_ID);
	if (!attr || attr->nla_len != NLA_HDRLEN + sizeof(id))
		return -1;
	memcpy(&id, attr_data(attr), sizeof(id));
	family = id;
	groups = attr_find(attributes, bytes - GENL_HDRLEN, CTRL_ATTR_MCAST_GROUPS);
	if (!groups)
		return 0;
	group = attr_data(groups);
	left = groups->nla_len - NLA_HDRLEN;
	while (attr_valid(group, left)) {
		const void *data = attr_data(group);
		size_t length = group->nla_len - NLA_HDRLEN;
		const struct nlattr *name = attr_find(data, length, CTRL_ATTR_MCAST_GRP_NAME);
		const struct nlattr *value = attr_find(data, length, CTRL_ATTR_MCAST_GRP_ID);

		if (name && name->nla_len == NLA_HDRLEN + 5 &&
		    !memcmp(attr_data(name), "mlme", 5) && value &&
		    value->nla_len == NLA_HDRLEN + sizeof(mlme_group))
			memcpy(&mlme_group, attr_data(value), sizeof(mlme_group));
		left -= NLA_ALIGN(group->nla_len);
		group = (const void *)((const unsigned char *)group + NLA_ALIGN(group->nla_len));
	}
	return 0;
}

static int exchange(int fd, struct message *message, bool dump, bool resolve)
{
	struct sockaddr_nl kernel = { .nl_family = AF_NETLINK };
	unsigned char response[32768];

	if (sendto(fd, message, message->header.nlmsg_len, 0,
		   (struct sockaddr *)&kernel, sizeof(kernel)) < 0) {
		perror("netlink send");
		return -1;
	}
	for (;;) {
		ssize_t received = recv(fd, response, sizeof(response), 0);
		struct nlmsghdr *header = (void *)response;

		if (received < 0) {
			perror("netlink receive");
			return -1;
		}
		for (; NLMSG_OK(header, received); header = NLMSG_NEXT(header, received)) {
			if (header->nlmsg_seq != sequence)
				continue;
			if (header->nlmsg_type == NLMSG_ERROR) {
				struct nlmsgerr *error;

				if (header->nlmsg_len < NLMSG_LENGTH(sizeof(*error)))
					return -1;
				error = NLMSG_DATA(header);
				if (error->error) {
					fprintf(stderr, "nl80211: %s (%d)\n",
						strerror(-error->error), error->error);
					return -1;
				}
				if (!dump)
					return 0;
			} else if (header->nlmsg_type == NLMSG_DONE) {
				if (header->nlmsg_len >= NLMSG_LENGTH(sizeof(int))) {
					int error;

					memcpy(&error, NLMSG_DATA(header), sizeof(error));
					if (error)
						return -1;
				}
				return 0;
			} else if (header->nlmsg_len >= NLMSG_LENGTH(GENL_HDRLEN)) {
				struct genlmsghdr *generic = NLMSG_DATA(header);
				size_t bytes = header->nlmsg_len - NLMSG_HDRLEN;

				if (resolve)
					return resolve_family(generic, bytes);
				print_bss(generic, bytes);
			}
		}
	}
}


static int hex_bytes(const char *text, unsigned char *output, size_t capacity)
{
	size_t length = strlen(text);

	if (length % 2 || length / 2 > capacity)
		return -1;
	for (size_t i = 0; i < length / 2; i++) {
		unsigned int value = 0;

		for (int j = 0; j < 2; j++) {
			unsigned char c = text[i * 2 + j];
			int digit = c >= '0' && c <= '9' ? c - '0' :
				c >= 'a' && c <= 'f' ? c - 'a' + 10 :
				c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;

			if (digit < 0)
				return -1;
			value = value * 16 + digit;
		}
		output[i] = value;
	}
	return length / 2;
}

static int read_pmk(unsigned char output[32])
{
	struct termios original, quiet;
	struct pollfd input = { .fd = STDIN_FILENO, .events = POLLIN };
	char line[66] = {};
	bool terminal = isatty(STDIN_FILENO);
	int result = -1;

	if (terminal) {
		if (tcgetattr(STDIN_FILENO, &original))
			return -1;
		quiet = original;
		quiet.c_lflag &= ~(ECHO | ECHONL);
		if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &quiet))
			return -1;
	}
	puts("PMK_INPUT_READY");
	fflush(stdout);
	if (poll(&input, 1, 30000) > 0 && fgets(line, sizeof(line), stdin)) {
		size_t bytes = strlen(line);

		if (bytes == 65 && line[64] == '\n') {
			line[64] = 0;
			result = hex_bytes(line, output, 32) == 32 ? 0 : -1;
		}
	}
	explicit_bzero(line, sizeof(line));
	if (terminal && tcsetattr(STDIN_FILENO, TCSAFLUSH, &original))
		result = -1;
	return result;
}

static int monitor_events(int fd, unsigned int ifindex)
{
	unsigned char response[32768];

	if (!mlme_group || setsockopt(fd, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP,
				      &mlme_group, sizeof(mlme_group))) {
		perror("nl80211 mlme subscription");
		return 1;
	}
	puts("NL80211_EVENT_MONITOR_READY");
	fflush(stdout);
	for (;;) {
		ssize_t received = recv(fd, response, sizeof(response), 0);
		struct nlmsghdr *header = (void *)response;

		if (received < 0) {
			if (errno == EAGAIN || errno == EINTR)
				continue;
			perror("event receive");
			return 1;
		}
		for (; NLMSG_OK(header, received); header = NLMSG_NEXT(header, received)) {
			if (header->nlmsg_type != family ||
			    header->nlmsg_len < NLMSG_LENGTH(GENL_HDRLEN))
				continue;
			struct genlmsghdr *generic = NLMSG_DATA(header);
			const void *data = (const unsigned char *)generic + GENL_HDRLEN;
			size_t bytes = header->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
			const struct nlattr *attr = attr_find(data, bytes, NL80211_ATTR_IFINDEX);
			uint32_t index;

			if (!attr || attr->nla_len != NLA_HDRLEN + sizeof(index))
				continue;
			memcpy(&index, attr_data(attr), sizeof(index));
			if (index != ifindex)
				continue;
			printf("NL80211_EVENT command=%u", generic->cmd);
			attr = attr_find(data, bytes, NL80211_ATTR_STATUS_CODE);
			if (attr && attr->nla_len == NLA_HDRLEN + sizeof(uint16_t)) {
				uint16_t status;

				memcpy(&status, attr_data(attr), sizeof(status));
				printf(" status=%u", status);
			}
			if (generic->cmd == NL80211_CMD_PORT_AUTHORIZED)
				printf(" PORT_AUTHORIZED");
			putchar('\n');
			fflush(stdout);
		}
	}
}

int main(int argc, char **argv)
{
	struct sockaddr_nl local = { .nl_family = AF_NETLINK };
	struct timeval timeout = { .tv_sec = 5 };
	struct message message;
	unsigned int ifindex = 0;
	bool dump, country, connect, disconnect, events;
	int fd, result;

	if (argc < 3) {
usage:
		fprintf(stderr, "usage: %s trigger|dump|disconnect|events interface\n"
			"       %s country CC\n"
			"       %s connect interface ssid_hex bssid_hex frequency_mhz\n",
			argv[0], argv[0], argv[0]);
		return 2;
	}
	country = !strcmp(argv[1], "country");
	connect = !strcmp(argv[1], "connect");
	disconnect = !strcmp(argv[1], "disconnect");
	dump = !strcmp(argv[1], "dump");
	events = !strcmp(argv[1], "events");
	if ((connect && argc != 6) || (!connect && argc != 3) ||
	    (!country && !connect && !disconnect && !dump && !events && strcmp(argv[1], "trigger")))
		goto usage;
	if (!country) {
		ifindex = if_nametoindex(argv[2]);
		if (!ifindex) {
			perror("interface");
			return 1;
		}
	}
	fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
	if (fd < 0 || bind(fd, (struct sockaddr *)&local, sizeof(local)) ||
	    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout))) {
		perror("netlink socket");
		return 1;
	}
	init_message(&message, GENL_ID_CTRL, CTRL_CMD_GETFAMILY, 0);
	add_attr(&message, CTRL_ATTR_FAMILY_NAME, "nl80211", sizeof("nl80211"));
	if (exchange(fd, &message, false, true))
		return 1;
	if (events)
		return monitor_events(fd, ifindex);
	init_message(&message, family,
		country ? NL80211_CMD_REQ_SET_REG : connect ? NL80211_CMD_CONNECT :
		disconnect ? NL80211_CMD_DISCONNECT : dump ? NL80211_CMD_GET_SCAN : NL80211_CMD_TRIGGER_SCAN,
		dump ? NLM_F_DUMP : NLM_F_ACK);
	if (!country)
		add_attr(&message, NL80211_ATTR_IFINDEX, &ifindex, sizeof(ifindex));
	if (country) {
		if (strlen(argv[2]) != 2 || argv[2][0] < 'A' || argv[2][0] > 'Z' ||
		    argv[2][1] < 'A' || argv[2][1] > 'Z')
			goto usage;
		add_attr(&message, NL80211_ATTR_REG_ALPHA2, argv[2], 3);
	}
	if (disconnect) {
		uint16_t reason = 3; /* IEEE802.11 deauthentication leaving. */

		add_attr(&message, NL80211_ATTR_REASON_CODE, &reason, sizeof(reason));
	}
	if (connect) {
		unsigned char ssid[32], bssid[6], pmk[32];
		static const unsigned char rsn[] = {
			48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4,
			1, 0, 0, 15, 172, 2, 0, 0
		};
		uint32_t version = NL80211_WPA_VERSION_2, cipher = 0x000fac04;
		uint32_t akm = 0x000fac02, auth = NL80211_AUTHTYPE_OPEN_SYSTEM;
		char *end;
		unsigned long parsed = strtoul(argv[5], &end, 10);
		uint32_t frequency = parsed;
		int ssid_bytes = hex_bytes(argv[3], ssid, sizeof(ssid));

		if (ssid_bytes <= 0 || hex_bytes(argv[4], bssid, sizeof(bssid)) != 6 ||
		    *end || parsed > UINT32_MAX || frequency < 2412 || frequency > 2462 ||
		    (frequency - 2412) % 5)
			goto usage;
		add_attr(&message, NL80211_ATTR_SSID, ssid, ssid_bytes);
		add_attr(&message, NL80211_ATTR_MAC, bssid, sizeof(bssid));
		add_attr(&message, NL80211_ATTR_WIPHY_FREQ, &frequency, sizeof(frequency));
		add_attr(&message, NL80211_ATTR_WPA_VERSIONS, &version, sizeof(version));
		add_attr(&message, NL80211_ATTR_CIPHER_SUITE_GROUP, &cipher, sizeof(cipher));
		add_attr(&message, NL80211_ATTR_CIPHER_SUITES_PAIRWISE, &cipher, sizeof(cipher));
		add_attr(&message, NL80211_ATTR_AKM_SUITES, &akm, sizeof(akm));
		add_attr(&message, NL80211_ATTR_AUTH_TYPE, &auth, sizeof(auth));
		add_attr(&message, NL80211_ATTR_PRIVACY, "", 0);
		add_attr(&message, NL80211_ATTR_IE, rsn, sizeof(rsn));
		if (read_pmk(pmk)) {
			explicit_bzero(pmk, sizeof(pmk));
			fprintf(stderr, "PMK input missing or invalid\n");
			return 1;
		}
		add_attr(&message, NL80211_ATTR_PMK, pmk, sizeof(pmk));
		explicit_bzero(pmk, sizeof(pmk));
	}
	result = exchange(fd, &message, dump, false);
	explicit_bzero(&message, sizeof(message));
	if (!result && !dump)
		printf("%s_REQUEST_ACCEPTED\n", country ? "COUNTRY" : connect ? "CONNECT" :
		       disconnect ? "DISCONNECT" : "SCAN");
	close(fd);
	return result ? 1 : 0;
}
