# SPDX-License-Identifier: GPL-2.0-only
CFLAGS += -O2 -g -Wall -Wextra -Werror
LDLIBS += -static

# These need an explicit personality launcher; do not run them as native tests.
TEST_GEN_PROGS_EXTENDED := exec-4k test-4k test-futex-slice test-exec-slice test-accounting test-dma-mmap test-writeback test-perf-mmap test-usage test-taskstats test-oom-backing test-commit-limit test-commit-guess test-shm-lock test-reclaim

TEST_FILES += test-memcg-first-victim.sh

include ../../lib.mk

$(OUTPUT)/exec-4k $(OUTPUT)/test-futex-slice $(OUTPUT)/test-exec-slice $(OUTPUT)/test-accounting $(OUTPUT)/test-dma-mmap $(OUTPUT)/test-writeback $(OUTPUT)/test-perf-mmap $(OUTPUT)/test-usage $(OUTPUT)/test-taskstats $(OUTPUT)/test-oom-backing $(OUTPUT)/test-commit-limit $(OUTPUT)/test-commit-guess $(OUTPUT)/test-shm-lock $(OUTPUT)/test-reclaim: LDFLAGS += -Wl,-z,max-page-size=65536 -Wl,-z,common-page-size=65536
$(OUTPUT)/test-4k: LDFLAGS += -Wl,-z,max-page-size=4096 -Wl,-z,common-page-size=4096
