// SPDX-License-Identifier: GPL-2.0-only
/* Linux PCI/DMA adapter for Rust firmware and control IPC protocols.
 * The experimental probes stop at ready4/ready2. They cannot start a radio.
 */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/firmware.h>
#include <linux/io.h>
#include <linux/interrupt.h>
#include <linux/iommu.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/pci.h>
#include <linux/pm_runtime.h>
#include <linux/sizes.h>
#include <linux/slab.h>
#include <linux/timekeeping.h>
#include <linux/workqueue.h>
#include "centauri_glue.h"

#define CENTAURI_BOOT_BAR_MIN_SIZE 0x9004
#define CENTAURI_CONTROL_BAR_MIN_SIZE 0x9080
#define CENTAURI_BOOT_RECORD_SIZE 32
#define CENTAURI_BOOT_IMAGE_LIMIT SZ_32M
#define CENTAURI_CONTROL_CONTEXT_SIZE SZ_16K
#define CENTAURI_CONTROL_ARENA_SIZE SZ_2M

struct centauri_boot {
	struct pci_dev *pdev;
	void __iomem *bar;
	void *image;
	dma_addr_t image_dma;
	size_t image_bytes;
	void *record;
	dma_addr_t record_dma;
	void *companion;
	dma_addr_t companion_dma;
	size_t companion_bytes;
	void *context;
	dma_addr_t context_dma;
	void *control_arena;
	dma_addr_t control_arena_dma;
	void *control_state;
	struct work_struct service_work;
	bool service_running;
	u64 serviced_events;
	int vectors;
	int requested_irqs;
};

/* Separate from devm probe storage: removing the bootstrap PCI device calls
 * ->remove() and releases all its DMA/IRQ resources before the port reset.
 */
struct centauri_transition {
	struct work_struct work;
	struct pci_dev *bootstrap;
	struct pci_dev *root;
};

static struct pci_driver centauri_boot_driver;

/* The consumer owns the destination through the normal DMA API. The device
 * link orders supplier removal behind consumer teardown; its device lock
 * protects the source against explicit unbind while copying. Firmware output
 * is copied opaquely, with only its external FTAB directory parsed by Rust.
 */
int centauri_control_copy_radio(struct pci_dev *consumer, void **data,
				dma_addr_t *dma, size_t *bytes)
{
	struct device_node *node;
	struct device_link *link;
	struct centauri_boot *boot;
	struct pci_dev *supplier;
	size_t offset, length;
	int error;

	supplier = pci_get_slot(consumer->bus,
				PCI_DEVFN(PCI_SLOT(consumer->devfn), 0));
	if (!supplier)
		return -EPROBE_DEFER;
	node = of_parse_phandle(consumer->dev.of_node, "apple,control", 0);
	if (!node || node != supplier->dev.of_node) {
		error = -EINVAL;
		goto put_node;
	}
	link = device_link_add(&consumer->dev, &supplier->dev,
			       DL_FLAG_AUTOREMOVE_CONSUMER);
	if (!link) {
		error = -ENOMEM;
		goto put_node;
	}
	device_lock(&supplier->dev);
	boot = pci_get_drvdata(supplier);
	if (supplier->dev.driver != &centauri_boot_driver.driver || !boot ||
	    supplier->device != 0x1901) {
		error = -EPROBE_DEFER;
		goto unlock;
	}
	/* Acquire the successful startup's published DMA/service owner. */
	if (!smp_load_acquire(&boot->service_running)) {
		error = -EPROBE_DEFER;
		goto unlock;
	}
	error = centauri_bootstrap_radio_layout(boot->image, boot->image_bytes,
						PCI_FUNC(consumer->devfn),
						&offset, &length);
	if (error)
		goto unlock;
	if (offset > boot->companion_bytes ||
	    length > boot->companion_bytes - offset) {
		error = -EINVAL;
		goto unlock;
	}
	*data = dma_alloc_coherent(&consumer->dev, length, dma, GFP_KERNEL);
	if (!*data) {
		error = -ENOMEM;
		goto unlock;
	}
	/* Runtime ready2 publishes these opaque output bytes. The destination
	 * has not yet been published to its separate radio function/domain.
	 */
	dma_rmb();
	memcpy(*data, boot->companion + offset, length);
	*bytes = length;
unlock:
	device_unlock(&supplier->dev);
put_node:
	of_node_put(node);
	pci_dev_put(supplier);
	return error;
}

static void centauri_transition_work(struct work_struct *work)
{
	struct centauri_transition *transition =
		container_of(work, struct centauri_transition, work);
	struct pci_dev *pdev = transition->bootstrap;
	struct pci_dev *root = transition->root;
	struct pci_dev *runtime;
	u16 pm;
	int error;

	error = pm_runtime_resume_and_get(&root->dev);
	if (error < 0)
		goto release;
	pci_lock_rescan_remove();
	/* Resetting a port with another live endpoint is not qualified. The
	 * bootstrap function must be the only registered device on this bus.
	 */
	if (!device_is_registered(&pdev->dev) ||
	    !list_is_singular(&pdev->bus->devices) ||
	    pci_dev_is_disconnected(root)) {
		error = -EBUSY;
		goto unlock;
	}
	pci_stop_and_remove_bus_device(pdev);
	/* Linux removal clears PME enable (205/216). Request D3hot+PME only
	 * after the old driver has drained/freed its DMA and vectors. This is
	 * a short firmware transition, not a system wake request/PME poller.
	 */
	if (!pci_pme_capable(pdev, PCI_D3hot)) {
		error = -EOPNOTSUPP;
		goto unlock;
	}
	error = pci_read_config_word(pdev, pdev->pm_cap + PCI_PM_CTRL, &pm);
	if (error)
		goto unlock;
	pm = (pm & ~PCI_PM_CTRL_PME_STATUS) | PCI_PM_CTRL_PME_ENABLE;
	error = pci_write_config_word(pdev, pdev->pm_cap + PCI_PM_CTRL, pm);
	if (error)
		goto unlock;
	error = pci_set_power_state(pdev, PCI_D3hot);
	if (error)
		goto unlock;
	error = pci_read_config_word(pdev, pdev->pm_cap + PCI_PM_CTRL, &pm);
	if (error)
		goto unlock;
	if ((pm & (PCI_PM_CTRL_STATE_MASK | PCI_PM_CTRL_PME_ENABLE)) !=
	    (PCI_D3hot | PCI_PM_CTRL_PME_ENABLE)) {
		error = -EIO;
		goto unlock;
	}
	usleep_range(10000, 11000);
	/* The retained reference keeps pdev and its empty bus alive. With no
	 * registered children, pci_reset_bus does not restore the old endpoint
	 * personality or re-enable its freed DMA/MSI state. The core invokes
	 * the host reset_root_port callback and restores the saved bridge.
	 */
	pci_dev_lock(root);
	error = pci_save_state(root);
	pci_dev_unlock(root);
	if (error)
		goto unlock;
	error = pci_reset_bus(pdev);
	if (error)
		goto unlock;
	pci_rescan_bus(pdev->bus);
	runtime = pci_get_slot(pdev->bus, pdev->devfn);
	if (!runtime || runtime->vendor != PCI_VENDOR_ID_APPLE ||
	    runtime->device != 0x1901) {
		error = -ENODEV;
	} else {
		dev_info(&root->dev, "Centauri runtime function enumerated\n");
	}
	pci_dev_put(runtime);
unlock:
	pci_unlock_rescan_remove();
	pm_runtime_put(&root->dev);
release:
	if (error)
		dev_err(&root->dev, "Centauri firmware transition failed: %d\n", error);
	pci_dev_put(pdev);
	pci_dev_put(root);
	kfree(transition);
	module_put(THIS_MODULE);
}

static int centauri_schedule_transition(struct pci_dev *pdev)
{
	struct centauri_transition *transition;

	transition = kzalloc_obj(*transition);
	if (!transition)
		return -ENOMEM;
	if (!try_module_get(THIS_MODULE)) {
		kfree(transition);
		return -ENODEV;
	}
	transition->bootstrap = pci_dev_get(pdev);
	transition->root = pci_dev_get(pdev->bus->self);
	INIT_WORK(&transition->work, centauri_transition_work);
	queue_work(system_long_wq, &transition->work);
	return 0;
}

static void centauri_control_work(struct work_struct *work)
{
	struct centauri_boot *boot =
		container_of(work, struct centauri_boot, service_work);
	int received;

	if (!smp_load_acquire(&boot->service_running))
		return;
	received = centauri_control_service_poll(boot->control_state);
	if (received < 0) {
		smp_store_release(&boot->service_running, false);
		pci_clear_master(boot->pdev);
		dev_err(&boot->pdev->dev, "shared queue service stopped: %d\n",
			received);
		return;
	}
	if (received && !boot->serviced_events)
		dev_info(&boot->pdev->dev, "shared receive queues serviced\n");
	boot->serviced_events += received;
}

/* Startup is polled. After publication, MSI schedules serialized Rust queue
 * service; the handler never touches DMA memory or races a control submission.
 */
static irqreturn_t centauri_boot_irq(int irq, void *data)
{
	struct centauri_boot *boot = data;

	/* Acquire the fully initialized Rust owner before scheduling service. */
	if (smp_load_acquire(&boot->service_running))
		queue_work(system_dfl_wq, &boot->service_work);
	return IRQ_HANDLED;
}

int centauri_boot_enable_irqs(void *owner, unsigned int count)
{
	struct centauri_boot *boot = owner;
	int error, i;

	if (boot->vectors)
		return -EBUSY;
	error = pci_alloc_irq_vectors(boot->pdev, count, count, PCI_IRQ_MSI);
	if (error < 0)
		return error;
	boot->vectors = error;
	for (i = 0; i < boot->vectors; i++) {
		error = request_irq(pci_irq_vector(boot->pdev, i),
				    centauri_boot_irq, 0, "centauri-bootstrap",
				    boot);
		if (error)
			return error;
		boot->requested_irqs++;
	}
	return 0;
}

u32 centauri_boot_read(void *bar, size_t offset)
{
	return readl((void __iomem *)bar + offset);
}

void centauri_boot_write(void *bar, size_t offset, u32 value)
{
	writel(value, (void __iomem *)bar + offset);
}

void centauri_boot_publish_dma(void *owner)
{
	struct centauri_boot *boot = owner;

	dma_wmb();
	pci_set_master(boot->pdev);
}

void centauri_control_dma_wmb(void)
{
	dma_wmb();
}

void centauri_control_dma_rmb(void)
{
	dma_rmb();
}

u64 centauri_boot_time_us(void)
{
	return ktime_get_real_ns() / NSEC_PER_USEC;
}

/* Generic peer-function configuration access. Protocol offsets, identities
 * and values belong to Rust; C only resolves/refcounts the PCI device.
 */
static int centauri_boot_config(void *owner, u8 function, u16 offset,
				u32 *value, bool write)
{
	struct centauri_boot *boot = owner;
	struct pci_dev *peer;
	int error;

	if (function > 7 || offset > PCI_CFG_SPACE_EXP_SIZE - sizeof(*value) ||
	    offset & 3)
		return -EINVAL;
	peer = pci_get_slot(boot->pdev->bus,
			    PCI_DEVFN(PCI_SLOT(boot->pdev->devfn), function));
	if (!peer)
		return -EPROBE_DEFER;
	if (write)
		error = pci_write_config_dword(peer, offset, *value);
	else
		error = pci_read_config_dword(peer, offset, value);
	pci_dev_put(peer);
	return pcibios_err_to_errno(error);
}

int centauri_boot_config_read(void *owner, u8 function, u16 offset, u32 *value)
{
	return centauri_boot_config(owner, function, offset, value, false);
}

int centauri_boot_config_write(void *owner, u8 function, u16 offset, u32 value)
{
	return centauri_boot_config(owner, function, offset, &value, true);
}

void centauri_boot_sleep(void)
{
	usleep_range(5000, 6000);
}

static void centauri_boot_free(struct centauri_boot *boot)
{
	/* The hardware must not retain access to pages returned to the allocator.
	 * Do not use devm DMA allocation: teardown ordering is explicit here.
	 */
	smp_store_release(&boot->service_running, false);
	pci_clear_master(boot->pdev);
	pci_wait_for_pending_transaction(boot->pdev);
	for (int i = 0; i < boot->requested_irqs; i++)
		free_irq(pci_irq_vector(boot->pdev, i), boot);
	/* IRQs can no longer queue work. Drain before returning Rust bookkeeping
	 * or its underlying coherent buffers, including on partial probe failure.
	 */
	cancel_work_sync(&boot->service_work);
	if (boot->control_state) {
		centauri_control_service_destroy(boot->control_state);
		dev_info(&boot->pdev->dev, "shared events serviced: %llu\n",
			 boot->serviced_events);
	}
	if (boot->vectors)
		pci_free_irq_vectors(boot->pdev);
	if (boot->context)
		dma_free_coherent(&boot->pdev->dev, CENTAURI_CONTROL_CONTEXT_SIZE,
				  boot->context, boot->context_dma);
	if (boot->control_arena)
		dma_free_coherent(&boot->pdev->dev, CENTAURI_CONTROL_ARENA_SIZE,
				  boot->control_arena, boot->control_arena_dma);
	if (boot->record)
		dma_free_coherent(&boot->pdev->dev, CENTAURI_BOOT_RECORD_SIZE,
				  boot->record, boot->record_dma);
	if (boot->companion)
		dma_free_coherent(&boot->pdev->dev, boot->companion_bytes,
				  boot->companion, boot->companion_dma);
	if (boot->image)
		dma_free_coherent(&boot->pdev->dev, boot->image_bytes,
				  boot->image, boot->image_dma);
}

static int centauri_boot_probe(struct pci_dev *pdev,
			       const struct pci_device_id *id)
{
	const struct firmware *firmware;
	struct iommu_domain *domain;
	struct centauri_boot *boot;
	const char *name;
	u32 platform = 0;
	bool runtime = pdev->device == 0x1901;
	int error;

	/* Cold and warm function identities are distinct. Require explicit DT
	 * opt-in for this incomplete first-stage probe; no model or serial gate.
	 * These two bring-up properties are not a finalized upstream binding.
	 */
	if (!of_device_is_available(pdev->dev.of_node) ||
	    !of_device_is_compatible(pdev->dev.of_node,
				     "apple,t2026-centauri-control"))
		return -ENODEV;
	if (!of_property_present(pdev->dev.of_node, "firmware-name"))
		return -ENODEV;
	if (!runtime && (!pdev->bus->self ||
			 !pci_find_host_bridge(pdev->bus)->reset_root_port))
		return dev_err_probe(&pdev->dev, -EOPNOTSUPP,
				     "bootstrap requires a host port reset callback\n");
	error = of_property_read_string(pdev->dev.of_node, "firmware-name",
					&name);
	if (error)
		return error;
	if (!runtime) {
		error = of_property_read_u32(pdev->dev.of_node,
					     "apple,host-platform-id", &platform);
		if (error || !platform)
			return -EINVAL;
	}
	if (!(pci_resource_flags(pdev, 0) & IORESOURCE_MEM) ||
	    pci_resource_len(pdev, 0) < (runtime ?
						 CENTAURI_CONTROL_BAR_MIN_SIZE :
						 CENTAURI_BOOT_BAR_MIN_SIZE))
		return -EINVAL;
	/* A disabled firmware-DART node from the warm transport is insufficient.
	 * The PCI host must describe normal Linux-owned DMA translation first.
	 */
	domain = iommu_get_domain_for_dev(&pdev->dev);
	if (!domain || !iommu_is_dma_domain(domain))
		return dev_err_probe(&pdev->dev, -EOPNOTSUPP,
				     "bootstrap requires a Linux DMA IOMMU domain\n");
	error = pcim_enable_device(pdev);
	if (error)
		return error;
	/* A fresh bootstrap function has no prior DMA owner. Never adopt its
	 * previous pointers or a firmware-owned translation domain.
	 */
	pci_clear_master(pdev);
	error = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(36));
	if (error)
		return error;
	boot = devm_kzalloc(&pdev->dev, sizeof(*boot), GFP_KERNEL);
	if (!boot)
		return -ENOMEM;
	boot->pdev = pdev;
	INIT_WORK(&boot->service_work, centauri_control_work);
	boot->bar = pcim_iomap_region(pdev, 0, KBUILD_MODNAME);
	if (IS_ERR(boot->bar))
		return PTR_ERR(boot->bar);
	error = request_firmware(&firmware, name, &pdev->dev);
	if (error)
		return error;
	if (!firmware->size || firmware->size > CENTAURI_BOOT_IMAGE_LIMIT) {
		error = -EINVAL;
		goto release;
	}
	boot->image_bytes = firmware->size;
	boot->image = dma_alloc_coherent(&pdev->dev, boot->image_bytes,
					 &boot->image_dma, GFP_KERNEL);
	if (!boot->image) {
		error = -ENOMEM;
		goto release;
	}
	memcpy(boot->image, firmware->data, boot->image_bytes);
	error = centauri_bootstrap_layout(boot->image, boot->image_bytes,
					  &boot->companion_bytes);
	if (error)
		goto release;
	if (runtime) {
		boot->companion =
			dma_alloc_coherent(&pdev->dev, boot->companion_bytes,
					   &boot->companion_dma, GFP_KERNEL);
		if (!boot->companion) {
			error = -ENOMEM;
			goto release;
		}
		error = centauri_bootstrap_allocation(boot->image,
						      boot->image_bytes,
						      boot->companion,
						      boot->companion_bytes);
		if (error)
			goto release;
	}
	boot->record = dma_alloc_coherent(&pdev->dev, CENTAURI_BOOT_RECORD_SIZE,
					  &boot->record_dma, GFP_KERNEL);
	if (!boot->record) {
		error = -ENOMEM;
		goto release;
	}
	/* Rust validates the container before publishing any pointer/doorbell.
	 * All addresses are returned by the DMA API, never fixed reference IOVAs.
	 */
	error = centauri_bootstrap_image(boot, (void __force *)boot->bar,
					 boot->image, boot->image_bytes,
					 boot->image_dma, boot->record,
					 boot->record_dma, platform, runtime,
					 boot->companion_dma, boot->companion_bytes);
	if (!error && runtime) {
		boot->context = dma_alloc_coherent(&pdev->dev,
						   CENTAURI_CONTROL_CONTEXT_SIZE,
						   &boot->context_dma, GFP_KERNEL);
		boot->control_arena = dma_alloc_coherent(&pdev->dev,
							 CENTAURI_CONTROL_ARENA_SIZE,
							 &boot->control_arena_dma,
							 GFP_KERNEL);
		if (!boot->context || !boot->control_arena)
			error = -ENOMEM;
		else
			error = centauri_control_start(boot,
						       (void __force *)boot->bar,
						       boot->context,
						       CENTAURI_CONTROL_CONTEXT_SIZE,
						       boot->context_dma,
						       boot->control_arena,
						       CENTAURI_CONTROL_ARENA_SIZE,
						       boot->control_arena_dma);
		if (!error) {
			boot->control_state =
				centauri_control_service_create((void __force *)boot->bar,
								boot->context, boot->context_dma,
								boot->control_arena,
								boot->control_arena_dma);
			if (!boot->control_state)
				error = -ENOMEM;
		}
	}
release:
	release_firmware(firmware);
	if (error) {
		dev_err(&pdev->dev, "firmware/control startup failed: %d\n",
			error);
		centauri_boot_free(boot);
		return error;
	}
	if (!runtime) {
		pci_clear_master(pdev);
		pci_wait_for_pending_transaction(pdev);
	}
	pci_set_drvdata(pdev, boot);
	if (runtime) {
		/* Publish the initialized owner to IRQ/work consumers. */
		smp_store_release(&boot->service_running, true);
		/* Startup completions may predate the service owner. Drain once even
		 * if their MSI has already been acknowledged during polled setup.
		 */
		queue_work(system_dfl_wq, &boot->service_work);
	}
	if (!runtime) {
		error = centauri_schedule_transition(pdev);
		if (error) {
			pci_set_drvdata(pdev, NULL);
			centauri_boot_free(boot);
			return error;
		}
	}
	if (runtime)
		dev_info(&pdev->dev,
			 "shared image and control IPC ready2; radio initialization pending\n");
	else
		dev_info(&pdev->dev, "initial image ready4; port restart scheduled\n");
	return 0;
}

static void centauri_boot_remove(struct pci_dev *pdev)
{
	centauri_boot_free(pci_get_drvdata(pdev));
}

static const struct pci_device_id centauri_boot_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_APPLE, 0x1900) },
	{ PCI_DEVICE(PCI_VENDOR_ID_APPLE, 0x1901) },
	{}
};
MODULE_DEVICE_TABLE(pci, centauri_boot_ids);

static struct pci_driver centauri_boot_driver = {
	.name = "centauri-bootstrap",
	.id_table = centauri_boot_ids,
	.probe = centauri_boot_probe,
	.remove = centauri_boot_remove,
};

int centauri_bootstrap_register(void)
{
	return pci_register_driver(&centauri_boot_driver);
}

void centauri_bootstrap_unregister(void)
{
	pci_unregister_driver(&centauri_boot_driver);
}
