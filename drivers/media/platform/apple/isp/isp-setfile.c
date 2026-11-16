// SPDX-License-Identifier: GPL-2.0-only
/*
 * Lens-shading preparation of a sensor tuning file ("setfile").
 *
 * Some camera modules carry factory-measured lens-shading data, which the ISP
 * firmware returns on request. With those modules the setfile's lens-shading
 * gain tables are a template: the firmware refuses the file until the tables'
 * row strides are cleared, and the picture is only right once the tables are
 * built from the module's own factory data. This file does both, in the copy
 * of the setfile that is about to be uploaded.
 *
 * A setfile is a 20-byte header followed by length-delimited records, each
 * starting with a 16-byte header that gives its length and id. Three records
 * are rewritten:
 *  - the lens-shading record: four 49x49 gain tables, one per plane of the
 *    factory data, each with a descriptor;
 *  - a flag that is set when the tables were built from factory data;
 *  - a 257-entry curve indexed by distance from the image centre.
 * All other records are uploaded as they are.
 */

#include <linux/bits.h>
#include <linux/build_bug.h>
#include <linux/device.h>
#include <linux/errno.h>
#include <linux/math.h>
#include <linux/math64.h>
#include <linux/minmax.h>
#include <linux/slab.h>
#include <linux/string.h>

#include "isp-cmd.h"
#include "isp-drv.h"
#include "isp-setfile.h"

/* Record ids */
#define SETFILE_REC_LSC		0xc003
#define SETFILE_REC_LSC_FACTORY 0x0a2a
#define SETFILE_REC_LSC_CURVE	0xc404

struct setfile_header {
	__be32 magic;
	__le32 unk_4[3];
	__le32 num_records;
} __packed;
static_assert(sizeof(struct setfile_header) == 20);

struct setfile_record {
	__le32 length; /* bytes after this field, up to the next record */
	__le32 zero_4;
	__le32 id;
	__le32 zero_c;
	u8 payload[];
} __packed;
static_assert(sizeof(struct setfile_record) == 16);

/*
 * Gains are unsigned fixed point with LSC_GAIN_SHIFT fractional bits; the
 * firmware accepts gains up to LSC_GAIN_MAX, just under 5.0.
 */
#define LSC_GAIN_SHIFT 12
#define LSC_GAIN_ONE   BIT(LSC_GAIN_SHIFT)
#define LSC_GAIN_MAX   0x4fff

#define LSC_PLANES     4
#define LSC_TABLE_DIM  49 /* entries per table row and column */
#define LSC_TABLE_SIZE (LSC_TABLE_DIM * LSC_TABLE_DIM)
#define LSC_CURVE_SIZE 257

struct setfile_lsc_desc {
	__le16 limit; /* largest gain in the table, 0xffff in the template */
	__le16 unk_2; /* 32 */
	__le16 unk_4; /* 32 */
	__le16 offset; /* index of this table's first entry in table[] */
	__le16 stride; /* row size in bytes in the template, must be 0 */
	__le16 unk_a;
	__le16 unk_c;
	__le16 width;
	__le16 height;
} __packed;
static_assert(sizeof(struct setfile_lsc_desc) == 18);

struct setfile_lsc {
	__le16 unk_0[3]; /* 2, 3072, 3024 */
	struct setfile_lsc_desc desc[LSC_PLANES];
	__le16 num_entries;
	__le16 table[LSC_PLANES * LSC_TABLE_SIZE];
} __packed;
static_assert(sizeof(struct setfile_lsc) == 19288);

struct setfile_lsc_factory {
	__le32 enable;
} __packed;

struct setfile_lsc_curve {
	__le16 unk_0; /* 4 */
	__le16 value[LSC_CURVE_SIZE];
} __packed;
static_assert(sizeof(struct setfile_lsc_curve) == 516);

struct setfile_lsc_records {
	struct setfile_lsc *lsc;
	struct setfile_lsc_factory *factory;
	struct setfile_lsc_curve *curve;
};

/*
 * The fields of unknown meaning are required to hold the values of the
 * template the preparation was established with.
 */
static const u16 setfile_lsc_unk_0[] = { 2, 3072, 3024 };
#define SETFILE_LSC_DESC_UNK_2	 32
#define SETFILE_LSC_DESC_UNK_4	 32
#define SETFILE_LSC_CURVE_UNK_0 4

/*
 * Factory data in the only format the preparation knows: a grid of 19
 * columns and 17 rows, format 0x050c. The INFO reply says 8 when the module
 * has factory lens-shading data, and the preparation applies only then. With
 * 0, all that is known is that the firmware accepts the file with its tables
 * left as they are in the template; that case is not handled here.
 */
#define LSC_INFO_LAYOUT                                             \
	(0x050c0000 | (CISP_LSC_GRID_ROWS << 8) | CISP_LSC_GRID_COLS)
#define LSC_INFO_FACTORY 8
#define LSC_GRID_INFO	 1

/* Positions across the image are fixed point, 0 to LSC_POS_ONE inclusive. */
#define LSC_POS_SHIFT 20
#define LSC_POS_ONE   BIT(LSC_POS_SHIFT)

struct lsc_factory {
	u32 x[CISP_LSC_GRID_COLS];
	u32 y[CISP_LSC_GRID_ROWS];
	u32 peak; /* largest response in any plane */
};

static int setfile_take_record(struct setfile_lsc_records *rec, u32 id,
			       void *payload, size_t len)
{
	switch (id) {
	case SETFILE_REC_LSC:
		if (rec->lsc || len != sizeof(*rec->lsc))
			return -EINVAL;
		rec->lsc = payload;
		break;
	case SETFILE_REC_LSC_FACTORY:
		if (rec->factory || len != sizeof(*rec->factory))
			return -EINVAL;
		rec->factory = payload;
		break;
	case SETFILE_REC_LSC_CURVE:
		if (rec->curve || len != sizeof(*rec->curve))
			return -EINVAL;
		rec->curve = payload;
		break;
	}
	return 0;
}

/*
 * Walk the records, checking that each lies within the file and that they
 * end exactly at its end, and pick out the three lens-shading records.
 */
static int setfile_find_records(void *data, size_t size,
				struct setfile_lsc_records *rec)
{
	const size_t rec_hdr = sizeof(struct setfile_record) -
			       sizeof_field(struct setfile_record, length);
	const struct setfile_header *hdr = data;
	size_t offset = sizeof(*hdr);
	u32 i, count;
	int err;

	memset(rec, 0, sizeof(*rec));
	if (size < sizeof(*hdr))
		return -EINVAL;

	count = le32_to_cpu(hdr->num_records);
	for (i = 0; i < count; i++) {
		struct setfile_record *r = data + offset;
		size_t len;

		if (size - offset < sizeof(*r))
			return -EINVAL;
		len = le32_to_cpu(r->length);
		if (len < rec_hdr || len > size - offset - sizeof(r->length))
			return -EINVAL;
		if (r->zero_4 || r->zero_c)
			return -EINVAL;

		err = setfile_take_record(rec, le32_to_cpu(r->id), r->payload,
					  len - rec_hdr);
		if (err)
			return err;
		offset += sizeof(r->length) + len;
	}

	if (offset != size)
		return -EINVAL;
	if (!rec->lsc || !rec->factory || !rec->curve)
		return -ENOENT;
	return 0;
}

static int setfile_check_lsc(const struct setfile_lsc_records *rec)
{
	const struct setfile_lsc *lsc = rec->lsc;
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(setfile_lsc_unk_0); i++)
		if (le16_to_cpu(lsc->unk_0[i]) != setfile_lsc_unk_0[i])
			return -EINVAL;
	if (le16_to_cpu(lsc->num_entries) != ARRAY_SIZE(lsc->table))
		return -EINVAL;

	for (i = 0; i < LSC_PLANES; i++) {
		const struct setfile_lsc_desc *desc = &lsc->desc[i];
		u16 stride = le16_to_cpu(desc->stride);

		if (le16_to_cpu(desc->limit) != U16_MAX ||
		    le16_to_cpu(desc->unk_2) != SETFILE_LSC_DESC_UNK_2 ||
		    le16_to_cpu(desc->unk_4) != SETFILE_LSC_DESC_UNK_4 ||
		    le16_to_cpu(desc->offset) != i * LSC_TABLE_SIZE ||
		    (stride != LSC_TABLE_DIM * sizeof(__le16) && stride) ||
		    desc->unk_a || desc->unk_c ||
		    le16_to_cpu(desc->width) != LSC_TABLE_DIM ||
		    le16_to_cpu(desc->height) != LSC_TABLE_DIM)
			return -EINVAL;
	}

	if (le16_to_cpu(rec->curve->unk_0) != SETFILE_LSC_CURVE_UNK_0)
		return -EINVAL;
	return 0;
}

/*
 * Convert an IEEE 754 single-precision value in [0, 1] to a fixed-point
 * position, rounding to nearest, without using the FPU.
 */
static int lsc_float_to_pos(u32 bits, u32 *pos)
{
	u32 exp = (bits >> 23) & 0xff;
	u32 mant = bits & GENMASK(22, 0);
	unsigned int shift;

	/* Reject negative values and anything above 1.0, including NaN. */
	if ((bits & BIT(31)) || exp > 127 || (exp == 127 && mant))
		return -EINVAL;

	/*
	 * The value is (mant + 2^23) * 2^(exp - 127 - 23); in fixed point that
	 * is a right shift by 127 + 23 - LSC_POS_SHIFT - exp, at least 3 here.
	 * Zero, subnormals and values that round to zero give position 0.
	 */
	shift = 127 + 23 - LSC_POS_SHIFT - exp;
	if (!exp || shift >= 32) {
		*pos = 0;
		return 0;
	}
	*pos = ((mant | BIT(23)) + BIT(shift - 1)) >> shift;
	return 0;
}

/* Interpolation needs strictly increasing grid positions. */
static int lsc_check_axis(const u32 *axis, unsigned int n)
{
	unsigned int i;

	for (i = 1; i < n; i++)
		if (axis[i] <= axis[i - 1])
			return -EINVAL;
	return 0;
}

/* A good reply has a zero low opcode word and echoes the channel. */
static bool lsc_reply_ok(u64 opcode, u32 reply_chan, u32 chan)
{
	return !lower_32_bits(opcode) && reply_chan == chan;
}

static int lsc_check_info(const struct cmd_ch_lsc_info *info, u32 chan)
{
	if (!lsc_reply_ok(info->opcode, info->chan, chan) ||
	    info->layout != LSC_INFO_LAYOUT || info->info != LSC_INFO_FACTORY)
		return -ENODATA;
	return 0;
}

static int lsc_parse_grid(const struct cmd_ch_lsc_grid *grid, u32 chan,
			  struct lsc_factory *f)
{
	const u8 *resp = &grid->response[0][0][0];
	unsigned int i;
	int err = 0;

	if (!lsc_reply_ok(grid->opcode, grid->chan, chan) ||
	    grid->info != LSC_GRID_INFO)
		return -EINVAL;

	for (i = 0; i < CISP_LSC_GRID_COLS && !err; i++)
		err = lsc_float_to_pos(grid->x[i], &f->x[i]);
	for (i = 0; i < CISP_LSC_GRID_ROWS && !err; i++)
		err = lsc_float_to_pos(grid->y[i], &f->y[i]);
	if (!err)
		err = lsc_check_axis(f->x, CISP_LSC_GRID_COLS);
	if (!err)
		err = lsc_check_axis(f->y, CISP_LSC_GRID_ROWS);
	if (err)
		return err;

	f->peak = 0;
	for (i = 0; i < sizeof(grid->response); i++)
		f->peak = max_t(u32, f->peak, resp[i]);
	if (!f->peak)
		return -ENODATA;
	return 0;
}

/*
 * Find the factory grid interval that holds @pos: return the index of its
 * first point and store in @frac how far into the interval @pos lies, as a
 * fixed-point fraction. Positions outside the grid are clamped to its first
 * or last point, which extends the edge values outwards.
 */
static unsigned int lsc_locate(const u32 *axis, unsigned int n, u32 pos,
			       u32 *frac)
{
	unsigned int i;

	if (pos <= axis[0]) {
		*frac = 0;
		return 0;
	}
	if (pos >= axis[n - 1]) {
		*frac = LSC_POS_ONE;
		return n - 2;
	}

	for (i = 0; pos > axis[i + 1]; i++)
		;
	*frac = div_u64((u64)(pos - axis[i]) << LSC_POS_SHIFT,
			axis[i + 1] - axis[i]);
	return i;
}

/* Table entries are spaced evenly across the image, edges included. */
static u32 lsc_table_pos(unsigned int i)
{
	return (i << LSC_POS_SHIFT) / (LSC_TABLE_DIM - 1);
}

/*
 * Bilinearly interpolate the factory response between the four grid points
 * around (col + fx, row + fy). The result has 2 * LSC_POS_SHIFT fractional
 * bits; with byte-sized responses it cannot overflow.
 */
static u64 lsc_interpolate(const u8 resp[CISP_LSC_GRID_ROWS][CISP_LSC_GRID_COLS],
			   unsigned int row, u32 fy, unsigned int col, u32 fx)
{
	u64 wx1 = fx, wy1 = fy;
	u64 wx0 = LSC_POS_ONE - wx1, wy0 = LSC_POS_ONE - wy1;

	return resp[row][col] * wx0 * wy0 +
	       resp[row][col + 1] * wx1 * wy0 +
	       resp[row + 1][col] * wx0 * wy1 +
	       resp[row + 1][col + 1] * wx1 * wy1;
}

/*
 * The gain that brings @response up to the brightest response in the factory
 * data, rounded to nearest and bounded to what the firmware accepts. A gain
 * is never below one: shading correction only brightens.
 */
static u16 lsc_gain(u64 response, u32 peak)
{
	u64 gain;

	if (!response)
		return LSC_GAIN_MAX;
	gain = div64_u64(((u64)peak << (2 * LSC_POS_SHIFT + LSC_GAIN_SHIFT)) +
			 response / 2, response);
	return clamp_t(u64, gain, LSC_GAIN_ONE, LSC_GAIN_MAX);
}

/* Resample each factory response plane onto its 49x49 gain table. */
static void lsc_build_tables(struct setfile_lsc *lsc,
			     const struct cmd_ch_lsc_grid *grid,
			     const struct lsc_factory *f)
{
	unsigned int plane, row, col;

	for (plane = 0; plane < LSC_PLANES; plane++) {
		unsigned int base = le16_to_cpu(lsc->desc[plane].offset);

		for (row = 0; row < LSC_TABLE_DIM; row++) {
			unsigned int gy;
			u32 fy;

			gy = lsc_locate(f->y, CISP_LSC_GRID_ROWS,
					lsc_table_pos(row), &fy);
			for (col = 0; col < LSC_TABLE_DIM; col++) {
				unsigned int gx;
				u64 resp;
				u32 fx;

				gx = lsc_locate(f->x, CISP_LSC_GRID_COLS,
						lsc_table_pos(col), &fx);
				resp = lsc_interpolate(grid->response[plane],
						       gy, fy, gx, fx);
				lsc->table[base + row * LSC_TABLE_DIM + col] =
					cpu_to_le16(lsc_gain(resp, f->peak));
			}
		}
	}
}

/*
 * Fill the curve from the finished gain tables. How the firmware interprets
 * the curve is not established; what is known is that with the template's
 * flat curve the corners, where the gains are largest, come out visibly
 * noisy. The values here are our own conservative choice: entry i covers the
 * squared distance from the table centre i / 256 of the way to a corner, and
 * holds the reciprocal of the running maximum, over all planes, of the gains
 * at or inside that distance. The curve therefore never rises outwards.
 */
static void lsc_build_curve(struct setfile_lsc_curve *curve,
			    const struct setfile_lsc *lsc)
{
	const int mid = LSC_TABLE_DIM / 2;
	const int r2_max = 2 * mid * mid;
	u16 ring[LSC_CURVE_SIZE] = {};
	unsigned int plane, i;
	u32 peak = LSC_GAIN_ONE;
	int row, col;

	for (plane = 0; plane < LSC_PLANES; plane++) {
		unsigned int base = le16_to_cpu(lsc->desc[plane].offset);

		for (row = 0; row < LSC_TABLE_DIM; row++) {
			for (col = 0; col < LSC_TABLE_DIM; col++) {
				int dx = col - mid, dy = row - mid;
				int bin = (dx * dx + dy * dy) *
					  (LSC_CURVE_SIZE - 1) / r2_max;
				unsigned int idx = base + row * LSC_TABLE_DIM + col;
				u16 gain = le16_to_cpu(lsc->table[idx]);

				ring[bin] = max(ring[bin], gain);
			}
		}
	}

	for (i = 0; i < LSC_CURVE_SIZE; i++) {
		peak = max_t(u32, peak, ring[i]);
		curve->value[i] =
			cpu_to_le16(DIV_ROUND_CLOSEST(LSC_GAIN_ONE * LSC_GAIN_ONE,
						      peak));
	}
}

static void setfile_apply_lsc(const struct setfile_lsc_records *rec,
			      const struct cmd_ch_lsc_grid *grid,
			      const struct lsc_factory *f)
{
	unsigned int i;

	for (i = 0; i < LSC_PLANES; i++) {
		rec->lsc->desc[i].limit = cpu_to_le16(LSC_GAIN_MAX);
		rec->lsc->desc[i].stride = 0;
	}
	lsc_build_tables(rec->lsc, grid, f);
	rec->factory->enable = cpu_to_le32(1);
	lsc_build_curve(rec->curve, rec->lsc);
}

/**
 * isp_setfile_prepare_lsc() - build a setfile's lens shading from factory data
 * @isp: the ISP
 * @chan: channel of the camera module
 * @data: the setfile, modified in place
 * @size: size of the setfile in bytes
 *
 * Validates the setfile, reads the module's factory lens-shading data and
 * rewrites the setfile's lens-shading records from it. On error @data is left
 * unchanged.
 *
 * Return: 0 on success, a negative error code otherwise.
 */
int isp_setfile_prepare_lsc(struct apple_isp *isp, u32 chan, void *data,
			    size_t size)
{
	struct setfile_lsc_records rec;
	struct cmd_ch_lsc_info info;
	struct cmd_ch_lsc_grid *grid;
	struct lsc_factory factory;
	int err;

	err = setfile_find_records(data, size, &rec);
	if (!err)
		err = setfile_check_lsc(&rec);
	if (err) {
		dev_dbg(isp->dev, "setfile lens-shading layout not recognised\n");
		return err;
	}

	err = isp_cmd_ch_lsc_info_get(isp, chan, &info);
	if (err)
		return err;
	err = lsc_check_info(&info, chan);
	if (err) {
		dev_dbg(isp->dev, "factory LSC info not usable: layout %#x info %#x\n",
			info.layout, info.info);
		return err;
	}

	grid = kzalloc(sizeof(*grid), GFP_KERNEL);
	if (!grid)
		return -ENOMEM;
	err = isp_cmd_ch_lsc_grid_get(isp, chan, grid);
	if (!err) {
		err = lsc_parse_grid(grid, chan, &factory);
		if (err)
			dev_dbg(isp->dev, "factory LSC grid not usable\n");
	}
	if (!err) {
		setfile_apply_lsc(&rec, grid, &factory);
		dev_dbg(isp->dev,
			"setfile lens shading built from factory data, gain limit %#x\n",
			LSC_GAIN_MAX);
	}
	kfree(grid);
	return err;
}
