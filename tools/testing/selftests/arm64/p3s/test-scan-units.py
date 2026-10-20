#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host unit test of the actual NUMA window calculation, without NUMA hardware.

Compile the production calculation and conversion helper verbatim, with only
counter/geometry accessors stubbed. Expected windows come from resident bytes
and the configured 32MiB scan size. No synthetic copy of the algorithm.
"""
from pathlib import Path
import re,subprocess,tempfile
root=Path(__file__).resolve().parents[5]
def function(path,name):
 s=(root/path).read_text();m=re.search(r'^static[^\n]*\b'+name+r'\(',s,re.M)
 assert m,name
 start=m.start();a=s.index('{',m.end());depth=0
 for i in range(a,len(s)):
  if s[i]=='{':depth+=1
  if s[i]=='}':
   depth-=1
   if not depth:return s[start:i+1]
 raise AssertionError(name)
code='''#include <stdio.h>
#include <stdlib.h>
#include <stddef.h>
struct mm_struct { unsigned long rss; unsigned int shift; };
struct task_struct { struct mm_struct *mm; };
#define DIV_ROUND_UP(n,d) (((n)+(d)-1)/(d))
#define PAGE_SHIFT_KERNEL 14
#define MB_TO_PAGES(mb) ((mb) << (20-14))
#define round_up(n,d) (((n)+(d)-1) & ~((d)-1))
static unsigned long sysctl_numa_balancing_scan_size=32;
static unsigned int mm_pte_shift(const struct mm_struct *mm) { return mm ? mm->shift : 14; }
static unsigned long get_mm_rss(struct mm_struct *mm) { return mm->rss; }
'''
code+=function('include/linux/p3s/mm.h','mm_slices_per_page')+'\n'
code+=function('include/linux/p3s/mm.h','mm_pages_to_native')+'\n'
code+=function('kernel/sched/fair.c','task_nr_scan_windows')+'\n'
code+='''int main(void) {
 unsigned long kib[] = {0,16384,65536,73728,205824,4,32772};
 unsigned int expected[] = {1,1,2,3,7,1,2};
 for (unsigned int s=12;s<=14;s+=2) for (unsigned int i=0;i<7;i++) {
  struct mm_struct mm = { .rss=((kib[i]<<10)+(1UL<<s)-1)>>s, .shift=s };
  struct task_struct p = { .mm=&mm };
  unsigned int n=task_nr_scan_windows(&p);
  printf("shift=%u resident_request=%luKiB windows=%u expected=%u\\n",s,kib[i],n,expected[i]);
  if(n!=expected[i])return 1;
 }
 return 0;
}
'''
with tempfile.TemporaryDirectory(prefix='p3s-scan-') as t:
 c=Path(t)/'scan.c';out=Path(t)/'scan';c.write_text(code)
 subprocess.run(['cc','-std=gnu11','-O2','-Wall','-Wextra','-Werror',str(c),'-o',str(out)],check=True)
 subprocess.run([str(out)],check=True)
print('PASS: production NUMA scan windows use resident bytes in both geometries')
