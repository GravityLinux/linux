/* SPDX-License-Identifier: GPL-2.0-only */
/* PRIVATE: T8130 DP bring-up from native MMIO evidence. No sink configuration.
 * HBR2 is qualified; HBR is a lower-clock candidate pending live training.
 * RBR/HBR3 remain rejected. Do not publish as complete.
 */
#define T8130_DP_PLL_LOCK 0x7034
#define T8130_DP_BGR 0x2218
#define T8130_DP_CLK_POWER 0x0200
#define T8130_DP_TX_RAILS 0x010c
#define T8130_DP_RX_CLOCK 0x000c

static int t8130_dp_apb_request(struct apple_atcphy *atcphy, u32 command)
{
	u32 reg;
	int ret;

	core_mask32(atcphy, AUSPLL_APB_CMD_OVERRIDE,
		    AUSPLL_APB_CMD_OVERRIDE_CMD | AUSPLL_APB_CMD_OVERRIDE_REQ,
		    FIELD_PREP(AUSPLL_APB_CMD_OVERRIDE_CMD, command) |
		    AUSPLL_APB_CMD_OVERRIDE_REQ | AUSPLL_APB_CMD_OVERRIDE_UNK28);
	ret = readl_poll_timeout(atcphy->regs.core + AUSPLL_APB_CMD_OVERRIDE,
		reg, reg & AUSPLL_APB_CMD_OVERRIDE_ACK, 10, 10000);
	if (ret)
		dev_err(atcphy->dev, "T8130 DP APB command %#x ACK timeout (%#x)\n", command, reg);
	return ret;
}

static int t8130_dp_apb_release(struct apple_atcphy *atcphy, u32 command)
{
	u32 reg;
	int ret;

	core_mask32(atcphy, AUSPLL_APB_CMD_OVERRIDE,
		    AUSPLL_APB_CMD_OVERRIDE_CMD | AUSPLL_APB_CMD_OVERRIDE_REQ,
		    FIELD_PREP(AUSPLL_APB_CMD_OVERRIDE_CMD, command));
	ret = readl_poll_timeout(atcphy->regs.core + AUSPLL_APB_CMD_OVERRIDE,
		reg, !(reg & AUSPLL_APB_CMD_OVERRIDE_ACK), 10, 10000);
	if (ret)
		dev_err(atcphy->dev, "T8130 DP APB release %#x timeout (%#x)\n", command, reg);
	return ret;
}

static u32 t8130_dp_rx_top(unsigned int pma)
{
	return pma ? LN1_AUSPMA_RX_TOP : LN0_AUSPMA_RX_TOP;
}

static u32 t8130_dp_tx_top(unsigned int pma)
{
	return pma ? LN1_AUSPMA_TX_TOP : LN0_AUSPMA_TX_TOP;
}

static void t8130_dp_prepare_lane(struct apple_atcphy *atcphy, unsigned int pma)
{
	u32 rx = t8130_dp_rx_top(pma), tx = t8130_dp_tx_top(pma);
	u32 rs = rx + 0x2000, ts = tx + 0x1000;

	/* Assert byte clocks and analog resets before stopping AUSPLL. */
	core_set32(atcphy, ts + 0x1c, GENMASK(23, 22));
	core_set32(atcphy, ts + 0x10, BIT(10));
	core_set32(atcphy, ts + 0x04, BIT(29));
	core_clear32(atcphy, ts + 0x44, BIT(20) | BIT(22) | BIT(24) | BIT(26));
	core_clear32(atcphy, T8130_DP_TX_RAILS, BIT(pma) | BIT(pma + 4));
	core_set32(atcphy, T8130_DP_TX_RAILS, BIT(pma + 8));
	core_set32(atcphy, rs + 0x30, BIT(11));
	core_set32(atcphy, rs + 0xa8, GENMASK(23, 22));
	core_set32(atcphy, rs + 0xa8, BIT(18));
	core_set32(atcphy, rs + 0x68, BIT(6) | BIT(8));
	core_clear32(atcphy, rs + 0xd4, BIT(18));
	core_set32(atcphy, rs + 0x6c, GENMASK(10, 9));
	core_set32(atcphy, rx + 0x1d4, BIT(0));
	core_clear32(atcphy, rs + 0x6c, BIT(11));
	core_clear32(atcphy, ACIOPHY_CFG0, BIT(6 + pma) | BIT(10 + pma));
	core_set32(atcphy, ACIOPHY_CFG0, BIT(14 + pma));
}

static int t8130_dp_stop(struct apple_atcphy *atcphy)
{
	const struct atcphy_mode_configuration *mode;
	int ret;

	mode = atcphy_get_mode_config(atcphy, atcphy->mode);
	core_set32(atcphy, 0x7000, DP_PMA_BYTECLK_RESET);
	for (int pma = 0; pma < 2; pma++) {
		u32 rx, rs, ts;

		if (!mode->dp_lane[pma])
			continue;
		rx = t8130_dp_rx_top(pma);
		rs = rx + 0x2000;
		ts = t8130_dp_tx_top(pma) + 0x1000;
		core_set32(atcphy, ts + 0x1c, GENMASK(23, 22));
		core_clear32(atcphy, ts + 0x10, BIT(9));
		core_clear32(atcphy, ts + 0x04, BIT(28));
		core_clear32(atcphy, ts + 0x44, BIT(20) | BIT(22));
		udelay(10);
		core_clear32(atcphy, ts + 0x44, BIT(26) | BIT(24));
		core_clear32(atcphy, ts + 0x44, BIT(12));
		core_set32(atcphy, T8130_DP_TX_RAILS, BIT(8 + pma));
		core_clear32(atcphy, T8130_DP_TX_RAILS, BIT(pma));
		udelay(10);
		core_clear32(atcphy, T8130_DP_TX_RAILS, BIT(4 + pma));
		core_clear32(atcphy, rs + 0x30, BIT(10));
		core_set32(atcphy, rs + 0xa8, BIT(22));
		core_clear32(atcphy, rs + 0xa8, BIT(17));
		core_clear32(atcphy, rs + 0x68, BIT(5) | BIT(7));
		core_clear32(atcphy, rs + 0xd4, BIT(16));
		core_set32(atcphy, rs + 0x6c, BIT(9));
		core_clear32(atcphy, rx + 0x1d4, BIT(1));
		core_clear32(atcphy, rs + 0x6c, BIT(11));
		core_set32(atcphy, ACIOPHY_CFG0, BIT(14 + pma));
		core_clear32(atcphy, ACIOPHY_CFG0, BIT(6 + pma));
		udelay(10);
		core_clear32(atcphy, ACIOPHY_CFG0, BIT(10 + pma));
		core_clear32(atcphy, T8130_DP_RX_CLOCK, BIT(pma) | BIT(4 + pma));
	}
	core_clear32(atcphy, 0x7000, DPTXPHY_PMA_LANE_RESET_N);
	ret = t8130_dp_apb_request(atcphy, 0);
	if (ret)
		return ret;
	ret = t8130_dp_apb_release(atcphy, 3);
	if (ret)
		return ret;
	core_clear32(atcphy, AUSPLL_CLKOUT_MASTER, BIT(3));
	core_clear32(atcphy, T8130_DP_CLK_POWER, BIT(7));
	core_set32(atcphy, T8130_DP_CLK_POWER, BIT(4));
	core_clear32(atcphy, T8130_DP_CLK_POWER, BIT(5));
	core_clear32(atcphy, T8130_DP_CLK_POWER, BIT(6));
	atcphy->dp_link_rate = -1;
	return 0;
}

static void t8130_dp_program_lane(struct apple_atcphy *atcphy, unsigned int pma)
{
	u32 rx = t8130_dp_rx_top(pma), tx = t8130_dp_tx_top(pma);
	u32 rs = rx + 0x2000, ts = tx + 0x1000;

	core_set32(atcphy, rs + 0x64, BIT(3));
	core_set32(atcphy, 0x7000, DPTXPHY_PMA_LANE_RESET_N);
	core_set32(atcphy, T8130_DP_TX_RAILS, BIT(4 + pma));
	core_set32(atcphy, T8130_DP_TX_RAILS, BIT(6 + pma));
	udelay(10);
	core_set32(atcphy, T8130_DP_TX_RAILS, BIT(pma));
	core_set32(atcphy, T8130_DP_TX_RAILS, BIT(2 + pma));
	udelay(10);
	core_mask32(atcphy, T8130_DP_TX_RAILS, BIT(8 + pma), BIT(10 + pma));

	/* Native DP clock/reset configuration; separate from board drive levels. */
	writel(0x0aafb800, atcphy->regs.core + ts + 0x44);
	core_set32(atcphy, ts + 0x44, BIT(24));
	core_set32(atcphy, ts + 0x44, BIT(26));
	core_set32(atcphy, ts + 0x44, GENMASK(9, 8));
	core_mask32(atcphy, ts + 0x44, GENMASK(18, 15), 0x10000);
	core_set32(atcphy, tx + 0xd0, BIT(20));
	core_clear32(atcphy, tx + 0x50, 0xc00f);
	core_set32(atcphy, tx + 0xd4, BIT(28));
	core_mask32(atcphy, tx + 0xd0, GENMASK(12, 0), BIT(0));
	core_set32(atcphy, ts + 0x44, BIT(20));
	core_set32(atcphy, ts + 0x44, BIT(22));
	core_set32(atcphy, ts + 0x04, BIT(25));
	core_set32(atcphy, ts + 0x04, GENMASK(29, 28));
	core_set32(atcphy, ts + 0x0c, BIT(26));
	core_mask32(atcphy, ts + 0x10, GENMASK(10, 8), 0x600);
	core_mask32(atcphy, ts + 0x1c, GENMASK(23, 22), BIT(23));

	core_set32(atcphy, rx + 0x160, BIT(0));
	core_set32(atcphy, rx + 0x1dc, GENMASK(16, 15));
	core_set32(atcphy, rs + 0xa4, BIT(1));
	core_set32(atcphy, rs + 0xa8, BIT(16));
	core_set32(atcphy, rs + 0x6c, BIT(16));
	/* RX transmitter mode fields moved from the legacy SHM layout. */
	core_mask32(atcphy, rs + 0xd4, GENMASK(20, 7), 0x123180);
	core_mask32(atcphy, rs + 0x64, GENMASK(15, 11), 0xb800);
	core_set32(atcphy, rs + 0xd8, BIT(2));
	core_clear32(atcphy, rs + 0xdc, BIT(3));
	core_mask32(atcphy, rs + 0xd4, GENMASK(15, 13), 0xc000);
	core_set32(atcphy, T8130_DP_RX_CLOCK, BIT(4 + pma));
	core_set32(atcphy, T8130_DP_RX_CLOCK, BIT(6 + pma));
	udelay(10);
	core_set32(atcphy, T8130_DP_RX_CLOCK, BIT(pma));
	core_set32(atcphy, T8130_DP_RX_CLOCK, BIT(2 + pma));
	core_set32(atcphy, ACIOPHY_CFG0, BIT(10 + pma));
	core_set32(atcphy, ACIOPHY_CFG0, BIT(12 + pma));
	udelay(10);
	core_set32(atcphy, ACIOPHY_CFG0, BIT(6 + pma));
	core_set32(atcphy, ACIOPHY_CFG0, BIT(8 + pma));
	udelay(10);
	core_mask32(atcphy, ACIOPHY_CFG0, BIT(14 + pma), BIT(16 + pma));
	core_set32(atcphy, rs + 0x6c, BIT(11));
	core_set32(atcphy, rx + 0x1d4, BIT(0) | BIT(1));
	core_mask32(atcphy, rs + 0x6c, GENMASK(10, 9), 0x400);
	core_set32(atcphy, rs + 0xd4, BIT(16) | BIT(18));
	core_set32(atcphy, rs + 0xd8, BIT(0));
	core_clear32(atcphy, rs + 0xd4, BIT(18));
	core_set32(atcphy, rs + 0x68, GENMASK(8, 7));
	core_set32(atcphy, rs + 0x68, GENMASK(6, 5));
	core_set32(atcphy, rs + 0xd8, BIT(1));
	core_clear32(atcphy, rs + 0xd8, BIT(0));
	core_clear32(atcphy, rs + 0xd8, BIT(1));
	core_set32(atcphy, rs + 0xa8, GENMASK(18, 17));
	core_set32(atcphy, rs + 0x6c, BIT(14));
	core_clear32(atcphy, rx + 0x210, 0xc00f);
	core_set32(atcphy, rx + 0x248, BIT(28));
	core_set32(atcphy, rx + 0x244, BIT(0));
	core_set32(atcphy, rs + 0xa8, BIT(21) | BIT(23));
	core_set32(atcphy, rs + 0x30, GENMASK(11, 10));
}

static int t8130_dp_configure_rate(struct apple_atcphy *atcphy, unsigned int rate)
{
	const struct atcphy_mode_configuration *mode;
	const struct atcphy_dp_link_rate_configuration *cfg;
	enum atcphy_dp_link_rate lr;
	u32 reg;
	int ret;

	if (!rate)
		return t8130_dp_stop(atcphy);
	/* HBR uses the same lane clock/analog mode as the qualified HBR2 path.
	 * Its lower PLL divider is being tested, rather than assumed qualified. */
	switch (rate) {
	case 2700: lr = ATCPHY_DP_LINK_RATE_HBR; break;
	case 5400: lr = ATCPHY_DP_LINK_RATE_HBR2; break;
	default: return -EOPNOTSUPP;
	}
	cfg = &dp_lr_config[lr];
	mode = atcphy_get_mode_config(atcphy, atcphy->mode);
	if (!mode->dp_lane[0] && !mode->dp_lane[1])
		return -ENOLINK;
	if (atcphy->dp_link_rate == lr)
		return 0;

	for (int pma = 0; pma < 2; pma++)
		if (mode->dp_lane[pma])
			t8130_dp_prepare_lane(atcphy, pma);
	core_set32(atcphy, 0x7000, DPTXPHY_PMA_LANE_RESET_N_OV);
	ret = t8130_dp_apb_request(atcphy, 0);
	if (ret)
		return ret;
	ret = t8130_dp_apb_release(atcphy, 3);
	if (ret)
		return ret;
	core_clear32(atcphy, 0x2200, BIT(3));
	core_set32(atcphy, T8130_DP_CLK_POWER, BIT(6));
	core_set32(atcphy, T8130_DP_CLK_POWER, BIT(5));
	core_clear32(atcphy, T8130_DP_CLK_POWER, BIT(4));
	core_set32(atcphy, T8130_DP_CLK_POWER, BIT(7));
	for (int pma = 0; pma < 2; pma++) {
		if (!mode->dp_lane[pma])
			continue;
		core_clear32(atcphy, T8130_DP_RX_CLOCK, BIT(6 + pma));
		core_clear32(atcphy, T8130_DP_RX_CLOCK, BIT(2 + pma));
	}
	core_clear32(atcphy, 0x2234, BIT(0));
	core_mask32(atcphy, 0x7000,
		DPRX_PCLK_SELECT | DPTX_PCLK1_SELECT | DPTX_PCLK2_SELECT,
		FIELD_PREP(DPRX_PCLK_SELECT, 1) |
		FIELD_PREP(DPTX_PCLK1_SELECT, 1) | FIELD_PREP(DPTX_PCLK2_SELECT, 1));
	/* T8130 keeps VCLK bit20 set; HBR2 descriptor is native-verified. */
	writel(FIELD_PREP(AUSPLL_FD_FREQ_COUNT_TARGET, cfg->freqinit_count_target) |
	       FIELD_PREP(AUSPLL_FD_KI_MAN, 8) | FIELD_PREP(AUSPLL_FD_KI_EXP, 3) |
	       FIELD_PREP(AUSPLL_FD_KP_MAN, 8) | FIELD_PREP(AUSPLL_FD_KP_EXP, 7),
	       atcphy->regs.core + AUSPLL_FREQ_DESC_A);
	writel(FIELD_PREP(AUSPLL_FD_FBDIVN_FRAC_DEN, cfg->fbdivn_frac_den) |
	       FIELD_PREP(AUSPLL_FD_FBDIVN_FRAC_NUM, cfg->fbdivn_frac_num),
	       atcphy->regs.core + AUSPLL_FREQ_DESC_B);
	writel(FIELD_PREP(AUSPLL_FD_PCLK_DIV_SEL, cfg->pclk_div_sel) |
	       FIELD_PREP(AUSPLL_FD_LFSDM_DIV, 1) | FIELD_PREP(AUSPLL_FD_LFCLK_CTRL, cfg->lfclk_ctrl) |
	       FIELD_PREP(AUSPLL_FD_VCLK_OP_DIVN, cfg->vclk_op_divn | 1) | AUSPLL_FD_VCLK_PRE_DIVN,
	       atcphy->regs.core + AUSPLL_FREQ_DESC_C);
	core_mask32(atcphy, AUSPLL_CLKOUT_DIV, AUSPLL_CLKOUT_PLLA_REFBUFCLK_DI,
		FIELD_PREP(AUSPLL_CLKOUT_PLLA_REFBUFCLK_DI, 7));
	core_set32(atcphy, T8130_DP_BGR, BIT(0));
	core_set32(atcphy, AUSPLL_CLKOUT_MASTER, BIT(3));
	ret = t8130_dp_apb_request(atcphy, 0);
	if (ret)
		return ret;
	ret = readl_poll_timeout(atcphy->regs.core + T8130_DP_PLL_LOCK,
		reg, reg & BIT(3), 10, 10000);
	if (ret) {
		dev_err(atcphy->dev, "T8130 DP PLL failed to lock (%#x)\n", reg);
		t8130_dp_apb_release(atcphy, 3);
		return ret;
	}
	ret = t8130_dp_apb_release(atcphy, 0x2800);
	if (ret)
		return ret;
	for (int pma = 0; pma < 2; pma++)
		if (mode->dp_lane[pma])
			t8130_dp_program_lane(atcphy, pma);
	core_clear32(atcphy, 0x7000, DP_PMA_BYTECLK_RESET | DP_MAC_DIV20_CLK_SEL);
	atcphy->dp_link_rate = lr;
	dev_info(atcphy->dev, "T8130 DP PLL locked at %u Mb/s, PMA=%u/%u\n",
		rate, mode->dp_lane[0], mode->dp_lane[1]);
	return 0;
}

static int t8130_dp_drive(struct apple_atcphy *atcphy, struct phy_configure_opts_dp *opts)
{
	const struct atcphy_mode_configuration *mode;
	u32 margin, taps, word, base;
	unsigned int table, pmas[2], n = 0;

	mode = atcphy_get_mode_config(atcphy, atcphy->mode);
	for (int pma = 0; pma < 2; pma++)
		if (mode->dp_lane[pma])
			pmas[n++] = pma;
	if (!opts->lanes || opts->lanes > n * 2 || opts->lanes == 3 ||
	    atcphy->dp_link_rate < 0)
		return -EINVAL;
	table = opts->link_rate >= 8100 ? 2 : opts->link_rate >= 5400 ? 1 : 0;
	for (int lane = 0; lane < opts->lanes; lane++)
		if (opts->voltage[lane] > 3 || opts->pre[lane] > 3 ||
		    opts->voltage[lane] + opts->pre[lane] > 3)
			return -EINVAL;
	for (int lane = 0; lane < opts->lanes; lane++) {
		word = atcphy->dp_drive_settings[table][opts->voltage[lane] * 4 + opts->pre[lane]];
		margin = ((word & 0x3f) >> 2) | ((word & 3) << 14);
		taps = (word >> 5) & 0x7ff80;
		if (lane & 1) {
			base = t8130_dp_tx_top(pmas[lane / 2]);
			core_mask32(atcphy, base + 0x50, 0xc00f, margin);
			core_mask32(atcphy, base + 0xd0, 0x7ff80, taps);
		} else {
			base = t8130_dp_rx_top(pmas[lane / 2]);
			core_mask32(atcphy, base + 0x210, 0xc00f, margin);
			core_mask32(atcphy, base + 0x244, 0x7ff80, taps);
		}
		dev_info(atcphy->dev, "T8130 DP lane %u swing=%u pre=%u table=%#x\n",
			 lane, opts->voltage[lane], opts->pre[lane], word);
	}
	return 0;
}

static int t8130_dp_configure(struct apple_atcphy *atcphy, struct phy_configure_opts_dp *opts)
{
	int ret;

	guard(mutex)(&atcphy->lock);
	if (opts->set_rate) {
		ret = t8130_dp_configure_rate(atcphy, opts->link_rate);
		if (ret)
			return ret;
	}
	if (opts->set_lanes)
		return -EOPNOTSUPP;
	if (opts->set_voltages)
		return t8130_dp_drive(atcphy, opts);
	return 0;
}
