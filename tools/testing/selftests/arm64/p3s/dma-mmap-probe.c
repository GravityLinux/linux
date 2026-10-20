// SPDX-License-Identifier: GPL-2.0-only
/* Optional scratch module: exercise the real dma_mmap_pages() entry point. */
#include <linux/dma-mapping.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mm.h>

static void *buffer;

static int probe_mmap(struct file *file, struct vm_area_struct *vma)
{
	return dma_mmap_pages(NULL, vma, SZ_64K, virt_to_page(buffer));
}

static const struct file_operations probe_fops = {
	.owner = THIS_MODULE,
	.mmap = probe_mmap,
};

static struct miscdevice probe_device = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "p3s-dma-mmap-probe",
	.fops = &probe_fops,
	.mode = 0600,
};

static int __init probe_init(void)
{
	int ret;

	buffer = alloc_pages_exact(SZ_64K, GFP_KERNEL | __GFP_ZERO);
	if (!buffer)
		return -ENOMEM;
	ret = misc_register(&probe_device);
	if (ret)
		free_pages_exact(buffer, SZ_64K);
	return ret;
}

static void __exit probe_exit(void)
{
	misc_deregister(&probe_device);
	free_pages_exact(buffer, SZ_64K);
}

module_init(probe_init);
module_exit(probe_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Scratch P3S DMA mmap bounds regression probe");
