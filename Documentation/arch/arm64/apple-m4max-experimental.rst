Experimental M4 Max peripheral integration
==========================================

All M4 Max peripheral additions in the experimental topic branches are
experimental. They are being integrated with the existing Neo and M4 drivers;
they are not a production release or a claim of full hardware qualification.
The existing Neo implementations and commit history are retained. No GPU-driver changes are part
of this series.

The combined tree adds J616c board descriptions and extends the existing
MTP, Broadcom wireless, AOP audio, PMP, ISP, DCP and CPUFreq implementations.
The Max speaker card/PCM remains a standalone implementation using the shared
TAS2764, ADMAC, AOP transport and thermal model. Its fixed -60dB hardware
attenuation ceiling remains in place.

Both platform descriptions and firmware layouts must be retained. In
particular, Neo's four-surface DCP ABI, its ISP sensor/bootstrap paths and
its AOP transport are not replaced by the Max variants. Shared firmware
protocol and power-report fixes belong in their existing subsystem drivers.

CPU OPP frequencies are filled by m1n1 from board data. The matching
bootloader change is required when booting the Max device tree. Linux reads
normal FDT properties; no raw ADT parsing is introduced in Linux.

Testing and remaining limits
----------------------------

Tests must identify their exact source/configuration and distinguish host
checks, builds and hardware workloads. A successful Neo build is not Neo
hardware qualification. Max hardware tests use all sixteen CPUs. The current
integration campaign covers display/VRR, audio, camera, wireless and CPUFreq.

Known limits include the unexplained upper P-state cap, unqualified thermal
management, quiet-only speaker playback, camera image-quality qualification and
unqualified external ports. Suspend/resume remains outside this series.

Camera calibration firmware
---------------------------

The J616c sensor0958 uses ``apple/isp_2224_01XX.dat``: magic ``0x22240106``,
28660 bytes. Install it in ``/lib/firmware/apple/`` before opening the camera.
The ordinary ISP setfile loader validates the size and magic and submits it
using the existing 64-bit ``CH_SET_FILE_LOAD`` command. A successful firmware
reply produces a "loaded calibration setfile" message.

Extract the data from the target OS's ``appleh16camerad`` using the companion
experimental asahi_firmware patch. Raw firmware archives must retain that
daemon alongside ``appleh13camerad`` for older cameras. The native Max capture
used for bring-up is retained privately with its provenance; proprietary
calibration bytes are not committed to the kernel tree. An unrelated host's
daemon can contain different data under the same magic and is not a validated
substitute. Loading calibration is distinct from complete image-quality and
privacy-LED qualification.
