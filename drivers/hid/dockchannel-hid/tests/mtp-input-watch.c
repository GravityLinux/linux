#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define MAX_INPUTS 32
struct input {
	char path[64], name[256];
	int keyboard, trackpad, changed, slot, tracking, x, y, pressure;
};
static const char *key_name(unsigned code)
{
	switch (code) {
	case KEY_ESC: return "ESC";
	case KEY_ENTER: return "ENTER";
	case KEY_SPACE: return "SPACE";
	case KEY_LEFTSHIFT: return "LEFTSHIFT";
	case KEY_RIGHTSHIFT: return "RIGHTSHIFT";
	case KEY_LEFTCTRL: return "LEFTCTRL";
	case KEY_LEFTALT: return "LEFTALT";
	case BTN_LEFT: return "BTN_LEFT";
	case BTN_RIGHT: return "BTN_RIGHT";
	case BTN_TOUCH: return "BTN_TOUCH";
	case BTN_TOOL_FINGER: return "BTN_TOOL_FINGER";
	default: return "KEY";
	}
}
static int bit(const unsigned char *bits, unsigned n)
{
	return (bits[n / 8] >> (n % 8)) & 1;
}
int main(void)
{
	struct pollfd fds[MAX_INPUTS];
	struct input inputs[MAX_INPUTS] = {0};
	int count = 0, keyboards = 0, trackpads = 0;
	setvbuf(stdout, NULL, _IOLBF, 0);
	for (int index = 0; index < 64 && count < MAX_INPUTS; index++) {
		char path[64], name[256] = {0};
		unsigned char keys[(KEY_MAX + 8) / 8] = {0};
		unsigned char abs[(ABS_MAX + 8) / 8] = {0};
		snprintf(path, sizeof(path), "/dev/input/event%d", index);
		int fd = open(path, O_RDONLY | O_NONBLOCK);
		if (fd < 0) continue;
		if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) < 0 ||
		    !strstr(name, "MTP")) { close(fd); continue; }
		if (ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(keys)), keys) < 0 ||
		    ioctl(fd, EVIOCGBIT(EV_ABS, sizeof(abs)), abs) < 0) {
			perror("EVIOCGBIT"); close(fd); continue;
		}
		struct input *in = &inputs[count];
		strcpy(in->path, path); strcpy(in->name, name);
		in->keyboard = bit(keys, KEY_A);
		in->trackpad = bit(abs, ABS_MT_POSITION_X) || bit(abs, ABS_X);
		in->tracking = -1;
		keyboards += in->keyboard; trackpads += in->trackpad;
		fds[count] = (struct pollfd){ .fd = fd, .events = POLLIN };
		printf("MTP_INPUT_DEVICE path=%s name=\"%s\" keyboard=%d trackpad=%d\n",
		       path, name, in->keyboard, in->trackpad);
		count++;
	}
	printf("MTP_INPUT_READY keyboards=%d trackpads=%d devices=%d\n", keyboards, trackpads, count);
	if (!count) return 2;
	for (;;) {
		if (poll(fds, count, -1) < 0) { if (errno == EINTR) continue; perror("poll"); return 1; }
		for (int i = 0; i < count; i++) {
			if (fds[i].revents & (POLLERR | POLLHUP | POLLNVAL)) {
				printf("MTP_INPUT_DISCONNECTED path=%s\n", inputs[i].path); return 1;
			}
			if (!(fds[i].revents & POLLIN)) continue;
			struct input_event events[64];
			ssize_t length = read(fds[i].fd, events, sizeof(events));
			if (length < 0) { if (errno == EAGAIN) continue; perror("read"); return 1; }
			if (!length || length % sizeof(events[0])) return 1;
			struct input *in = &inputs[i];
			for (size_t j = 0; j < (size_t)length / sizeof(events[0]); j++) {
				struct input_event *ev = &events[j];
				if (ev->type == EV_KEY)
					printf("MTP_KEY path=%s code=%u name=%s value=%d\n", in->path, ev->code, key_name(ev->code), ev->value);
				if (ev->type == EV_ABS) {
					in->changed = 1;
					switch (ev->code) {
					case ABS_MT_SLOT: in->slot = ev->value; break;
					case ABS_MT_TRACKING_ID: in->tracking = ev->value; break;
					case ABS_X: case ABS_MT_POSITION_X: in->x = ev->value; break;
					case ABS_Y: case ABS_MT_POSITION_Y: in->y = ev->value; break;
					case ABS_PRESSURE: case ABS_MT_PRESSURE: in->pressure = ev->value; break;
					}
				}
				if (ev->type == EV_REL)
					printf("MTP_REL path=%s code=%u value=%d\n", in->path, ev->code, ev->value);
				if (ev->type == EV_SYN && ev->code == SYN_DROPPED)
					printf("MTP_INPUT_DROPPED path=%s\n", in->path);
				if (ev->type == EV_SYN && ev->code == SYN_REPORT && in->changed) {
					printf("MTP_TOUCH path=%s slot=%d tracking=%d x=%d y=%d pressure=%d\n",
					       in->path, in->slot, in->tracking, in->x, in->y, in->pressure);
					in->changed = 0;
				}
			}
		}
	}
}
