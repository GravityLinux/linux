#!/bin/sh
# SPDX-License-Identifier: GPL-2.0
#
# Ensure <linux/p3s_user_pages.h> is the last header included in C files.

set -e

if [ $# -lt 1 ]; then
	echo "Usage: $0 <source-file>" >&2
	exit 1
fi

file="$1"

# Fast exit if the file does not include p3s_user_pages.h
grep -q "p3s_user_pages\.h" "$file" || exit 0

case "$file" in
*.h)
	printf "\n\033[31m[P3S ERROR]\033[0m %s: " "$file" >&2
	printf "'#include <linux/p3s_user_pages.h>' must not be included in header files\n\n" >&2
	exit 1
	;;
esac

awk '
/^#include/ && !/\.c[">]/ && !/syscall_table/ && !/regset\.h/ {
	last = FNR
}
/p3s_user_pages\.h/ {
	p3s = FNR
}
END {
	if (p3s && p3s < last) {
		print "\n\033[31m[P3S ERROR]\033[0m " FILENAME ": " \
		      "\x27#include <linux/p3s_user_pages.h>\x27 " \
		      "must be the last include (line " \
		      p3s " < " last ")\n"
		exit 1
	}
}
' "$file" >&2
