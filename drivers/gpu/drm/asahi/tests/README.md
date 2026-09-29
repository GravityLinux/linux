

Native dependency roots and preparation (source-only):

```sh
python3 check_g17p_dependency_vm.py /path/to/m1n1
rustc --edition=2021 -Dwarnings g17p_dependency_prepare.rs -o /tmp/neo-dependency-prepare
/tmp/neo-dependency-prepare
```

The root test executes the actual shim dependency-state join over synthetic
owned leaves and compares complete resulting PTEs with the Rust join. It
includes live mappings differing from the bootstrap inventory, physical aliases
with different attributes, active/dormant collisions, source-ordered state
aliases, separate zero robustness owners, and input/overlap rejection.

The preparation harness runs production queue/graph preparation and mixed
report routing with models of kernel allocation, Memory, and user UAT APIs.
It also compiles the production Vm replacement methods against the checked
Memory API. It verifies held producers, the hidden closing context, scheduler
owner replacement, decoded late low references, the compact descriptors,
CL/growth/receipt/render/CL report interleaving, credit retention on invalid
reports, and unchanged standalone report admission. The intentional unknown
low-reference rejection prints one ownership diagnostic before PASS.

Native release and retirement (source-only):

```sh
python3 check_g17p_dependency_release.py /path/to/m1n1
python3 check_g17p_dependency_retire.py /path/to/m1n1
```

Release executes the actual shim publication tail and a bounded interpreter
for its two source-authored host ASM primitives over synthetic owned RAM. Full
store/control/mailbox traces agree for 16 seeds. Class-state failures, rejected
boundaries and all 49 access-failure prefixes retain the exact published prefix.
No firmware instructions are interpreted. Retirement executes the source
G17PQueueFence predicates, render status snapshot and finish-order assignment:
147456 combinations cover independent queue/consumer/status/growth gates and
8-bit wrap. Failure and incorrect retirement order are terminal.

The production preparation harness also runs the concrete control adapter:
seven control bodies, ten notifications, independent consumer advancement and
finite control-window admission. It reads real production snapshot destinations
in modeled RAM, then retires closing CL, render and opening CL. Neither queue
retirement nor another command's status is a completion witness. Its mixed
report checks now include allocation refusal, owned native fragment event 2,
and retained credits on wrong event or duplicate limit.

These checks do not execute the target's CPU cache/TLBI instructions, RTKit
notifications, firmware, or GPU workloads. Preparation remains disconnected
from Session pending its RTKit/lifetime/timeout/copyback integration, cold
bootstrap and hardware qualification. They do not establish native GPU execution or stage-one
parity.
