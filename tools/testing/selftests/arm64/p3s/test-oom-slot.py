#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile the real OOM transition hook against a guarded synthetic PTE table.

Geometry/atomic/PTE decoding are stubbed; the production sibling scan is not.
An inconsistent VA must never choose the memory window being read. Test every
slot of a 512-entry table, including both guard-page boundaries, and compare
the hook's weight with an independent census of the window before and after.
"""
from pathlib import Path
import re
import resource
import subprocess
import sys
import tempfile

root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[5]
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
source = (root / 'mm/p3s.c').read_text()
m = re.search(r'^void p3s_account_pte\(', source, re.M)
assert m
start = source.index('{', m.end())
depth = 0
for end in range(start, len(source)):
    depth += (source[end] == '{') - (source[end] == '}')
    if not depth:
        hook = source[m.start():end + 1]
        break

code = r'''
#define _GNU_SOURCE
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>
typedef uint64_t pte_t;
#define PAGE_SIZE_KERNEL 16384UL
#define PAGE_SHIFT_4KB 12
#define P3S_SLICES_PER_PAGE 4
#define READ_ONCE(x) (x)
static unsigned long warnings;
#define VM_WARN_ON_ONCE(x) do { warnings += !!(x); } while (0)
struct mm_struct { long p3s_oom_pages; };
struct p3s_backing { unsigned long index; enum {P3S_NONE, P3S_RESIDENT, P3S_SWAP} kind; };
static struct p3s_backing p3s_pte_backing(pte_t pte)
{
    return (struct p3s_backing){ .index = pte >> 2, .kind = pte & 3 };
}
static bool p3s_same_backing(struct p3s_backing a, struct p3s_backing b)
{
    return a.kind == b.kind && (a.kind == P3S_NONE || a.index == b.index);
}
static void atomic_long_add(int n, long *p) { *p += n; }
'''
code += hook
code += r'''
static long census(pte_t *window)
{
    long n = 0;
    for (unsigned int i = 0; i < 4; ++i) {
        if (!(window[i] & 3)) continue;
        unsigned int j;
        for (j = 0; j < i; ++j) if (window[j] == window[i]) break;
        n += j == i;
    }
    return n;
}
int main(void)
{
    size_t page = sysconf(_SC_PAGESIZE);
    assert(page == 4096);
    char *guard = mmap(NULL, 3 * page, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(guard != MAP_FAILED);
    pte_t *table = (void *)(guard + page);
    assert(!mprotect(table, page, PROT_READ | PROT_WRITE));
    unsigned long count = 0;
    /* Present keys, a nonpresent key and empty entries exercise transitions. */
    const pte_t keys[] = {0, (10 << 2) | 1, (11 << 2) | 1, (10 << 2) | 2};
    for (unsigned int index = 0; index < 512; ++index)
    for (unsigned int va_slot = 0; va_slot < 4; ++va_slot)
    for (unsigned int pattern = 0; pattern < 4; ++pattern)
    for (unsigned int replacement = 0; replacement < 4; ++replacement) {
        pte_t *window = table + (index & ~3U);
        for (unsigned int j = 0; j < 4; ++j) window[j] = keys[(j + pattern) % 4];
        struct mm_struct mm = { .p3s_oom_pages = census(window) };
        pte_t old = table[index], new = keys[replacement];
        /* Deliberately disagree with the pointer for three of four VAs. */
        p3s_account_pte(&mm, (0x100UL + va_slot) << 12, table + index, old, new);
        table[index] = new;
        assert(mm.p3s_oom_pages == census(window));
        ++count;
    }
    assert(!munmap(guard, 3 * page));
    printf("PASS: %lu PTE transitions, mismatched VA safe at both table edges (%lu warnings)\n", count, warnings);
    return 0;
}
'''
with tempfile.TemporaryDirectory(prefix='p3s-slot-') as tmp:
    c = Path(tmp) / 'slot.c'
    out = Path(tmp) / 'slot'
    c.write_text(code)
    subprocess.run(['cc', '-std=gnu11', '-O2', '-Wall', '-Wextra', '-Werror',
                    str(c), '-o', str(out)], check=True)
    run = subprocess.run([str(out)], cwd=tmp)
    sys.exit(run.returncode if run.returncode >= 0 else 128 - run.returncode)
