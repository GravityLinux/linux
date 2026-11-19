// SPDX-License-Identifier: GPL-2.0
/*
 * Emulate a local clock event device via a pseudo clock device.
 */
#include <linux/cpu.h>
#include <linux/err.h>
#include <linux/hrtimer.h>
#include <linux/interrupt.h>
#include <linux/percpu.h>
#include <linux/profile.h>
#include <linux/clockchips.h>
#include <linux/sched.h>
#include <linux/smp.h>
#include <linux/module.h>

#include "tick-internal.h"

#undef MODULE_PARAM_PREFIX
#define MODULE_PARAM_PREFIX "tick_broadcast_hrtimer."

static struct hrtimer bctimer;
static struct clock_event_device ce_broadcast_hrtimer;

/* Opt-in until timer latency and power have been measured on the target. */
static int preferred_cpu = -1;
module_param_named(cpu, preferred_cpu, int, 0444);
MODULE_PARM_DESC(cpu, "Preferred hrtimer broadcast CPU (-1: follow the caller)");

#ifdef CONFIG_SMP
DEFINE_STATIC_KEY_FALSE(tick_broadcast_hrtimer_preferred_key);

static call_single_data_t bc_handoff = CSD_INIT(tick_broadcast_hrtimer_reprogram,
					       &ce_broadcast_hrtimer);

/* Called with tick_broadcast_lock held, including from the idle entry path. */
void tick_broadcast_hrtimer_queue(struct clock_event_device *bc)
{
	int cpu = preferred_cpu;

	if (bc != &ce_broadcast_hrtimer || cpu < 0 || cpu >= nr_cpu_ids ||
	    !cpu_online(cpu) || cpu == smp_processor_id() ||
	    bc->bound_on == cpu || bc->next_event == KTIME_MAX)
		return;

	/*
	 * Never wait under tick_broadcast_lock: the remote callback takes it.
	 * A busy CSD already carries a request; it reads the latest deadline
	 * under that lock. An offline target leaves the local timer intact.
	 */
	smp_call_function_single_async(cpu, &bc_handoff);
}
#else
void tick_broadcast_hrtimer_queue(struct clock_event_device *bc) { }
#endif

static int bc_shutdown(struct clock_event_device *evt)
{
	/*
	 * Note, we cannot cancel the timer here as we might
	 * run into the following live lock scenario:
	 *
	 * cpu 0		cpu1
	 * lock(broadcast_lock);
	 *			hrtimer_interrupt()
	 *			bc_handler()
	 *			   tick_handle_oneshot_broadcast();
	 *			    lock(broadcast_lock);
	 * hrtimer_cancel()
	 *  wait_for_callback()
	 */
	hrtimer_try_to_cancel(&bctimer);
	return 0;
}

/*
 * This is called from the guts of the broadcast code when the cpu
 * which is about to enter idle has the earliest broadcast timer event.
 */
static int bc_set_next(ktime_t expires, struct clock_event_device *bc)
{
	/*
	 * This is called either from enter/exit idle code or from the
	 * broadcast handler. In all cases tick_broadcast_lock is held.
	 *
	 * hrtimer_cancel() cannot be called here neither from the
	 * broadcast handler nor from the enter/exit idle code. The idle
	 * code can run into the problem described in bc_shutdown() and the
	 * broadcast handler cannot wait for itself to complete for obvious
	 * reasons.
	 *
	 * Each caller tries to arm the hrtimer on its own CPU, but if the
	 * hrtimer callback function is currently running, then
	 * hrtimer_start() cannot move it and the timer stays on the CPU on
	 * which it is assigned at the moment.
	 */
	hrtimer_start(&bctimer, expires, HRTIMER_MODE_ABS_PINNED_HARD);
	/*
	 * The core tick broadcast mode expects bc->bound_on to be set
	 * correctly to prevent a CPU which has the broadcast hrtimer
	 * armed from going deep idle.
	 *
	 * As tick_broadcast_lock is held, nothing can change the cpu
	 * base which was just established in hrtimer_start() above. So
	 * the below access is safe even without holding the hrtimer
	 * base lock.
	 */
	bc->bound_on = bctimer.base->cpu_base->cpu;
	if (tick_broadcast_hrtimer_preferred_enabled())
		tick_broadcast_hrtimer_queue(bc);

	return 0;
}

static struct clock_event_device ce_broadcast_hrtimer = {
	.name			= "bc_hrtimer",
	.set_state_shutdown	= bc_shutdown,
	.set_next_ktime		= bc_set_next,
	.features		= CLOCK_EVT_FEAT_ONESHOT |
				  CLOCK_EVT_FEAT_HRTIMER,
	.rating			= 0,
	.bound_on		= -1,
	.min_delta_ns		= 1,
	.max_delta_ns		= KTIME_MAX,
	.min_delta_ticks	= 1,
	.max_delta_ticks	= ULONG_MAX,
	.mult			= 1,
	.shift			= 0,
	.cpumask		= cpu_possible_mask,
};

static enum hrtimer_restart bc_handler(struct hrtimer *t)
{
	ce_broadcast_hrtimer.event_handler(&ce_broadcast_hrtimer);

	return HRTIMER_NORESTART;
}

void tick_setup_hrtimer_broadcast(void)
{
	if (preferred_cpu < -1 || preferred_cpu >= nr_cpu_ids ||
	    (preferred_cpu >= 0 && !cpu_possible(preferred_cpu))) {
		pr_warn("bc_hrtimer: invalid preferred CPU %d, using the caller\n",
			preferred_cpu);
		preferred_cpu = -1;
	}
	hrtimer_setup(&bctimer, bc_handler, CLOCK_MONOTONIC, HRTIMER_MODE_ABS_HARD);
	clockevents_register_device(&ce_broadcast_hrtimer);
}

#ifdef CONFIG_SMP
static int __init bc_preferred_cpu_init(void)
{
	/* Static-key patching needs process context, not time_init(). */
	if (preferred_cpu >= 0)
		static_branch_enable(&tick_broadcast_hrtimer_preferred_key);
	return 0;
}
early_initcall(bc_preferred_cpu_init);
#endif
