// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple DockChannel UART driver
 * Copyright The Asahi Linux Contributors
 */

#include <linux/console.h>
#include <linux/io.h>
#include <linux/serial_core.h>

#define DOCKCHANNEL_TX8      0x04
#define DOCKCHANNEL_TX_FREE  0x14

static void dockchannel_uart_putc(struct uart_port *port, unsigned char c)
{
	while (readl(port->membase + DOCKCHANNEL_TX_FREE) == 0)
		cpu_relax();
	writel(c, port->membase + DOCKCHANNEL_TX8);
}

static void dockchannel_uart_early_write(struct console *con, const char *s,
					 unsigned int n)
{
	struct earlycon_device *dev = con->data;

	uart_console_write(&dev->port, s, n, dockchannel_uart_putc);
}

static int __init dockchannel_uart_early_setup(struct earlycon_device *device,
					       const char *opt)
{
	struct uart_port *port = &device->port;

	unsigned long mapbase = port->mapbase;
	if (!mapbase)
		return -ENODEV;

	device->con->write = dockchannel_uart_early_write;
	return 0;
}

OF_EARLYCON_DECLARE(dockchannel, "apple,dockchannel-uart",
		    dockchannel_uart_early_setup);
EARLYCON_DECLARE(dockchannel, dockchannel_uart_early_setup);
