// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Copyright The Asahi Linux Contributors
 *
 * CPU idle support for Apple M4 (T8132) and later SoCs
 *
 * On these cores a WFI that lets the core power down does not keep the
 * general purpose registers: execution continues after the WFI, with the stack
 * pointer and the system registers intact but x0-x30 cleared. The power-down
 * state therefore saves the callee-saved registers and the link register on
 * the stack around WFI and reloads them afterwards, as the deep WFI of the
 * Asahi cpuidle-apple driver does on earlier SoCs (without its CYC_OVRD
 * write, which M4 does not have). The CPU PM notifiers save and restore what
 * the rest of the kernel keeps in the core: FP state, timers and the
 * interrupt controller's per-CPU setup.
 */

#define pr_fmt(fmt) "cpuidle-apple-m4: " fmt

#include <linux/context_tracking.h>
#include <linux/cpu.h>
#include <linux/cpu_pm.h>
#include <linux/cpuidle.h>
#include <linux/init.h>
#include <linux/moduleparam.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/sched/clock.h>
#include <linux/sched/idle.h>
#include <linux/string.h>

enum idle_state {
	STATE_POLL,
	STATE_PWRDOWN,
	STATE_COUNT
};

static bool enable;
module_param(enable, bool, 0444);
MODULE_PARM_DESC(enable, "Register the power-down idle state (default: off)");

static char *cpus;
module_param(cpus, charp, 0444);
MODULE_PARM_DESC(cpus, "CPUs that may power down in idle, as a cpulist (default: all)");

static struct cpumask apple_m4_idle_cpus;
static bool apple_m4_idle_nop;

/* Measured: a wake from the state adds about 13 us to a cross-CPU round trip. */
static unsigned int exit_latency_us = 20;
module_param(exit_latency_us, uint, 0444);
MODULE_PARM_DESC(exit_latency_us, "Power-down state exit latency in us");

static unsigned int target_residency_us = 2000;
module_param(target_residency_us, uint, 0444);
MODULE_PARM_DESC(target_residency_us, "Power-down state target residency in us");

/*
 * Unlike the M1-M3 deep WFI, the M4 power-down state does not wake on the
 * local arch timer: without TIMER_STOP the first CPU to enter it never came
 * back. With it, the tick moves to the hrtimer broadcast, whose owner CPU is
 * kept out of the state, so a single power-down CPU never enters it.
 */
static bool timer_stop = true;
module_param(timer_stop, bool, 0444);
MODULE_PARM_DESC(timer_stop, "The local timer stops in the power-down state (default: on)");

#define POLL_RELAX_COUNT	200

/*
 * The shallow state polls, in the same way as the generic poll state (which
 * arm64 does not build): a plain WFI is the state below.
 */
static __cpuidle int apple_m4_enter_poll(struct cpuidle_device *dev,
					 struct cpuidle_driver *drv, int index)
{
	u64 time_start = local_clock_noinstr();

	dev->poll_time_limit = false;

	raw_local_irq_enable();
	if (!current_set_polling_and_test()) {
		unsigned int loop_count = 0;
		u64 limit = cpuidle_poll_time(drv, dev);

		while (!need_resched()) {
			cpu_relax();
			if (loop_count++ < POLL_RELAX_COUNT)
				continue;

			loop_count = 0;
			if (local_clock_noinstr() - time_start > limit) {
				dev->poll_time_limit = true;
				break;
			}
		}
	}
	raw_local_irq_disable();

	current_clr_polling();

	return index;
}

/*
 * WFI, with the registers it does not keep saved on the stack: x18 (shadow
 * call stack) to x30. x0-x17 are caller-saved. Interrupts are masked, so a
 * wakeup shows in ISR_EL1; sleep again until one is pending.
 */
asm(
	".pushsection .cpuidle.text, \"ax\"\n"
	".type apple_m4_lossy_wfi, @function\n"
	"apple_m4_lossy_wfi:\n"
		"stp x29, x30, [sp, #-16]!\n"
		"stp x27, x28, [sp, #-16]!\n"
		"stp x25, x26, [sp, #-16]!\n"
		"stp x23, x24, [sp, #-16]!\n"
		"stp x21, x22, [sp, #-16]!\n"
		"stp x19, x20, [sp, #-16]!\n"
		"str x18, [sp, #-16]!\n"
	"1:\n"
		"dsb sy\n"
		"wfi\n"
		"mrs x0, ISR_EL1\n"
		"cbz x0, 1b\n"
		"ldr x18, [sp], #16\n"
		"ldp x19, x20, [sp], #16\n"
		"ldp x21, x22, [sp], #16\n"
		"ldp x23, x24, [sp], #16\n"
		"ldp x25, x26, [sp], #16\n"
		"ldp x27, x28, [sp], #16\n"
		"ldp x29, x30, [sp], #16\n"
		"ret\n"
	".size apple_m4_lossy_wfi, . - apple_m4_lossy_wfi\n"
	".popsection\n"
);

void apple_m4_lossy_wfi(void);

static __cpuidle int apple_m4_enter_pwrdown(struct cpuidle_device *dev,
					    struct cpuidle_driver *drv, int index)
{
	/*
	 * Powering down clobbers FP state, the timers and the AIC's per-CPU
	 * masks, among other things. The CPU PM notifiers save and restore
	 * them.
	 */
	if (cpu_pm_enter())
		return -1;

	ct_cpuidle_enter();
	apple_m4_lossy_wfi();
	ct_cpuidle_exit();

	cpu_pm_exit();

	return index;
}

static struct cpuidle_driver apple_m4_idle_driver = {
	.name = "apple_m4_idle",
	.owner = THIS_MODULE,
	.states = {
		[STATE_POLL] = {
			.enter			= apple_m4_enter_poll,
			.exit_latency		= 0,
			.target_residency	= 0,
			.power_usage		= UINT_MAX,
			.name			= "POLL",
			.desc			= "CPU polling",
			.flags			= CPUIDLE_FLAG_POLLING,
		},
		[STATE_PWRDOWN] = {
			.enter			= apple_m4_enter_pwrdown,
			.power_usage		= 0,
			.name			= "CPU PD",
			.desc			= "CPU powered down",
			.flags			= CPUIDLE_FLAG_RCU_IDLE,
		},
	},
	.safe_state_index = STATE_POLL,
	.state_count = STATE_COUNT,
};

static int apple_m4_cpuidle_probe(struct platform_device *pdev)
{
	struct cpuidle_state *pd = &apple_m4_idle_driver.states[STATE_PWRDOWN];
	int ret;

	pd->exit_latency = exit_latency_us;
	pd->target_residency = target_residency_us;
	if (timer_stop)
		pd->flags |= CPUIDLE_FLAG_TIMER_STOP;

	/*
	 * The other CPUs get no cpuidle driver and keep the default idle,
	 * which idle=nop keeps from executing WFI.
	 */
	cpumask_copy(&apple_m4_idle_cpus, cpu_possible_mask);
	if (cpus && cpulist_parse(cpus, &apple_m4_idle_cpus)) {
		pr_err("invalid cpus=%s\n", cpus);
		return -EINVAL;
	}
	cpumask_and(&apple_m4_idle_cpus, &apple_m4_idle_cpus, cpu_possible_mask);
	if (cpumask_empty(&apple_m4_idle_cpus))
		return -EINVAL;
	/*
	 * Once polling is off, the CPUs left out fall back to the default idle,
	 * whose WFI only idle=nop keeps them from executing.
	 */
	if (!cpumask_equal(&apple_m4_idle_cpus, cpu_possible_mask) &&
	    !apple_m4_idle_nop) {
		pr_err("cpus=%s needs idle=nop on the command line\n", cpus);
		return -EINVAL;
	}
	apple_m4_idle_driver.cpumask = &apple_m4_idle_cpus;

	ret = cpuidle_register(&apple_m4_idle_driver, NULL);
	if (ret)
		return ret;

	/*
	 * The kernel boots with nohlt, because the default idle's WFI would
	 * lose the CPU's registers. Now that idle goes through this driver,
	 * whose only other state polls, stop forcing the poll loop.
	 */
	cpu_idle_poll_ctrl(false);

	pr_info("registered power-down idle on CPUs %*pbl (exit %u us, residency %u us%s)\n",
		cpumask_pr_args(&apple_m4_idle_cpus), exit_latency_us,
		target_residency_us, timer_stop ? ", timer stop" : "");
	return 0;
}

static struct platform_driver apple_m4_cpuidle_driver = {
	.driver = {
		.name = "cpuidle-apple-m4",
	},
	.probe = apple_m4_cpuidle_probe,
};

/* Whether the boot command line has @tok as a whole word. */
static bool __init apple_m4_cmdline_has(const char *tok)
{
	const char *p = boot_command_line;
	size_t len = strlen(tok);

	while ((p = strstr(p, tok))) {
		if ((p == boot_command_line || p[-1] == ' ') &&
		    (p[len] == ' ' || p[len] == '\0'))
			return true;
		p += len;
	}
	return false;
}

static int __init apple_m4_cpuidle_init(void)
{
	struct platform_device *pdev;
	int ret;

	if (!enable)
		return 0;

	/*
	 * cpu_idle_force_poll is private to the scheduler, so look for the
	 * nohlt that raised it before lowering it in probe.
	 */
	if (!apple_m4_cmdline_has("nohlt")) {
		pr_warn("needs nohlt on the command line, not registering\n");
		return 0;
	}

	apple_m4_idle_nop = apple_m4_cmdline_has("idle=nop");

	if (!of_machine_is_compatible("apple,arm-platform"))
		return 0;

	if (!of_machine_is_compatible("apple,t8132"))
		return 0;

	ret = platform_driver_register(&apple_m4_cpuidle_driver);
	if (ret)
		return ret;

	pdev = platform_device_register_simple("cpuidle-apple-m4", -1, NULL, 0);
	if (IS_ERR(pdev)) {
		platform_driver_unregister(&apple_m4_cpuidle_driver);
		return PTR_ERR(pdev);
	}

	return 0;
}
device_initcall(apple_m4_cpuidle_init);
