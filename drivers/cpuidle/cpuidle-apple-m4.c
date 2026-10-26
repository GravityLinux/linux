// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Copyright The Asahi Linux Contributors
 *
 * CPU idle support for Apple M4 (T8132) and later SoCs
 *
 * On these cores WFI and WFE do not retain CPU state, and a core that wakes
 * from a state-losing WFI restarts at its reset vector. The boot loader's reset
 * vector checks a per-CPU resume slot (its address is in the CPU node's
 * "apple,idle-resume-addr" property) and, if it is armed, jumps there with the
 * MMU off. Idle therefore saves the CPU context with cpu_suspend(), arms the
 * slot with the physical address of cpu_resume and executes WFI. Should WFI
 * return with the context intact, the slot is disarmed and the idle period
 * ends normally.
 */

#define pr_fmt(fmt) "cpuidle-apple-m4: " fmt

#include <linux/cpu.h>
#include <linux/cpu_pm.h>
#include <linux/cpuidle.h>
#include <linux/debugfs.h>
#include <linux/init.h>
#include <linux/io.h>
#include <linux/moduleparam.h>
#include <linux/of.h>
#include <linux/percpu.h>
#include <linux/platform_device.h>
#include <linux/sched/clock.h>
#include <linux/sched/idle.h>
#include <linux/seq_file.h>
#include <linux/string.h>

#include <asm/cacheflush.h>
#include <asm/suspend.h>

enum idle_state {
	STATE_POLL,
	STATE_PWRDOWN,
	STATE_COUNT
};

static bool enable;
module_param(enable, bool, 0444);
MODULE_PARM_DESC(enable, "Register the power-down idle state (default: off)");

static unsigned int exit_latency_us = 100;
module_param(exit_latency_us, uint, 0444);
MODULE_PARM_DESC(exit_latency_us, "Power-down state exit latency in us");

static unsigned int target_residency_us = 2000;
module_param(target_residency_us, uint, 0444);
MODULE_PARM_DESC(target_residency_us, "Power-down state target residency in us");

struct apple_m4_idle_cpu {
	u64 *slot;
	u64 resumed;
	u64 retained;
};

static DEFINE_PER_CPU(struct apple_m4_idle_cpu, apple_m4_idle_cpu);

#define POLL_RELAX_COUNT	200

/*
 * WFI is not usable as a shallow state here, so the shallow state polls, in
 * the same way as the generic poll state (which arm64 does not build).
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

static __always_inline void apple_m4_set_slot(u64 *slot, u64 entry)
{
	WRITE_ONCE(*slot, entry);
	/* The reset vector reads the slot with the MMU and caches off */
	dcache_clean_poc((unsigned long)slot, (unsigned long)(slot + 1));
}

/* Runs with RCU idle, after cpu_suspend()'s ct_cpuidle_enter() */
static noinstr int apple_m4_powerdown_finisher(unsigned long arg)
{
	u64 *slot = (u64 *)arg;

	apple_m4_set_slot(slot, __pa_symbol(cpu_resume));
	dsb(sy);
	wfi();

	/* Still here: WFI kept the context, so cpu_suspend() returns 1 */
	apple_m4_set_slot(slot, 0);
	return 1;
}

static __cpuidle int apple_m4_enter_pwrdown(struct cpuidle_device *dev,
					    struct cpuidle_driver *drv, int index)
{
	struct apple_m4_idle_cpu *cpu = this_cpu_ptr(&apple_m4_idle_cpu);
	int ret;

	/*
	 * Powering down clobbers FP state, the timers and the AIC's per-CPU
	 * masks, among other things. The CPU PM notifiers save and restore
	 * them. cpu_suspend() handles the RCU/context-tracking transition, so
	 * this state is CPUIDLE_FLAG_RCU_IDLE.
	 */
	if (cpu_pm_enter())
		return -1;

	ret = cpu_suspend((unsigned long)cpu->slot, apple_m4_powerdown_finisher);
	if (ret == 0) {
		/* Resumed through the reset vector; the slot is still armed */
		apple_m4_set_slot(cpu->slot, 0);
		cpu->resumed++;
	} else if (ret == 1) {
		cpu->retained++;
	}

	cpu_pm_exit();

	return ret < 0 ? -1 : index;
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
			.flags			= CPUIDLE_FLAG_RCU_IDLE |
						  CPUIDLE_FLAG_TIMER_STOP,
		},
	},
	.safe_state_index = STATE_POLL,
	.state_count = STATE_COUNT,
};

static int apple_m4_idle_stats_show(struct seq_file *s, void *unused)
{
	unsigned int cpu;

	seq_puts(s, "cpu resumed retained\n");
	for_each_possible_cpu(cpu) {
		struct apple_m4_idle_cpu *c = per_cpu_ptr(&apple_m4_idle_cpu, cpu);

		seq_printf(s, "%u %llu %llu\n", cpu, READ_ONCE(c->resumed),
			   READ_ONCE(c->retained));
	}
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(apple_m4_idle_stats);

static int apple_m4_map_slots(void)
{
	unsigned int cpu;

	for_each_possible_cpu(cpu) {
		struct device_node *np = of_get_cpu_node(cpu, NULL);
		u64 addr;
		int ret;

		if (!np)
			return -ENODEV;
		ret = of_property_read_u64(np, "apple,idle-resume-addr", &addr);
		of_node_put(np);
		if (ret) {
			pr_info("CPU%u has no resume slot, not registering\n", cpu);
			return -ENODEV;
		}
		if (!IS_ALIGNED(addr, sizeof(u64)))
			return -EINVAL;

		per_cpu(apple_m4_idle_cpu, cpu).slot =
			memremap(addr, sizeof(u64), MEMREMAP_WB);
		if (!per_cpu(apple_m4_idle_cpu, cpu).slot)
			return -ENOMEM;
	}

	return 0;
}

static void apple_m4_unmap_slots(void)
{
	unsigned int cpu;

	for_each_possible_cpu(cpu) {
		struct apple_m4_idle_cpu *c = per_cpu_ptr(&apple_m4_idle_cpu, cpu);

		if (c->slot)
			memunmap(c->slot);
		c->slot = NULL;
	}
}

static int apple_m4_cpuidle_probe(struct platform_device *pdev)
{
	struct cpuidle_state *pd = &apple_m4_idle_driver.states[STATE_PWRDOWN];
	int ret;

	ret = apple_m4_map_slots();
	if (ret) {
		apple_m4_unmap_slots();
		return ret;
	}

	pd->exit_latency = exit_latency_us;
	pd->target_residency = target_residency_us;
	apple_m4_idle_driver.cpumask = (struct cpumask *)cpu_possible_mask;

	ret = cpuidle_register(&apple_m4_idle_driver, NULL);
	if (ret) {
		apple_m4_unmap_slots();
		return ret;
	}

	/*
	 * The kernel boots with nohlt, because the default idle's WFI would
	 * lose the CPU. Now that idle goes through this driver, whose only
	 * other state polls, stop forcing the poll loop.
	 */
	cpu_idle_poll_ctrl(false);

	debugfs_create_file("cpuidle_apple_m4", 0444, NULL, NULL,
			    &apple_m4_idle_stats_fops);
	pr_info("registered power-down idle (exit %u us, residency %u us)\n",
		exit_latency_us, target_residency_us);
	return 0;
}

static struct platform_driver apple_m4_cpuidle_driver = {
	.driver = {
		.name = "cpuidle-apple-m4",
	},
	.probe = apple_m4_cpuidle_probe,
};

/*
 * cpu_idle_force_poll is private to the scheduler, so look for the nohlt that
 * raised it before lowering it in probe.
 */
static bool __init apple_m4_have_nohlt(void)
{
	const char *p = boot_command_line;

	while ((p = strstr(p, "nohlt"))) {
		if ((p == boot_command_line || p[-1] == ' ') &&
		    (p[5] == ' ' || p[5] == '\0'))
			return true;
		p += 5;
	}
	return false;
}

static int __init apple_m4_cpuidle_init(void)
{
	struct platform_device *pdev;
	int ret;

	if (!enable)
		return 0;

	if (!apple_m4_have_nohlt()) {
		pr_warn("needs nohlt on the command line, not registering\n");
		return 0;
	}

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
