// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple SoC Watchdog driver
 *
 * Copyright (C) The Asahi Linux Contributors
 */

#include <linux/bits.h>
#include <linux/clk.h>
#include <linux/delay.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/kernel.h>
#include <linux/limits.h>
#include <linux/math64.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/pm_wakeup.h>
#include <linux/rtc.h>
#include <linux/spinlock.h>
#include <linux/timekeeping.h>
#include <linux/watchdog.h>

/*
 * Apple Watchdog MMIO registers
 *
 * This HW block has three separate watchdogs. WD0 resets the machine
 * to recovery mode and is not very useful for us. WD1 and WD2 trigger a normal
 * machine reset. WD0 additionally supports a configurable interrupt.
 * WD0 is never armed to reset the machine here; its interrupt is used as a
 * wake alarm instead (see below).
 *
 * APPLE_WDT_WDx_CUR_TIME is a simple counter incremented for each tick of the
 * reference clock. It can also be overwritten to any value.
 * Whenever APPLE_WDT_CTRL_RESET_EN is set in APPLE_WDT_WDx_CTRL and
 * APPLE_WDT_WDx_CUR_TIME >= APPLE_WDT_WDx_BITE_TIME the entire machine is
 * reset.
 * Whenever APPLE_WDT_CTRL_IRQ_EN is set and APPLE_WDTx_WD1_CUR_TIME >=
 * APPLE_WDTx_WD1_BARK_TIME an interrupt is triggered and
 * APPLE_WDT_CTRL_IRQ_STATUS is set. The interrupt can be cleared by writing
 * 1 to APPLE_WDT_CTRL_IRQ_STATUS.
 */
#define APPLE_WDT_WD0_CUR_TIME		0x00
#define APPLE_WDT_WD0_BITE_TIME		0x04
#define APPLE_WDT_WD0_BARK_TIME		0x08
#define APPLE_WDT_WD0_CTRL		0x0c

#define APPLE_WDT_WD1_CUR_TIME		0x10
#define APPLE_WDT_WD1_BITE_TIME		0x14
#define APPLE_WDT_WD1_CTRL		0x1c

#define APPLE_WDT_WD2_CUR_TIME		0x20
#define APPLE_WDT_WD2_BITE_TIME		0x24
#define APPLE_WDT_WD2_CTRL		0x2c

#define APPLE_WDT_CTRL_IRQ_EN		BIT(0)
#define APPLE_WDT_CTRL_IRQ_STATUS	BIT(1)
#define APPLE_WDT_CTRL_RESET_EN		BIT(2)

#define APPLE_WDT_TIMEOUT_DEFAULT	30

struct apple_wdt {
	struct watchdog_device wdd;
	struct device *dev;
	void __iomem *regs;
	unsigned long clk_rate;

	/* WD0 interrupt as a wake alarm, exposed as an alarm-only RTC */
	struct rtc_device *rtc;
	spinlock_t alarm_lock;
	time64_t alarm_time;
	bool alarm_enabled;
};

static struct apple_wdt *to_apple_wdt(struct watchdog_device *wdd)
{
	return container_of(wdd, struct apple_wdt, wdd);
}

static int apple_wdt_start(struct watchdog_device *wdd)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);

	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_CUR_TIME);
	writel_relaxed(APPLE_WDT_CTRL_RESET_EN, wdt->regs + APPLE_WDT_WD1_CTRL);

	return 0;
}

static int apple_wdt_stop(struct watchdog_device *wdd)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);

	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_CTRL);

	return 0;
}

static int apple_wdt_ping(struct watchdog_device *wdd)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);

	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_CUR_TIME);

	return 0;
}

static int apple_wdt_set_timeout(struct watchdog_device *wdd, unsigned int s)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);
	u32 actual;

	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_CUR_TIME);

	actual = min(s, wdd->max_hw_heartbeat_ms / 1000);
	writel_relaxed(wdt->clk_rate * actual, wdt->regs + APPLE_WDT_WD1_BITE_TIME);

	wdd->timeout = s;

	return 0;
}

static unsigned int apple_wdt_get_timeleft(struct watchdog_device *wdd)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);
	u32 cur_time, reset_time;

	cur_time = readl_relaxed(wdt->regs + APPLE_WDT_WD1_CUR_TIME);
	reset_time = readl_relaxed(wdt->regs + APPLE_WDT_WD1_BITE_TIME);

	return (reset_time - cur_time) / wdt->clk_rate;
}

static int apple_wdt_restart(struct watchdog_device *wdd, unsigned long mode,
			     void *cmd)
{
	struct apple_wdt *wdt = to_apple_wdt(wdd);

	writel_relaxed(APPLE_WDT_CTRL_RESET_EN, wdt->regs + APPLE_WDT_WD1_CTRL);
	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_BITE_TIME);
	writel_relaxed(0, wdt->regs + APPLE_WDT_WD1_CUR_TIME);

	/*
	 * Flush writes and then wait for the SoC to reset. Even though the
	 * reset is queued almost immediately experiments have shown that it
	 * can take up to ~120-125ms until the SoC is actually reset. Just
	 * wait 150ms here to be safe.
	 */
	(void)readl(wdt->regs + APPLE_WDT_WD1_CUR_TIME);
	mdelay(150);

	return 0;
}

static const struct watchdog_ops apple_wdt_ops = {
	.owner = THIS_MODULE,
	.start = apple_wdt_start,
	.stop = apple_wdt_stop,
	.ping = apple_wdt_ping,
	.set_timeout = apple_wdt_set_timeout,
	.get_timeleft = apple_wdt_get_timeleft,
	.restart = apple_wdt_restart,
};

#if IS_ENABLED(CONFIG_RTC_CLASS)
/*
 * The block sits in the always-on domain and its counter keeps running in
 * suspend-to-idle, so WD0's interrupt can end a sleep at a given time. The
 * counter is 32 bits wide (about 179 s at 24 MHz): a later alarm is reached
 * in steps, and only the last step reports the alarm and wakes the system.
 * The alarm device reads the system's real time and has no time of its own
 * to set.
 */
static void apple_wdt_alarm_program(struct apple_wdt *wdt)
{
	s64 left_ns;
	u64 ticks;

	writel_relaxed(APPLE_WDT_CTRL_IRQ_STATUS, wdt->regs + APPLE_WDT_WD0_CTRL);
	if (!wdt->alarm_enabled)
		return;

	left_ns = wdt->alarm_time * NSEC_PER_SEC - ktime_get_real_ns();
	ticks = left_ns > 0 ? mul_u64_u32_div(left_ns, wdt->clk_rate, NSEC_PER_SEC) : 0;
	ticks = clamp_t(u64, ticks, 1, U32_MAX);

	writel_relaxed(0, wdt->regs + APPLE_WDT_WD0_CUR_TIME);
	writel_relaxed(ticks, wdt->regs + APPLE_WDT_WD0_BARK_TIME);
	writel(APPLE_WDT_CTRL_IRQ_EN | APPLE_WDT_CTRL_IRQ_STATUS,
	       wdt->regs + APPLE_WDT_WD0_CTRL);
}

static irqreturn_t apple_wdt_alarm_irq(int irq, void *data)
{
	struct apple_wdt *wdt = data;
	bool fired = false;

	spin_lock(&wdt->alarm_lock);
	if (!(readl_relaxed(wdt->regs + APPLE_WDT_WD0_CTRL) & APPLE_WDT_CTRL_IRQ_STATUS)) {
		spin_unlock(&wdt->alarm_lock);
		return IRQ_NONE;
	}
	if (wdt->alarm_enabled &&
	    ktime_get_real_ns() >= wdt->alarm_time * NSEC_PER_SEC) {
		wdt->alarm_enabled = false;
		fired = true;
	}
	apple_wdt_alarm_program(wdt);
	spin_unlock(&wdt->alarm_lock);

	if (fired) {
		rtc_update_irq(wdt->rtc, 1, RTC_AF | RTC_IRQF);
		pm_wakeup_hard_event(wdt->dev);
	}

	return IRQ_HANDLED;
}

static int apple_wdt_rtc_read_time(struct device *dev, struct rtc_time *tm)
{
	rtc_time64_to_tm(ktime_get_real_seconds(), tm);
	return 0;
}

static int apple_wdt_rtc_read_alarm(struct device *dev, struct rtc_wkalrm *alrm)
{
	struct apple_wdt *wdt = dev_get_drvdata(dev);
	unsigned long flags;

	spin_lock_irqsave(&wdt->alarm_lock, flags);
	rtc_time64_to_tm(wdt->alarm_time, &alrm->time);
	alrm->enabled = wdt->alarm_enabled;
	spin_unlock_irqrestore(&wdt->alarm_lock, flags);

	return 0;
}

static int apple_wdt_rtc_set_alarm(struct device *dev, struct rtc_wkalrm *alrm)
{
	struct apple_wdt *wdt = dev_get_drvdata(dev);
	unsigned long flags;

	spin_lock_irqsave(&wdt->alarm_lock, flags);
	wdt->alarm_time = rtc_tm_to_time64(&alrm->time);
	wdt->alarm_enabled = alrm->enabled;
	apple_wdt_alarm_program(wdt);
	spin_unlock_irqrestore(&wdt->alarm_lock, flags);

	return 0;
}

static int apple_wdt_rtc_alarm_irq_enable(struct device *dev, unsigned int enabled)
{
	struct apple_wdt *wdt = dev_get_drvdata(dev);
	unsigned long flags;

	spin_lock_irqsave(&wdt->alarm_lock, flags);
	wdt->alarm_enabled = enabled;
	apple_wdt_alarm_program(wdt);
	spin_unlock_irqrestore(&wdt->alarm_lock, flags);

	return 0;
}

static const struct rtc_class_ops apple_wdt_rtc_ops = {
	.read_time = apple_wdt_rtc_read_time,
	.read_alarm = apple_wdt_rtc_read_alarm,
	.set_alarm = apple_wdt_rtc_set_alarm,
	.alarm_irq_enable = apple_wdt_rtc_alarm_irq_enable,
};

static int apple_wdt_alarm_init(struct apple_wdt *wdt, struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	int irq, ret;

	/*
	 * Only with an rtc alias: the alarm device then gets a fixed number
	 * and never takes rtc0 from the RTC that holds the time of day.
	 */
	if (of_alias_get_id(dev->of_node, "rtc") < 0)
		return 0;

	irq = platform_get_irq_optional(pdev, 0);
	if (irq < 0)
		return 0;

	if (readl_relaxed(wdt->regs + APPLE_WDT_WD0_CTRL) & APPLE_WDT_CTRL_RESET_EN) {
		dev_info(dev, "WD0 armed to reset, no wake alarm\n");
		return 0;
	}

	spin_lock_init(&wdt->alarm_lock);
	writel(APPLE_WDT_CTRL_IRQ_STATUS, wdt->regs + APPLE_WDT_WD0_CTRL);

	wdt->rtc = devm_rtc_allocate_device(dev);
	if (IS_ERR(wdt->rtc))
		return PTR_ERR(wdt->rtc);
	wdt->rtc->ops = &apple_wdt_rtc_ops;
	/* Alarm times are compared in nanoseconds of CLOCK_REALTIME. */
	wdt->rtc->range_max = KTIME_SEC_MAX - 1;
	clear_bit(RTC_FEATURE_UPDATE_INTERRUPT, wdt->rtc->features);

	/*
	 * Kept enabled through suspend: the handler decides whether a step is
	 * the alarm and only then reports a wakeup event.
	 */
	ret = devm_request_irq(dev, irq, apple_wdt_alarm_irq, IRQF_NO_SUSPEND,
			       dev_name(dev), wdt);
	if (ret)
		return ret;

	ret = devm_device_init_wakeup(dev);
	if (ret)
		return ret;

	return devm_rtc_register_device(wdt->rtc);
}
#else
static int apple_wdt_alarm_init(struct apple_wdt *wdt, struct platform_device *pdev)
{
	return 0;
}
#endif

static const struct watchdog_info apple_wdt_info = {
	.identity = "Apple SoC Watchdog",
	.options = WDIOF_MAGICCLOSE | WDIOF_KEEPALIVEPING | WDIOF_SETTIMEOUT,
};

static int apple_wdt_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct apple_wdt *wdt;
	struct clk *clk;
	u32 wdt_ctrl;
	int ret;

	wdt = devm_kzalloc(dev, sizeof(*wdt), GFP_KERNEL);
	if (!wdt)
		return -ENOMEM;

	wdt->regs = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(wdt->regs))
		return PTR_ERR(wdt->regs);

	clk = devm_clk_get_enabled(dev, NULL);
	if (IS_ERR(clk))
		return PTR_ERR(clk);
	wdt->clk_rate = clk_get_rate(clk);
	if (!wdt->clk_rate)
		return -EINVAL;

	platform_set_drvdata(pdev, wdt);
	wdt->dev = dev;

	wdt->wdd.ops = &apple_wdt_ops;
	wdt->wdd.info = &apple_wdt_info;
	wdt->wdd.max_hw_heartbeat_ms = U32_MAX / wdt->clk_rate * 1000;
	wdt->wdd.timeout = APPLE_WDT_TIMEOUT_DEFAULT;

	wdt_ctrl = readl_relaxed(wdt->regs + APPLE_WDT_WD1_CTRL);
	if (wdt_ctrl & APPLE_WDT_CTRL_RESET_EN)
		set_bit(WDOG_HW_RUNNING, &wdt->wdd.status);

	watchdog_init_timeout(&wdt->wdd, 0, dev);
	apple_wdt_set_timeout(&wdt->wdd, wdt->wdd.timeout);
	watchdog_stop_on_unregister(&wdt->wdd);
	watchdog_set_restart_priority(&wdt->wdd, 128);

	ret = devm_watchdog_register_device(dev, &wdt->wdd);
	if (ret)
		return ret;

	ret = apple_wdt_alarm_init(wdt, pdev);
	if (ret)
		dev_warn(dev, "no wake alarm: %d\n", ret);

	return 0;
}

static int apple_wdt_resume(struct device *dev)
{
	struct apple_wdt *wdt = dev_get_drvdata(dev);

	if (watchdog_active(&wdt->wdd) || watchdog_hw_running(&wdt->wdd))
		apple_wdt_start(&wdt->wdd);

	return 0;
}

static int apple_wdt_suspend(struct device *dev)
{
	struct apple_wdt *wdt = dev_get_drvdata(dev);

	if (watchdog_active(&wdt->wdd) || watchdog_hw_running(&wdt->wdd))
		apple_wdt_stop(&wdt->wdd);

	return 0;
}

static DEFINE_SIMPLE_DEV_PM_OPS(apple_wdt_pm_ops, apple_wdt_suspend, apple_wdt_resume);

static const struct of_device_id apple_wdt_of_match[] = {
	{ .compatible = "apple,t8103-wdt" },
	{ .compatible = "apple,wdt" },
	{},
};
MODULE_DEVICE_TABLE(of, apple_wdt_of_match);

static struct platform_driver apple_wdt_driver = {
	.driver = {
		.name = "apple-watchdog",
		.of_match_table = apple_wdt_of_match,
		.pm = pm_sleep_ptr(&apple_wdt_pm_ops),
	},
	.probe = apple_wdt_probe,
};
module_platform_driver(apple_wdt_driver);

MODULE_DESCRIPTION("Apple SoC watchdog driver");
MODULE_AUTHOR("Sven Peter <sven@svenpeter.dev>");
MODULE_LICENSE("Dual MIT/GPL");
