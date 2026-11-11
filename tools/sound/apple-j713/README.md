# J713 headphone-only commissioning

Kernel card ID `AppleJ713`, playback PCM2 only, 48kHz stereo S24_LE. Devices0/1
are reserved and absent. No capture, speaker route, amplifier dependency or
thermal model. MCA TX2/port2 uses ADMAC channel4; CS42L84 uses12MHz sysclk.
This retains the mini's transport choice; Air playback has not been qualified.

Install `ucm2/conf.d/AppleJ713/` under `/usr/share/alsa/ucm2/conf.d/` or use
`ALSA_CONFIG_UCM2` pointing to this `ucm2/` tree. UCM queries and ALSA control
subscriptions do not require opening PCM devices. Do not select/play a PCM
until the user authorizes audible commissioning. Stage A silent checks mask
PipeWire/WirePlumber autostart and trace PCM opens; no zero-filled DMA test.

UCM exposes only Headphones, jack availability and the existing Jack DAC
control/route selectors. It must not be renamed from a speaker profile.
The board DT deliberately omits all SN012776 nodes and holds shared amp
GPIO15 low. Headset mic and internal microphones are deferred.
