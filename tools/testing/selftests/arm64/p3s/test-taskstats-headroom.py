#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run production taskstats update/export arithmetic with synthetic CPU time.

The actual functions are compiled verbatim. Only counters, task/MM plumbing,
and arithmetic primitives are stubbed. A 150-CPU-day 16GiB lifetime is small
enough for the original native-page accumulator but not KiB-ns/1024.
"""
from pathlib import Path
import re
import subprocess
import sys
import tempfile

root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[5]

def function(path, name):
    source = (root / path).read_text()
    m = re.search(r'^(?:static[^\n]*|void)\b[^\n]*\b' + name + r'\(', source, re.M)
    assert m, name
    start = source.index('{', m.end())
    depth = 0
    for end in range(start, len(source)):
        depth += (source[end] == '{') - (source[end] == '}')
        if not depth:
            return source[m.start():end + 1]
    raise AssertionError(name)

code = r'''
#include <assert.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
typedef uint64_t u64;
typedef uint32_t u32;
#define PAGE_SHIFT_KERNEL 14
#define PF_KTHREAD 1
#define TICK_NSEC 4000000ULL
#define KB_MASK (~1023ULL)
#define READ_ONCE(x) (x)
#define unlikely(x) (x)
#define do_div(n,d) ((n) /= (d))
struct mm_struct { unsigned long rss, total_vm; unsigned int shift; };
struct io_accounting { u64 rchar, wchar, syscr, syscw; };
struct task_struct { struct mm_struct *mm; unsigned long flags; u64 acct_timexpd, acct_rss_mem1, acct_vm_mem1; struct io_accounting ioac; };
struct taskstats { u64 coremem, virtmem, hiwater_rss, hiwater_vm, read_char, write_char, read_syscalls, write_syscalls, read_bytes, write_bytes, cancelled_write_bytes; };
static struct mm_struct *get_task_mm(struct task_struct *p) { return p->mm; }
static void mmput(struct mm_struct *mm) { (void)mm; }
static unsigned int mm_pte_shift(const struct mm_struct *mm) { return mm->shift; }
static unsigned long get_mm_rss(const struct mm_struct *mm) { return mm->rss; }
static unsigned long get_mm_hiwater_rss(const struct mm_struct *mm) { return mm->rss; }
static unsigned long get_mm_hiwater_vm(const struct mm_struct *mm) { return mm->total_vm; }
static u64 mul_u64_u64_shr(u64 a, u64 b, unsigned int shift) { return ((__uint128_t)a * b) >> shift; }
static inline u64 mul_u64_u32_div(u64 a, u32 b, u32 div) { return (__uint128_t)a * b / div; }
'''
code += function('include/linux/p3s/mm.h', 'mm_pages_to_kb') + '\n'
code += function('kernel/tsacct.c', '__acct_update_integrals') + '\n'
code += function('kernel/tsacct.c', 'xacct_add_tsk') + '\n'
code += r'''
static void expect_integral(struct task_struct *p, u64 expected, const char *label)
{
    struct taskstats s = {0};
    xacct_add_tsk(&s, p);
    printf("%s got=%"PRIu64" expected=%"PRIu64" MiB-us\n", label, s.coremem, expected);
    if (s.coremem != expected || s.virtmem != expected) exit(1);
}
int main(void)
{
    const u64 sec = 1000000000ULL;
    for (unsigned int shift = 12; shift <= 14; shift += 2) {
        struct mm_struct mm = { .rss = (16ULL << 30) >> shift, .shift = shift };
        mm.total_vm = mm.rss;
        struct task_struct p = { .mm = &mm };
        u64 seconds = 150 * 86400ULL;
        for (u64 t = 1; t <= seconds; ++t) __acct_update_integrals(&p, t * sec, 0);
        expect_integral(&p, 16384ULL * seconds * 1000000, "150 CPU days, 16GiB");
    }
    /* Fractional native pages are preserved before the wide multiply. */
    struct mm_struct mm = { .rss = ((64UL << 20) + 4096) >> 12, .shift = 12 };
    mm.total_vm = mm.rss;
    struct task_struct p = { .mm = &mm };
    __acct_update_integrals(&p, 16 * sec, 0);
    /* Exec into native geometry, with a different resident working set. */
    mm.shift = 14;
    mm.total_vm = mm.rss = (16UL << 20) >> 14;
    __acct_update_integrals(&p, 32 * sec, 0);
    expect_integral(&p, (64ULL + 16) * 16 * 1000000 + 62500, "partial 4K page and exec");
    /* Export must not overflow its intermediate scaling multiplication. */
    p.acct_rss_mem1 = p.acct_vm_mem1 = UINT64_MAX / 2;
    expect_integral(&p, (__uint128_t)(UINT64_MAX / 2) * 16 / 1000, "wide export");
    puts("PASS: native baseline headroom, cross-exec fractions and wide export");
    return 0;
}
'''
with tempfile.TemporaryDirectory(prefix='p3s-taskstats-') as tmp:
    c = Path(tmp) / 'integral.c'
    out = Path(tmp) / 'integral'
    c.write_text(code)
    subprocess.run(['cc', '-std=gnu11', '-O2', '-Wall', '-Wextra', '-Werror',
                    str(c), '-o', str(out)], check=True)
    subprocess.run([str(out)], check=True)
