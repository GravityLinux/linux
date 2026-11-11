# J773g speaker and headphone playback

The J773g machine driver exposes the protected internal speaker on
`hw:T8132Speaker,0` and stereo headphones on `hw:T8132Speaker,2`.
Both endpoints use 48 kHz S24_LE (24 significant bits in 32-bit containers).
The headphone codec also supplies the `Headphone Jack` presence control.
Headset microphone capture is not implemented.

The headphone link uses MCA clock/sync/serializer/port 2 and ADMAC TX2
(logical DMA channel 4). The CS42L84 is at I2C1 address 0x4b, with reset on
nub GPIO 17 and an active-low interrupt on AP GPIO 98. These assignments
were checked against the J773g platform description and live macOS I2C/MMIO
activity. No proprietary code was disassembled or decompiled.

Linux divides the 24 MHz reference by two and uses a 250-bit frame for
48 kHz. Two 32-bit TDM slots carry the samples; the remainder of each frame
is idle. The codec uses its existing 12 MHz internal-clock mode directly,
without its PLL. This initializes independently of macOS's NCO clock setup.

MCA's T8132 register aperture contains separate banks of 0x100-byte units,
not consecutive 0xc000-byte clusters. Each frontend gets its own global
output clock selector and enable bit, so headphone and speaker streams can
run independently. The speaker alone uses the protected PCM component;
headphone playback never enters the speaker thermal model.

## Desktop configuration

Install the supplied ALSA UCM files on the target:

```sh
sudo install -d /usr/share/alsa/ucm2/conf.d/T8132Speaker
sudo install -m644 ucm2/conf.d/T8132Speaker/*.conf \
  /usr/share/alsa/ucm2/conf.d/T8132Speaker/
systemctl --user restart wireplumber
```

PipeWire then exposes **Headphones** and **Internal Speaker** under the
HiFi profile, including jack availability and their separate volume
controls. Application formats are converted to the native 48 kHz S24_LE
format. Select Headphones in the desktop audio controls if another output
was saved as the default.

## Bring-up validation

Use a matching kernel and DTB, with SND_SOC_APPLE_T8132_SPEAKER enabled.
The option selects the MCA, TAS2764, CS42L84 and ADMAC drivers. Install
matching modules and built-in module metadata as well, including when
uinput is built in: startup services may still call modprobe for it.

Check `aplay -l`, `amixer -c T8132Speaker contents`, and the PipeWire routes.
For direct playback, supply raw stereo S24_LE at 48 kHz, or use PipeWire for
format conversion. The `speaker-audio/state` sysfs attribute reports the
speaker limiter separately; headphone-only playback must leave it idle.

The initial native format is deliberately restricted. In live tests,
configuring the codec receive width as 32 bits changed the observed channel
output; changing the MCA adapter count and DMA frame metadata did not
resolve it. This needs further codec framing investigation before widening
the machine driver's advertised formats. Do not treat successful DMA
completion alone as proof of correct channel output.
