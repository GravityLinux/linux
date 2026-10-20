#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only
# Root on a candidate 16K host. Only this test's cgroups are constrained.
set -euo pipefail
[[ $(id -u) == 0 ]] || { echo "SKIP: root required"; exit 4; }
cd -- "$(dirname -- "$(readlink -f -- "$0")")"
[[ $(getconf PAGESIZE) == 16384 ]]
[[ $(wc -l < /proc/swaps) == 1 ]]
prefix=warthog-oom-verify-$$
slice=$prefix-check.slice
cleanup() {
 systemctl stop "$prefix-native" "$prefix-sparse" "$prefix-pressure" 2>/dev/null || true
 systemctl stop "$slice" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
systemctl set-property --runtime "$slice" MemoryMax=448M MemorySwapMax=0 MemoryAccounting=yes
start() {
 systemd-run --unit="$1" --slice="$slice" --property=OOMScoreAdjust=0 \
  --property=OOMPolicy=continue --property=RemainAfterExit=yes \
  --property=RuntimeMaxSec=180 "${@:2}"
}
ready() {
 for ((i=0;i<60;i++)); do
  journalctl -b -u "$1" --no-pager | grep -q READY && return 0
  [[ $(systemctl show "$1" -p ExecMainStatus --value) != 9 ]] || return 1
  sleep 1
 done
 return 1
}
start "$prefix-native" "$PWD/test-oom-backing" dense --hold 128
ready "$prefix-native"
start "$prefix-sparse" "$PWD/exec-4k" "$PWD/test-oom-backing" sparse --hold 256
ready "$prefix-sparse"
sparse_pid=$(systemctl show "$prefix-sparse" -p MainPID --value)
cg=$(systemctl show "$slice" -p ControlGroup --value)
for unit in native sparse; do
 pid=$(systemctl show "$prefix-$unit" -p MainPID --value)
 echo "$unit pid=$pid"
 grep -E '^(VmRSS|RssAnon|VmPTE):' /proc/"$pid"/status
 cat /proc/"$pid"/oom_score
 done
start "$prefix-pressure" "$PWD/test-oom-backing" dense --hold 128
# A pressure task can itself die after the first victim. Do not require READY.
first=
for ((i=0;i<30;i++)); do
 first=$(dmesg | grep -F "oom_memcg=$cg," | grep -F 'oom-kill:constraint=CONSTRAINT_MEMCG,' | head -1 || true)
 [[ -z $first ]] || break
 sleep 1
 done
[[ -n $first ]]
printf '%s\n' "$first"
[[ $first == *"pid=$sparse_pid,"* ]]
[[ $(awk '$1=="oom_kill" {print $2}' "/sys/fs/cgroup$cg/memory.events") -ge 1 ]]
sleep 1
for unit in native sparse pressure; do
 systemctl show "$prefix-$unit" -p Id -p MainPID -p ExecMainStatus
 done
cat "/sys/fs/cgroup$cg/memory.events"
dmesg > "$prefix-dmesg.log"
if grep -Ei '\bWARNING:|\bBUG:|\bOops:|Kernel panic|Unable to handle kernel|BTRFS.*(error|corrupt)|asahi.*(fault|timeout|hang)' "$prefix-dmesg.log"; then
 echo 'FAIL: kernel fault'; exit 1
fi
echo 'PASS: correct sparse first victim, no kernel faults; later kills are diagnostic'
