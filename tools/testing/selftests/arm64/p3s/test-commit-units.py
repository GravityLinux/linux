#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise production overcommit admission without reserving physical RAM."""
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[5]
def function(path, name):
    source = (root / path).read_text()
    match = re.search(r"^(?:static[^\n]*|int)\b[^\n]*\b" + name + r"\(", source, re.M)
    assert match, name
    start = source.index("{", match.end())
    depth = 0
    for end in range(start, len(source)):
        depth += (source[end] == "{") - (source[end] == "}")
        if not depth:
            return source[match.start():end + 1]
    raise AssertionError(name)

code = r"""
#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#define PAGE_SHIFT_KERNEL 14
#define ENOMEM 12
#define OVERCOMMIT_GUESS 0
#define OVERCOMMIT_ALWAYS 1
#define OVERCOMMIT_NEVER 2
#define DIV_ROUND_UP(n,d) (((n) + (d) - 1) / (d))
#define min_t(t,a,b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define pr_warn_ratelimited(...) ((void)bytes_failed)
struct mm_struct { unsigned long total_vm; bool small; };
static unsigned int mm_pte_shift(const struct mm_struct *mm) { return mm->small ? 12 : 14; }
static long vm_committed_as;
static unsigned long total_swap_pages, sysctl_admin_reserve_kbytes, sysctl_user_reserve_kbytes;
static int sysctl_overcommit_memory;
static unsigned long totalram_pages(void) { return (16UL << 30) >> 14; }
static unsigned long vm_commit_limit(void) { return (1UL << 30) >> 14; }
static void vm_acct_memory(long n) { vm_committed_as += n; }
static void vm_unacct_memory(long n) { vm_committed_as -= n; }
static long percpu_counter_read_positive(long *n) { return *n; }
"""
for path, names in (("include/linux/p3s/mm.h", ("mm_slices_per_page", "mm_pages_to_native")),
                    ("mm/util.c", ("vm_enough_memory", "__vm_enough_memory", "__vm_enough_memory_private"))):
    for name in names:
        code += function(path, name) + "\n"
code += r"""
int main(void)
{
    struct mm_struct native = {0}, small = {.small = true};
    const long ram = totalram_pages();
    for (int geometry = 0; geometry < 2; ++geometry) {
        struct mm_struct *mm = geometry ? &small : &native;
        long slices = geometry ? 4 : 1;
        long half_ram = ram * slices / 2;
        sysctl_overcommit_memory = OVERCOMMIT_GUESS;
        assert(!__vm_enough_memory_private(mm, half_ram, 1));
        assert(vm_committed_as == half_ram);
        /* The promise and later uncharge cannot depend on policy changes. */
        sysctl_overcommit_memory = OVERCOMMIT_NEVER;
        vm_unacct_memory(half_ram);
        assert(!vm_committed_as);
        sysctl_overcommit_memory = OVERCOMMIT_GUESS;
        assert(__vm_enough_memory_private(mm, ram * slices + 1, 1) == -ENOMEM);
        assert(!vm_committed_as);
        /* Shared object counts are native even for a 4K caller. */
        assert(!__vm_enough_memory(mm, ram / 2, 1));
        vm_unacct_memory(ram / 2);
        assert(__vm_enough_memory(mm, ram + 1, 1) == -ENOMEM);
        assert(!vm_committed_as);
        sysctl_overcommit_memory = OVERCOMMIT_NEVER;
        long pages = (400UL << 20) >> mm_pte_shift(mm);
        int rc = __vm_enough_memory_private(mm, pages, 1);
        assert(rc == (geometry ? -ENOMEM : 0));
        if (!rc) vm_unacct_memory(pages);
        assert(!vm_committed_as);
    }
    puts("PASS: dense heuristic, native shared bound, strict promises, balanced policy switch");
    return 0;
}
"""
with tempfile.TemporaryDirectory(prefix="p3s-commit-") as tmp:
    source = Path(tmp) / "commit.c"
    binary = Path(tmp) / "commit"
    source.write_text(code)
    subprocess.run(["cc", "-std=gnu11", "-O2", "-Wall", "-Wextra", "-Werror", "-Wno-sign-compare",
                    str(source), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
