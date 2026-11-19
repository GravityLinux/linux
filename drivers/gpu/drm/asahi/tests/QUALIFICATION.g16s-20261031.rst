Experimental G16S / T6040 GPU enablement, 2026-10-31
==================================================

Scope and integration
---------------------

This adds the reported 20-core M4 Pro (T6040/G16S, J773s) to the existing
single-ASC G16C implementation. G16S, G16C and G17P remain one macro axis;
G16S uses the Max pointer topology, control protocol, scheduler and lifetimes.
The new profile selects identity, cluster/core counts, operand allocation and
register offsets, deflake offsets and hardware/power templates. No Pro-specific
scheduler was introduced. Other Pro bins have not been qualified.

Linux parent: c1dc9416af0f752efe7e6e41df98f4d093b1dd0f.
Use these isolated companion commits:

* Bootloader: 81902ab7, experimental/g16s-enablement, worktree
  /Users/user/gravity/bootloader-g16s-enablement.
* Mesa: 4c576f3559a, experimental/g16s-enablement, worktree
  /Users/user/gravity/mesa-vulkan-apple9.

The bootloader change dispatches T6040 to the existing GPU FDT builder,
accepts two voltage columns, and supplies the Pro maximum power. Linux consumes
typed FDT nodes, never raw ADT. Include t6040-gpu.dtsi from a working T6040
platform tree providing aic and ps_gfx. This is a GPU fragment, not a new
complete Mac mini platform port. The fragment includes five bootloader-filled
reservations and the shared eleven GPU/eight CS OPP slots, with two voltage
columns. Its GPU and mailbox share an always-on power domain.

The kernel reports variant S and chip 0x6040 truthfully. Mesa accepts S, selects
the existing multi-cluster G16C ISA path and uses the compact 4 GiB private
render-context window. A Mesa build lacking this change is not suitable.
Use macOS 26.6.2/25G83 GPU firmware. The Pro accepts the reported chainloading
iBoot stamp mBoot-20457.1.29 in addition to mBoot-18000.161.10; this does not
qualify other firmware ABIs.

partial_independent_owner must remain zero on Pro. Probe rejects a nonzero
setting because the report describes a discard/texture hang with two retained
render pools. The reported 0xfbc hardware word remains four; it is not blindly
scaled to the active cluster count. No unreported changes to MMIO topology,
queue VA spacing, preemption allocation policy or lifecycle were inferred.

Host evidence
-------------

check_g16s_profile.py compiles the production version macro and Rust sources.
It compares sizes, alignment and all 948 field offsets in 17 actual generated
G16S/G16C shapes, in addition to the existing Max RE layout assertions. It
checks the Pro compute operand/preemption constants and unchanged Max/Neo
constants. The saved J773s ADT scalar inputs pass through actual bootloader C
FDT population and the actual Linux board/parser/boot-graph encoders. Checks
cover the power/thermal template, two voltage columns, chip/core fields,
1578 MHz maximum, eight CS states, single firmware root and invalid table
sizes rejected before mutation. The Pro GPU fragment compiles through CPP/dtc;
the only warnings are inherited empty reservation names lacking unit addresses.
The GPU binding parses as YAML; full dt-schema validation was not run.

Run from the Linux tree (Python 3.10+ and Rust required)::

  python3 drivers/gpu/drm/asahi/tests/check_g16s_profile.py \
    /path/to/bootloader-with-g16s \
    --fdt-template /path/to/Max-host-fixture.dtb \
    --evidence /path/to/new-evidence

The fixture is the existing host-only /gpu tree with the shared OPP includes;
the test resizes its voltage columns before calling the actual C population
routine. It does not claim to compare against a captured Pro firmware image.

Existing regression gates also passed:

* Four complete Max boot graphs: 144 full RAM owners compared byte-for-byte,
  including aliases, channels, MMIO and rejection-before-mutation cases.
* 195 complete Max compute pages, including two native descriptor DATA images;
  256 complete Neo pages compared with the original tagged driver.
* 260 render cases, four independent native DATA images, and existing Neo
  serialization comparisons.
* 203 power fields / 406 independent offset-width assertions, 48 complete
  controller images and 24 invalid-property/board cases.
* Historical Neo constructor, probe, identity and scheduler-shape checks.

Hardware evidence and limits
----------------------------

Pro hardware was unavailable and remains UNTESTED. Neo hardware was not
accessed. These host checks establish consistency with the supplied report and
saved ADT, not actual Pro execution or complete support for all T6040 machines.

The changed kernel and a fresh complete ARM64 Mesa Vulkan/Zink build were
regression-tested on M4 Max RID0 only (Mac16,5/J616c, chip 0x6041, exact serial
JPWWJRX2N0). All 16 CPUs remained online using spin-table startup.

* Two native Vulkan clients -> DMA-BUF export/import -> physical DCP scanout:
  256 frames / 255 flips, 3456x2160, 119.514 fps. No CPU framebuffer copy.
* Native memory UAPI: all 6313 checks passed.
* Native 200000-instance PB pressure: two generations at each of explicit-close
  origin 0x7000000000 and file-close origin 0x7100004000; full color/arguments,
  query, six timestamps, 294884 guards per generation and prior-bank poison.
* Zink/GLES copy: two 16 MiB BOs, full independent copy oracle passed.
* GPU unbind RC0; no GPU fault, timeout, firmware crash or kernel warning.
  Target returned to the development proxy with the serial port released.

This is a native presentation regression test, not a game/compositor benchmark
or Vulkan conformance result. Pro concurrency and long-running workloads still
need hardware testing. The tested kernel's code is the implementation here;
the new scheduled_profile.rs was subsequently whitespace-formatted only.

Kernel Image SHA256:
1fb3c4b705a483acf131ad79095ae2be217c39f71bde00cf3164029d237677f2
Fresh Vulkan DSO SHA256:
61efc9cc3dca003a7c1f5d570f7fcb00e9a160640a302de9d242be76b1d00f57

Full local artifacts (build logs, inputs, generated layouts, boot graph,
monitor/dmesg, exact binaries, runtime manifest and results.json):
/Users/user/gravity/bootloader-m4maxmbp/build/g16s-enablement-20261031/.
Remote build source: lab@192.168.170.255:/home/lab/linux-gpu-performance-7.3;
Mesa source: /home/lab/mesa-g16s-enablement at the documented Mesa parent plus
its isolated patch. No Apple binary disassembly was performed.
