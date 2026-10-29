// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* T8132 P-core, E-cluster and GPU activity inputs to PMP's memory and fabric policy. */
#include <linux/atomic.h>
#include <linux/cpu.h>
#include <linux/cpufreq.h>
#include <linux/cpuhotplug.h>
#include <linux/device.h>
#include <linux/io.h>
#include <linux/ktime.h>
#include <linux/math64.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/smp.h>
#include <linux/soc/apple/pmp-report.h>
#include <linux/workqueue.h>

#include "pmp-cpu-vote.h"
#include "pmp-vote-policy.h"

#define CPU_VOTE_INTERVAL_MS 20
#define CPU_VOTE_THRESHOLD 50
#define CPU_VOTE_HOLD_MS 80

static bool cpu_vote_enabled = true;
module_param_named(cpu_vote, cpu_vote_enabled, bool, 0444);
MODULE_PARM_DESC(cpu_vote, "Enable the T8132 CPU/GPU memory/fabric vote");

/* Changes take effect at the next policy evaluation. */
static bool cpu_vote_cpm = true;
module_param_named(cpu_vote_cpm, cpu_vote_cpm, bool, 0644);
MODULE_PARM_DESC(cpu_vote_cpm, "Tie T8132 cluster CPM bit0 to the CPU/GPU vote");

static unsigned int cpu_vote_sources =
	APPLE_PMP_VOTE_CPU | APPLE_PMP_VOTE_GPU | APPLE_PMP_VOTE_E_CPU;
static atomic_t gpu_flights = ATOMIC_INIT(0);
static atomic64_t gpu_last_ms = ATOMIC64_INIT(0);
static atomic64_t gpu_busy_clock = ATOMIC64_INIT(0);

/* Reject transient desktop work; zero GPU threshold selects legacy pulses. */
static unsigned int cpu_vote_p_samples = 1;
static unsigned int cpu_vote_e_samples = 1;
static unsigned int cpu_vote_gpu_percent = 40;
static unsigned int cpu_vote_gpu_window_ms = 100;

static int cpu_vote_set_filter(const char *text, const struct kernel_param *kp)
{
	unsigned int value;
	int ret = kstrtouint(text, 0, &value);

	if (ret)
		return ret;
	if (kp->arg == &cpu_vote_gpu_percent) {
		if (value > 100)
			return -EINVAL;
	} else if (kp->arg == &cpu_vote_gpu_window_ms) {
		if (value < 20 || value > 500 || value % 20)
			return -EINVAL;
	} else if (!value || value > 5) {
		return -EINVAL;
	}
	WRITE_ONCE(*(unsigned int *)kp->arg, value);
	return 0;
}

static const struct kernel_param_ops cpu_vote_filter_ops = {
	.set = cpu_vote_set_filter,
	.get = param_get_uint,
};
module_param_cb(cpu_vote_p_samples, &cpu_vote_filter_ops, &cpu_vote_p_samples, 0644);
MODULE_PARM_DESC(cpu_vote_p_samples, "Consecutive P-core occupied samples (1..5)");
module_param_cb(cpu_vote_e_samples, &cpu_vote_filter_ops, &cpu_vote_e_samples, 0644);
MODULE_PARM_DESC(cpu_vote_e_samples, "Consecutive E aggregate occupied samples (1..5)");
module_param_cb(cpu_vote_gpu_percent, &cpu_vote_filter_ops, &cpu_vote_gpu_percent, 0644);
MODULE_PARM_DESC(cpu_vote_gpu_percent, "GPU in-flight union busy percent (0 = legacy)");
module_param_cb(cpu_vote_gpu_window_ms, &cpu_vote_filter_ops, &cpu_vote_gpu_window_ms, 0644);
MODULE_PARM_DESC(cpu_vote_gpu_window_ms, "GPU busy window ms (20..500, multiple of 20)");

static int cpu_vote_set_sources(const char *text, const struct kernel_param *kp)
{
	unsigned int value;
	int ret = kstrtouint(text, 0, &value);

	if (ret)
		return ret;
	if (value & ~(APPLE_PMP_VOTE_CPU | APPLE_PMP_VOTE_GPU |
		      APPLE_PMP_VOTE_E_CPU))
		return -EINVAL;
	WRITE_ONCE(*(unsigned int *)kp->arg, value);
	return 0;
}

static const struct kernel_param_ops cpu_vote_sources_ops = {
	.set = cpu_vote_set_sources,
	.get = param_get_uint,
};
module_param_cb(cpu_vote_sources, &cpu_vote_sources_ops, &cpu_vote_sources, 0644);
MODULE_PARM_DESC(cpu_vote_sources, "Activity mask: bit0 P-core, bit1 GPU, bit2 E-cluster");

/* Summed non-idle percent: the default is one occupied E-core equivalent. */
static unsigned int cpu_vote_e_threshold = 100;

static int cpu_vote_set_e_threshold(const char *text, const struct kernel_param *kp)
{
	unsigned int value;
	int ret = kstrtouint(text, 0, &value);

	if (ret)
		return ret;
	if (!value || value > 600)
		return -EINVAL;
	WRITE_ONCE(*(unsigned int *)kp->arg, value);
	return 0;
}

static const struct kernel_param_ops cpu_vote_e_threshold_ops = {
	.set = cpu_vote_set_e_threshold,
	.get = param_get_uint,
};
module_param_cb(cpu_vote_e_threshold, &cpu_vote_e_threshold_ops,
		&cpu_vote_e_threshold, 0644);
MODULE_PARM_DESC(cpu_vote_e_threshold,
		"E-cluster aggregate non-idle percent (100 = one busy core)");

/* Scalar publication only: no GPU pointer, allocation, locking or MMIO. */
void apple_pmp_cpu_vote_gpu_activity(unsigned int active)
{
	u64 old, next, now;

	if (!READ_ONCE(cpu_vote_enabled))
		return;

	do {
		old = atomic64_read(&gpu_busy_clock);
		now = ktime_get_ns();
		next = apple_pmp_vote_gpu_clock(old, now, active != 0);
	} while (atomic64_cmpxchg(&gpu_busy_clock, old, next) != old);
	atomic64_set(&gpu_last_ms, div_u64(now, NSEC_PER_MSEC));
	atomic_set(&gpu_flights, active);
}
EXPORT_SYMBOL_GPL(apple_pmp_cpu_vote_gpu_activity);

/* Low nibble: DCS F1..F6; high nibble: SoC VMIN..VOVD. */
static unsigned int cpu_vote_value = 0x35;

static int cpu_vote_set_value(const char *text, const struct kernel_param *kp)
{
	unsigned int value;
	int ret = kstrtouint(text, 0, &value);

	if (ret)
		return ret;
	if (!apple_pmp_vote_valid(value))
		return -EINVAL;
	WRITE_ONCE(*(unsigned int *)kp->arg, value);
	return 0;
}

static const struct kernel_param_ops cpu_vote_value_ops = {
	.set = cpu_vote_set_value,
	.get = param_get_uint,
};
module_param_cb(cpu_vote_value, &cpu_vote_value_ops, &cpu_vote_value, 0644);
MODULE_PARM_DESC(cpu_vote_value, "CPU-load vote: DCS 0..5 and SoC 0..3");

struct cpu_sample {
	struct delayed_work work;
	struct apple_pmp_cpu_vote *vote;
	u64 idle, wall;
	struct apple_pmp_vote_cpu_time e_time;
	struct apple_pmp_vote_run p_run;
	unsigned int cpu;
	bool enabled, valid;
};

struct cpm_cluster {
	void __iomem *reg;
	cpumask_t cpus;
	u32 saved_bit;
	bool saved, valid, raised;
};

struct vote_source_stats {
	u64 start_ms, until_ms, completed_ms, raises, events;
};

struct apple_pmp_cpu_vote {
	struct device *dev;
	void __iomem *events, *request;
	struct cpm_cluster cpm[2];
	/* Serializes samples, release, statistics and teardown. */
	struct mutex lock;
	struct delayed_work release_work;
	struct cpu_sample *samples;
	cpumask_t p_cpus, e_cpus, sample_cpus;
	struct apple_pmp_vote_policy policy;
	struct apple_pmp_vote_run e_run;
	struct apple_pmp_vote_gpu_window gpu_window;
	struct vote_source_stats source[3];
	u64 p_hot[3], e_hot[3], gpu_hist[11], p_samples, e_samples;
	u64 gpu_hist_ms;
	struct hlist_node hotplug_node;
	int hotplug_state;
	u32 saved_enables[2], active_vote;
	u64 started_ns, changed_ns, on_ns, ticks, changes, cpm_deferred;
	unsigned int last_cpu, last_load, last_e_load;
	u64 e_sample_ms;
	bool e_sampling;
	bool active, stopping, instance_added, stats_created;
};

struct cpm_request {
	struct cpm_cluster *cluster;
	bool raised, restore;
};

/* Runs synchronously on an online CPU in the register's own cluster. */
static void cpu_vote_cpm_access(void *arg)
{
	struct cpm_request *request = arg;
	struct cpm_cluster *cluster = request->cluster;
	u64 old;
	bool raised;

	if (WARN_ON_ONCE(!cpumask_test_cpu(smp_processor_id(), &cluster->cpus)))
		return;
	old = readq(cluster->reg);
	if (!cluster->saved) {
		cluster->saved_bit = old & 1;
		cluster->saved = true;
	}
	raised = request->restore ? cluster->saved_bit : request->raised;
	writeq(apple_pmp_cpm_control(old, raised), cluster->reg);
	readq(cluster->reg);
	cluster->raised = raised;
	cluster->valid = !request->restore;
	if (request->restore)
		cluster->saved = false;
}

/* Caller holds vote->lock and protects cpu_online_mask against hotplug. */
static void cpu_vote_cpm_apply(struct apple_pmp_cpu_vote *vote, bool raised,
			       bool restore)
{
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(vote->cpm); i++) {
		struct cpm_cluster *cluster = &vote->cpm[i];
		struct cpm_request request = { cluster, raised, restore };
		int ret;

		if (!cluster->reg || (restore && !cluster->saved) ||
		    (!restore && cluster->valid && cluster->raised == raised))
			continue;
		ret = smp_call_function_any(&cluster->cpus, cpu_vote_cpm_access,
					    &request, 1);
		if (ret) {
			/* Retry an offline cluster after its next online event. */
			cluster->valid = false;
			dev_warn_ratelimited(
				vote->dev,
				"CPM cluster %u access deferred: %d\n", i, ret);
		}
	}
}

/* Caller holds vote->lock. Only bit0 is owned; bits27:20 are status. */
static void cpu_vote_cpm_write(struct apple_pmp_cpu_vote *vote, bool raised)
{
	if (!vote->cpm[0].reg || (!READ_ONCE(cpu_vote_cpm) &&
				  !vote->cpm[0].saved && !vote->cpm[1].saved))
		return;
	raised = raised && READ_ONCE(cpu_vote_cpm);
	if (vote->cpm[0].valid && vote->cpm[1].valid &&
	    vote->cpm[0].raised == raised && vote->cpm[1].raised == raised)
		return;
	/*
	 * The hotplug writer drains these workers. Never wait on it while holding
	 * vote->lock: defer the access instead, and retry at a later policy sample.
	 */
	if (!cpus_read_trylock()) {
		vote->cpm_deferred++;
		/* A release must finish even if every deferrable sampler goes idle. */
		if (!vote->stopping)
			mod_delayed_work(
				system_wq, &vote->release_work,
				msecs_to_jiffies(CPU_VOTE_INTERVAL_MS));
		return;
	}
	cpu_vote_cpm_apply(vote, raised, false);
	cpus_read_unlock();
}

/* Called with lock held, after PMP has completed its ready handshake. */
static void cpu_vote_write(struct apple_pmp_cpu_vote *vote, unsigned int value)
{
	u64 now;

	cpu_vote_cpm_write(vote, value != 0);
	if (value == vote->active_vote)
		return;
	now = ktime_get_ns();
	writel(value, vote->request);
	readl(vote->request);
	if (vote->active_vote)
		vote->on_ns += now - vote->changed_ns;
	vote->changed_ns = now;
	vote->active_vote = value;
	vote->changes++;
}

static u64 cpu_vote_idle_time(unsigned int cpu, u64 *wall)
{
#ifdef CONFIG_CPU_FREQ
	return get_cpu_idle_time(cpu, wall, 0);
#else
	*wall = 0;
	return 0;
#endif
}

/* Caller holds the policy mutex. Source holds overlap and must not be summed. */
static void cpu_vote_note_source(struct apple_pmp_cpu_vote *vote,
				 unsigned int index, u64 when_ms)
{
	struct vote_source_stats *s = &vote->source[index];

	if (when_ms < div_u64(vote->started_ns, NSEC_PER_MSEC) ||
	    (s->until_ms && when_ms + CPU_VOTE_HOLD_MS < s->until_ms))
		return;
	s->events++;
	if (!s->until_ms || when_ms > s->until_ms) {
		s->completed_ms += s->until_ms - s->start_ms;
		s->start_ms = when_ms;
		s->raises++;
	}
	s->until_ms = when_ms + CPU_VOTE_HOLD_MS;
}

/* One consistent snapshot, with no lock/allocation/MMIO in the GPU producer. */
static u64 cpu_vote_gpu_busy(u64 *now)
{
	u64 state;

	do {
		state = atomic64_read(&gpu_busy_clock);
		*now = ktime_get_ns();
	} while (state != atomic64_read(&gpu_busy_clock));
	if (state & 1)
		state += *now << 1;
	return state >> 1;
}

/* Called with lock held; a zero threshold preserves legacy event pulses. */
static unsigned int cpu_vote_gpu_input(struct apple_pmp_cpu_vote *vote, u64 *now)
{
	u64 ns, busy = cpu_vote_gpu_busy(&ns);
	u64 last = atomic64_read(&gpu_last_ms);
	unsigned int active = atomic_read(&gpu_flights);
	unsigned int threshold = READ_ONCE(cpu_vote_gpu_percent);
	unsigned int percent;

	*now = div_u64(ns, NSEC_PER_MSEC);
	percent = apple_pmp_vote_gpu_fraction(&vote->gpu_window, ns, busy,
					      READ_ONCE(cpu_vote_gpu_window_ms));
	if (*now - vote->gpu_hist_ms >= CPU_VOTE_INTERVAL_MS) {
		vote->gpu_hist_ms = *now;
		vote->gpu_hist[percent / 10]++;
	}
	if (!(READ_ONCE(cpu_vote_sources) & APPLE_PMP_VOTE_GPU))
		return 0;
	if (!threshold) {
		apple_pmp_vote_note_activity(&vote->policy, min(last, *now));
		if (last)
			cpu_vote_note_source(vote, 1, min(last, *now));
	} else {
		active = percent >= threshold;
	}
	if (active)
		cpu_vote_note_source(vote, 1, *now);
	return active ? APPLE_PMP_VOTE_GPU : 0;
}

/*
 * Read all E idle counters together, at most once per sample period. Idle
 * cores need neither a timer nor an IPI. Only a fresh aggregate contributes
 * activity; a cached busy result must not extend the shared hold forever.
 * The existing release work does not poll this input or wake idle CPUs.
 */
static unsigned int cpu_vote_e_input(struct apple_pmp_cpu_vote *vote, u64 now)
{
	bool fresh = !vote->e_sampling;
	unsigned int cpu, load = 0, threshold;

	if (!(READ_ONCE(cpu_vote_sources) & APPLE_PMP_VOTE_E_CPU)) {
		vote->e_sampling = false;
		vote->e_run = (struct apple_pmp_vote_run){0};
		vote->last_e_load = 0;
		return 0;
	}
	if (!fresh && now - vote->e_sample_ms < CPU_VOTE_INTERVAL_MS)
		return 0;
	vote->e_sample_ms = now;
	vote->e_sampling = true;
	for_each_cpu(cpu, &vote->e_cpus) {
		struct cpu_sample *sample = &vote->samples[cpu];
		u64 wall, idle;

		if (fresh || !sample->enabled)
			sample->e_time.valid = false;
		if (!sample->enabled)
			continue;
		idle = cpu_vote_idle_time(cpu, &wall);
		load += apple_pmp_vote_load(&sample->e_time, idle, wall);
	}
	vote->last_e_load = load;
	vote->e_samples++;
	threshold = READ_ONCE(cpu_vote_e_threshold);
	fresh = apple_pmp_vote_sustained(&vote->e_run, load >= threshold,
					 now, READ_ONCE(cpu_vote_e_samples));
	for (cpu = 0; cpu < ARRAY_SIZE(vote->e_hot); cpu++)
		vote->e_hot[cpu] += vote->e_run.count > cpu;
	if (fresh)
		cpu_vote_note_source(vote, 2, now);
	return fresh ? APPLE_PMP_VOTE_E_CPU : 0;
}

static void cpu_vote_release(struct work_struct *work)
{
	struct apple_pmp_cpu_vote *vote = container_of(to_delayed_work(work),
				struct apple_pmp_cpu_vote, release_work);
	u64 now;
	unsigned int next, inputs;

	mutex_lock(&vote->lock);
	inputs = cpu_vote_gpu_input(vote, &now);
	if (!vote->stopping) {
		next = apple_pmp_vote_decide(&vote->policy, now, inputs,
					     CPU_VOTE_HOLD_MS,
					     READ_ONCE(cpu_vote_value));
		cpu_vote_write(vote, next);
		if (next)
			mod_delayed_work(system_wq, &vote->release_work,
					 msecs_to_jiffies(CPU_VOTE_HOLD_MS -
					(now - vote->policy.last_busy_ms)));
	}
	mutex_unlock(&vote->lock);
}

static void cpu_vote_sample(struct work_struct *work)
{
	struct cpu_sample *sample = container_of(to_delayed_work(work),
					       struct cpu_sample, work);
	struct apple_pmp_cpu_vote *vote = sample->vote;
	u64 wall, idle = cpu_vote_idle_time(sample->cpu, &wall);
	unsigned int load = 0, next, inputs, i;
	u64 now;

	mutex_lock(&vote->lock);
	if (vote->stopping || !sample->enabled)
		goto unlock;
	if (sample->valid && wall > sample->wall && idle >= sample->idle) {
		u64 dt = wall - sample->wall;
		u64 di = min(idle - sample->idle, dt);

		load = div64_u64(100 * (dt - di), dt);
	}
	sample->wall = wall;
	sample->idle = idle;
	sample->valid = true;
	vote->last_cpu = sample->cpu;
	vote->last_load = load;
	vote->ticks++;
	inputs = cpu_vote_gpu_input(vote, &now);
	inputs |= cpu_vote_e_input(vote, now);
	if ((READ_ONCE(cpu_vote_sources) & APPLE_PMP_VOTE_CPU) &&
	    cpumask_test_cpu(sample->cpu, &vote->p_cpus)) {
		bool busy = apple_pmp_vote_sustained(&sample->p_run,
			apple_pmp_vote_cpu_input(load, CPU_VOTE_THRESHOLD),
			now, READ_ONCE(cpu_vote_p_samples));

		vote->p_samples++;
		for (i = 0; i < ARRAY_SIZE(vote->p_hot); i++)
			vote->p_hot[i] += sample->p_run.count > i;
		if (busy) {
			inputs |= APPLE_PMP_VOTE_CPU;
			cpu_vote_note_source(vote, 0, now);
		}
	}
	next = apple_pmp_vote_decide(&vote->policy, now, inputs,
				     CPU_VOTE_HOLD_MS, READ_ONCE(cpu_vote_value));
	cpu_vote_write(vote, next);
	if (next)
		mod_delayed_work(system_wq, &vote->release_work,
			msecs_to_jiffies(CPU_VOTE_HOLD_MS -
					(now - vote->policy.last_busy_ms)));
	/*
	 * Each core has its own deferrable timer. Sampling neither depends
	 * on another cluster waking nor periodically wakes an idle core.
	 */
	queue_delayed_work_on(sample->cpu, system_wq, &sample->work,
			      msecs_to_jiffies(CPU_VOTE_INTERVAL_MS));
unlock:
	mutex_unlock(&vote->lock);
}

static int cpu_vote_online(unsigned int cpu, struct hlist_node *node)
{
	struct apple_pmp_cpu_vote *vote =
		container_of(node, struct apple_pmp_cpu_vote, hotplug_node);
	struct cpu_sample *sample = &vote->samples[cpu];

	if (!cpumask_test_cpu(cpu, &vote->sample_cpus))
		return 0;
	mutex_lock(&vote->lock);
	sample->idle = cpu_vote_idle_time(cpu, &sample->wall);
	sample->valid = true;
	sample->enabled = true;
	sample->p_run = (struct apple_pmp_vote_run){0};
	if (cpumask_test_cpu(cpu, &vote->cpm[0].cpus))
		vote->cpm[0].valid = false;
	if (cpumask_test_cpu(cpu, &vote->cpm[1].cpus))
		vote->cpm[1].valid = false;
	queue_delayed_work_on(cpu, system_wq, &sample->work,
			      msecs_to_jiffies(CPU_VOTE_INTERVAL_MS));
	mutex_unlock(&vote->lock);
	return 0;
}

static int cpu_vote_offline(unsigned int cpu, struct hlist_node *node)
{
	struct apple_pmp_cpu_vote *vote =
		container_of(node, struct apple_pmp_cpu_vote, hotplug_node);
	struct cpu_sample *sample = &vote->samples[cpu];

	if (!cpumask_test_cpu(cpu, &vote->sample_cpus))
		return 0;
	mutex_lock(&vote->lock);
	sample->enabled = false;
	sample->valid = false;
	sample->e_time.valid = false;
	{
		unsigned int i;

		for (i = 0; i < ARRAY_SIZE(vote->cpm); i++) {
			struct cpm_cluster *cluster = &vote->cpm[i];
			cpumask_t online;
			struct cpm_request request = { cluster, false, true };

			cpumask_and(&online, &cluster->cpus, cpu_online_mask);
			if (cluster->saved && cpumask_test_cpu(cpu, &online) &&
			    cpumask_weight(&online) == 1)
				smp_call_function_single(
					cpu, cpu_vote_cpm_access, &request, 1);
			if (cpumask_test_cpu(cpu, &cluster->cpus))
				cluster->valid = false;
		}
	}
	mutex_unlock(&vote->lock);
	/*
	 * Sampling only tries the CPU read lock, so draining it here cannot
	 * deadlock with the CPU hotplug writer.
	 */
	cancel_delayed_work_sync(&sample->work);
	return 0;
}

static ssize_t cpu_vote_show(struct device *dev, struct device_attribute *attr,
			     char *buf)
{
	struct apple_pmp_cpu_vote *vote = apple_pmp_report_cpu_vote(dev);
	u64 now, on, gpu_now, gpu_busy;
	unsigned int i;
	ssize_t n;

	if (!vote)
		return -ENODEV;
	mutex_lock(&vote->lock);
	now = ktime_get_ns();
	on = vote->on_ns + (vote->active_vote ? now - vote->changed_ns : 0);
	n = sysfs_emit(
		buf,
		"active=%u vote=%#x readback=%#x last_cpu=%u load=%u samples=%llu changes=%llu on_ns=%llu total_ns=%llu value=%#x interval_ms=%u hold_ms=%u threshold=%u cpm=%u cpm_deferred=%llu e_load=%u e_threshold=%u sources=%#x gpu_flights=%d gpu_last_ms=%lld\n",
		vote->active, vote->active_vote,
		vote->active ? readl(vote->request) : 0, vote->last_cpu,
		vote->last_load, vote->ticks, vote->changes, on,
		now - vote->started_ns, READ_ONCE(cpu_vote_value),
		CPU_VOTE_INTERVAL_MS, CPU_VOTE_HOLD_MS, CPU_VOTE_THRESHOLD,
		READ_ONCE(cpu_vote_cpm) && !!vote->cpm[0].reg,
		vote->cpm_deferred, vote->last_e_load,
		READ_ONCE(cpu_vote_e_threshold), READ_ONCE(cpu_vote_sources),
		atomic_read(&gpu_flights),
		atomic64_read(&gpu_last_ms));
	/* Extend the existing attribute without changing its original counters. */
	n--; /* replace newline */
	for (i = 0; i < ARRAY_SIZE(vote->source); i++) {
		struct vote_source_stats *s = &vote->source[i];
		static const char * const name[] = { "p", "gpu", "e" };
		u64 ms = s->completed_ms;

		if (s->until_ms)
			ms += min(div_u64(now, NSEC_PER_MSEC), s->until_ms) -
				s->start_ms;
		n += sysfs_emit_at(buf, n,
			" %s_on_ns=%llu %s_raises=%llu %s_events=%llu",
			name[i], ms * NSEC_PER_MSEC, name[i], s->raises,
			name[i], s->events);
	}
	gpu_busy = cpu_vote_gpu_busy(&gpu_now);
	n += sysfs_emit_at(buf, n,
		" gpu_busy_ns=%llu gpu_percent=%u gpu_window_ms=%u gpu_threshold=%u p_required=%u e_required=%u p_samples=%llu e_samples=%llu",
		gpu_busy, vote->gpu_window.percent,
		READ_ONCE(cpu_vote_gpu_window_ms), READ_ONCE(cpu_vote_gpu_percent),
		READ_ONCE(cpu_vote_p_samples), READ_ONCE(cpu_vote_e_samples),
		vote->p_samples, vote->e_samples);
	for (i = 0; i < ARRAY_SIZE(vote->p_hot); i++)
		n += sysfs_emit_at(buf, n, " p_hot%u=%llu e_hot%u=%llu",
				i + 1, vote->p_hot[i], i + 1, vote->e_hot[i]);
	for (i = 0; i < ARRAY_SIZE(vote->gpu_hist); i++)
		n += sysfs_emit_at(buf, n, " gpu_hist%u=%llu", i,
				vote->gpu_hist[i]);
	n += sysfs_emit_at(buf, n, "\n");
	mutex_unlock(&vote->lock);
	return n;
}
static DEVICE_ATTR_RO(cpu_vote);

struct apple_pmp_cpu_vote *
apple_pmp_cpu_vote_probe(struct platform_device *pdev, void __iomem *events)
{
	struct apple_pmp_cpu_vote *vote;
	bool cpm_e, cpm_p;
	int cpu;

	if (!cpu_vote_enabled || !events ||
	    !of_device_is_compatible(pdev->dev.of_node,
				     "apple,t8132-pmp-v2-report") ||
	    !platform_get_resource_byname(pdev, IORESOURCE_MEM, "perf-vote"))
		return NULL;
	vote = devm_kzalloc(&pdev->dev, sizeof(*vote), GFP_KERNEL);
	if (!vote)
		return ERR_PTR(-ENOMEM);
	vote->samples = devm_kcalloc(&pdev->dev, nr_cpu_ids,
				     sizeof(*vote->samples), GFP_KERNEL);
	if (!vote->samples)
		return ERR_PTR(-ENOMEM);
	vote->request =
		devm_platform_ioremap_resource_byname(pdev, "perf-vote");
	if (IS_ERR(vote->request))
		return ERR_CAST(vote->request);
	/* Old DTBs retain the v13 policy without the optional CPM control. */
	cpm_e = !!platform_get_resource_byname(pdev, IORESOURCE_MEM, "cpm-e");
	cpm_p = !!platform_get_resource_byname(pdev, IORESOURCE_MEM, "cpm-p");
	if (cpm_e != cpm_p)
		return ERR_PTR(-EINVAL);
	if (cpm_e) {
		vote->cpm[0].reg =
			devm_platform_ioremap_resource_byname(pdev, "cpm-e");
		if (IS_ERR(vote->cpm[0].reg))
			return ERR_CAST(vote->cpm[0].reg);
		vote->cpm[1].reg =
			devm_platform_ioremap_resource_byname(pdev, "cpm-p");
		if (IS_ERR(vote->cpm[1].reg))
			return ERR_CAST(vote->cpm[1].reg);
	}
	vote->dev = &pdev->dev;
	vote->events = events;
	vote->hotplug_state = -1;
	mutex_init(&vote->lock);
	INIT_DELAYED_WORK(&vote->release_work, cpu_vote_release);
	for_each_possible_cpu(cpu) {
		struct device_node *np = of_cpu_device_node_get(cpu);
		struct cpu_sample *sample = &vote->samples[cpu];

		if (of_device_is_compatible(np, "apple,donan-p")) {
			cpumask_set_cpu(cpu, &vote->p_cpus);
			cpumask_set_cpu(cpu, &vote->cpm[1].cpus);
		}
		if (of_device_is_compatible(np, "apple,donan-e")) {
			cpumask_set_cpu(cpu, &vote->e_cpus);
			cpumask_set_cpu(cpu, &vote->cpm[0].cpus);
		}
		if (of_device_is_compatible(np, "apple,donan-p") ||
		    of_device_is_compatible(np, "apple,donan-e")) {
			cpumask_set_cpu(cpu, &vote->sample_cpus);
			sample->cpu = cpu;
			sample->vote = vote;
			INIT_DEFERRABLE_WORK(&sample->work, cpu_vote_sample);
		}
		of_node_put(np);
	}
	if (cpumask_empty(&vote->p_cpus) ||
	    (vote->cpm[0].reg && cpumask_empty(&vote->cpm[0].cpus)))
		return ERR_PTR(-ENODEV);
	return vote;
}

void apple_pmp_cpu_vote_start(struct apple_pmp_cpu_vote *vote)
{
	unsigned int i;
	int ret;

	if (!vote || vote->active)
		return;
	ret = cpuhp_setup_state_multi(CPUHP_AP_ONLINE_DYN,
				      "pmdomain/apple-pmp:online",
				      cpu_vote_online, cpu_vote_offline);
	if (ret < 0) {
		dev_err(vote->dev, "CPU vote hotplug setup failed: %d\n", ret);
		return;
	}
	vote->hotplug_state = ret;
	writel(0, vote->request);
	readl(vote->request);
	for (i = 0; i < 2; i++) {
		u32 old = readl(vote->events + 4 * i);

		vote->saved_enables[i] = old & BIT(1);
		writel(old | BIT(1), vote->events + 4 * i);
		readl(vote->events + 4 * i);
	}
	cpu_vote_cpm_write(vote, false);
	vote->started_ns = ktime_get_ns();
	vote->changed_ns = vote->started_ns;
	vote->active = true;
	ret = cpuhp_state_add_instance(vote->hotplug_state,
				       &vote->hotplug_node);
	if (ret) {
		dev_err(vote->dev, "CPU vote hotplug registration failed: %d\n",
			ret);
		apple_pmp_cpu_vote_stop(vote);
		return;
	}
	vote->instance_added = true;
	ret = device_create_file(vote->dev, &dev_attr_cpu_vote);
	if (ret)
		dev_warn(vote->dev, "CPU vote statistics unavailable: %d\n",
			 ret);
	else
		vote->stats_created = true;
	dev_info(
		vote->dev,
		"CPU/GPU memory/fabric vote enabled: sources=%#x value=%#x P=%*pbl period=%u ms threshold=%u%% E=%*pbl e_threshold=%u%% hold=%u ms\n",
		READ_ONCE(cpu_vote_sources), READ_ONCE(cpu_vote_value),
		cpumask_pr_args(&vote->p_cpus), CPU_VOTE_INTERVAL_MS,
		CPU_VOTE_THRESHOLD, cpumask_pr_args(&vote->e_cpus),
		READ_ONCE(cpu_vote_e_threshold), CPU_VOTE_HOLD_MS);
}

void apple_pmp_cpu_vote_stop(struct apple_pmp_cpu_vote *vote)
{
	unsigned int i;

	if (!vote)
		return;
	mutex_lock(&vote->lock);
	vote->stopping = true;
	mutex_unlock(&vote->lock);
	if (vote->stats_created) {
		device_remove_file(vote->dev, &dev_attr_cpu_vote);
		vote->stats_created = false;
	}
	if (vote->instance_added) {
		cpuhp_state_remove_instance(vote->hotplug_state,
					    &vote->hotplug_node);
		vote->instance_added = false;
	}
	if (vote->hotplug_state >= 0) {
		cpuhp_remove_multi_state(vote->hotplug_state);
		vote->hotplug_state = -1;
	}
	cancel_delayed_work_sync(&vote->release_work);
	/* All workers and our hotplug callbacks are gone before blocking. */
	cpus_read_lock();
	mutex_lock(&vote->lock);
	if (vote->active) {
		cpu_vote_write(vote, 0);
		writel(0, vote->request);
		readl(vote->request);
		for (i = 0; i < 2; i++) {
			u32 old = readl(vote->events + 4 * i);

			writel((old & ~BIT(1)) | vote->saved_enables[i],
			       vote->events + 4 * i);
			readl(vote->events + 4 * i);
		}
		cpu_vote_cpm_apply(vote, false, true);
		vote->active = false;
	}
	mutex_unlock(&vote->lock);
	cpus_read_unlock();
}
