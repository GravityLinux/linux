// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* T8132 CPU activity input to PMP's memory and fabric policy. */
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
#include <linux/workqueue.h>

#include "pmp-cpu-vote.h"
#include "pmp-vote-policy.h"

#define CPU_VOTE_INTERVAL_MS 20
#define CPU_VOTE_THRESHOLD 50
#define CPU_VOTE_HOLD_MS 80

static bool cpu_vote_enabled = true;
module_param_named(cpu_vote, cpu_vote_enabled, bool, 0444);
MODULE_PARM_DESC(cpu_vote, "Enable the T8132 CPU-load memory/fabric vote");

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
	unsigned int cpu;
	bool enabled, valid;
};

struct apple_pmp_cpu_vote {
	struct device *dev;
	void __iomem *events, *request;
	/* Serializes samples, release, statistics and teardown. */
	struct mutex lock;
	struct delayed_work release_work;
	struct cpu_sample *samples;
	cpumask_t p_cpus;
	struct apple_pmp_vote_policy policy;
	struct hlist_node hotplug_node;
	int hotplug_state;
	u32 saved_enables[2], active_vote;
	u64 started_ns, changed_ns, on_ns, ticks, changes;
	unsigned int last_cpu, last_load;
	bool active, stopping, instance_added, stats_created;
};

/* Called with lock held, after PMP has completed its ready handshake. */
static void cpu_vote_write(struct apple_pmp_cpu_vote *vote, unsigned int value)
{
	u64 now;

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

static void cpu_vote_release(struct work_struct *work)
{
	struct apple_pmp_cpu_vote *vote = container_of(to_delayed_work(work),
				struct apple_pmp_cpu_vote, release_work);
	u64 now;
	unsigned int next;

	mutex_lock(&vote->lock);
	now = ktime_to_ms(ktime_get());
	if (!vote->stopping) {
		next = apple_pmp_vote_decide(&vote->policy, now, 0,
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
	u64 wall, idle = get_cpu_idle_time(sample->cpu, &wall, 0);
	unsigned int load = 0, next;

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
	next = apple_pmp_vote_decide(&vote->policy, ktime_to_ms(ktime_get()),
		apple_pmp_vote_cpu_input(load, CPU_VOTE_THRESHOLD),
		CPU_VOTE_HOLD_MS, READ_ONCE(cpu_vote_value));
	cpu_vote_write(vote, next);
	if (load >= CPU_VOTE_THRESHOLD)
		mod_delayed_work(system_wq, &vote->release_work,
				 msecs_to_jiffies(CPU_VOTE_HOLD_MS));
	/*
	 * Each P-core has its own deferrable timer. Sampling neither depends
	 * on another cluster waking nor periodically wakes an idle core.
	 */
	queue_delayed_work_on(sample->cpu, system_wq, &sample->work,
			      msecs_to_jiffies(CPU_VOTE_INTERVAL_MS));
unlock:
	mutex_unlock(&vote->lock);
}

static int cpu_vote_online(unsigned int cpu, struct hlist_node *node)
{
	struct apple_pmp_cpu_vote *vote = container_of(node,
				struct apple_pmp_cpu_vote, hotplug_node);
	struct cpu_sample *sample = &vote->samples[cpu];

	if (!cpumask_test_cpu(cpu, &vote->p_cpus))
		return 0;
	mutex_lock(&vote->lock);
	sample->idle = get_cpu_idle_time(cpu, &sample->wall, 0);
	sample->valid = true;
	sample->enabled = true;
	queue_delayed_work_on(cpu, system_wq, &sample->work,
			      msecs_to_jiffies(CPU_VOTE_INTERVAL_MS));
	mutex_unlock(&vote->lock);
	return 0;
}

static int cpu_vote_offline(unsigned int cpu, struct hlist_node *node)
{
	struct apple_pmp_cpu_vote *vote = container_of(node,
				struct apple_pmp_cpu_vote, hotplug_node);
	struct cpu_sample *sample = &vote->samples[cpu];

	if (!cpumask_test_cpu(cpu, &vote->p_cpus))
		return 0;
	mutex_lock(&vote->lock);
	sample->enabled = false;
	sample->valid = false;
	mutex_unlock(&vote->lock);
	/*
	 * Sampling does not take cpus_read_lock(), so draining it here cannot
	 * deadlock with the CPU hotplug writer.
	 */
	cancel_delayed_work_sync(&sample->work);
	return 0;
}

static ssize_t cpu_vote_show(struct device *dev, struct device_attribute *attr,
			     char *buf)
{
	struct apple_pmp_cpu_vote *vote = apple_pmp_report_cpu_vote(dev);
	u64 now, on;
	ssize_t n;

	if (!vote)
		return -ENODEV;
	mutex_lock(&vote->lock);
	now = ktime_get_ns();
	on = vote->on_ns + (vote->active_vote ? now - vote->changed_ns : 0);
	n = sysfs_emit(buf,
		       "active=%u vote=%#x readback=%#x last_cpu=%u load=%u samples=%llu changes=%llu on_ns=%llu total_ns=%llu value=%#x interval_ms=%u hold_ms=%u threshold=%u\n",
		vote->active, vote->active_vote,
		vote->active ? readl(vote->request) : 0,
		vote->last_cpu, vote->last_load, vote->ticks, vote->changes,
		on, now - vote->started_ns, READ_ONCE(cpu_vote_value),
		CPU_VOTE_INTERVAL_MS, CPU_VOTE_HOLD_MS, CPU_VOTE_THRESHOLD);
	mutex_unlock(&vote->lock);
	return n;
}
static DEVICE_ATTR_RO(cpu_vote);

struct apple_pmp_cpu_vote *
apple_pmp_cpu_vote_probe(struct platform_device *pdev, void __iomem *events)
{
	struct apple_pmp_cpu_vote *vote;
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
	vote->request = devm_platform_ioremap_resource_byname(pdev, "perf-vote");
	if (IS_ERR(vote->request))
		return ERR_CAST(vote->request);
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
			sample->cpu = cpu;
			sample->vote = vote;
			INIT_DEFERRABLE_WORK(&sample->work, cpu_vote_sample);
		}
		of_node_put(np);
	}
	if (cpumask_empty(&vote->p_cpus))
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
				      "pmdomain/apple-pmp:online", cpu_vote_online, cpu_vote_offline);
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
	vote->started_ns = ktime_get_ns();
	vote->changed_ns = vote->started_ns;
	vote->active = true;
	ret = cpuhp_state_add_instance(vote->hotplug_state, &vote->hotplug_node);
	if (ret) {
		dev_err(vote->dev, "CPU vote hotplug registration failed: %d\n", ret);
		apple_pmp_cpu_vote_stop(vote);
		return;
	}
	vote->instance_added = true;
	ret = device_create_file(vote->dev, &dev_attr_cpu_vote);
	if (ret)
		dev_warn(vote->dev, "CPU vote statistics unavailable: %d\n", ret);
	else
		vote->stats_created = true;
	dev_info(vote->dev,
		 "CPU memory/fabric vote enabled: value=%#x P=%*pbl period=%u ms threshold=%u%% hold=%u ms\n",
		 READ_ONCE(cpu_vote_value), cpumask_pr_args(&vote->p_cpus),
		 CPU_VOTE_INTERVAL_MS, CPU_VOTE_THRESHOLD, CPU_VOTE_HOLD_MS);
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
		cpuhp_state_remove_instance(vote->hotplug_state, &vote->hotplug_node);
		vote->instance_added = false;
	}
	if (vote->hotplug_state >= 0) {
		cpuhp_remove_multi_state(vote->hotplug_state);
		vote->hotplug_state = -1;
	}
	cancel_delayed_work_sync(&vote->release_work);
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
		vote->active = false;
	}
	mutex_unlock(&vote->lock);
}
