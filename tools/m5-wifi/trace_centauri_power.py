# SPDX-License-Identifier: GPL-2.0-only
"""Scoped reference capture of the Centauri reset/CLKREQ transport.

Use after trace_centauri.py. This is observation tooling, not the standalone
Python driver. Message definitions and endpoint API are from Gravity/Asahi;
only the SMC keys and GPIO pins explicitly named by this device's ADT are kept.
No firmware executable or unrelated SMC payload is read.
"""

from m1n1.trace.asc import ASCTracer, EP, msg, DIR
from m1n1.fw.smc import (
    SMCMessage, SMCWriteKey, SMCReadKey, SMCResult,
    SMC_WRITE_KEY, SMC_READ_KEY, SMC_NOTIFICATION,
)
from m1n1.hv import TraceMode
from m1n1.utils import Register64, irange


class SMCTraceMessage(Register64):
    TYPE = 7, 0
    ID = 15, 12
    SIZE = 23, 16
    KEY = 63, 32


class CentauriPowerEndpoint(EP):
    # Replies need not satisfy the request header's reserved-zero fields.
    BASE_MESSAGE = SMCTraceMessage

    def __init__(self, tracer, epid):
        super().__init__(tracer, epid)
        self.state.sram = None
        self.state.pending = {}

    @msg(SMC_WRITE_KEY, DIR.TX, SMCTraceMessage)
    def write_key(self, message):
        self.state.pending.pop(message.ID, None)
        if message.KEY not in self.tracer.keys:
            return True
        key = message.KEY.to_bytes(4, "big").decode("ascii")
        self.state.pending[message.ID] = ("write", key)
        if self.state.sram is None or not 0 < message.SIZE <= 4:
            self.tracer.capture.emit("smc_power_unavailable", key=key, size=message.SIZE)
            return True
        data = self.hv.iface.readmem(self.state.sram, message.SIZE)
        self.tracer.capture.emit("smc_power_write", key=key,
                                 value=hex(int.from_bytes(data, "little")),
                                 size=message.SIZE, message_id=message.ID)
        return True

    @msg(SMC_READ_KEY, DIR.TX, SMCTraceMessage)
    def read_key(self, message):
        self.state.pending.pop(message.ID, None)
        if message.KEY in self.tracer.keys:
            key = message.KEY.to_bytes(4, "big").decode("ascii")
            self.state.pending[message.ID] = ("read", key)
            self.tracer.capture.emit("smc_power_read", key=key, message_id=message.ID)
        return True

    @msg(None, DIR.TX, SMCTraceMessage)
    def other_request(self, message):
        self.state.pending.pop(message.ID, None)
        return True

    @msg(None, DIR.RX, Register64)
    def reply(self, message):
        if self.state.sram is None:
            self.state.sram = message.value
            self.tracer.capture.emit("smc_shared_memory", address=hex(message.value))
            return True
        result = SMCResult(message.value)
        if result.RESULT == SMC_NOTIFICATION:
            return True
        pending = self.state.pending.pop(result.ID, None)
        if pending is not None:
            direction, key = pending
            self.tracer.capture.emit("smc_power_reply", key=key, request=direction,
                                     result=result.RESULT, value=hex(result.VALUE),
                                     message_id=result.ID)
        return True


class CentauriPowerSMC(ASCTracer):
    ENDPOINTS = {0x20: CentauriPowerEndpoint}

    def __init__(self, hv, capture, keys):
        self.capture = capture
        self.keys = keys
        super().__init__(hv, "/arm-io/smc", verbose=0)

    def handle_msg(self, direction, message, envelope):
        # In particular, do not run the inherited firmware-syslog reader.
        if envelope.EP == 0x20:
            return self.epmap[0x20].handle_msg(direction, message, envelope)


def start_power_capture(hv, capture):
    bridge = hv.adt["/arm-io/apcie/pci-bridge0"]
    keys = set()
    for name in ("function-reset", "function-warm_reset"):
        reference = bridge.getprop(name)
        if reference.name == "pKW4":
            keys.add(int(reference.args[0]))
    tracer = CentauriPowerSMC(hv, capture, keys)
    tracer.start()
    for name in ("function-perst", "function-clkreq"):
        reference = bridge.getprop(name)
        if reference.name != "GPIO":
            raise ValueError(f"Unexpected {name} transport")
        controller = next(n for n in hv.adt.walk_tree()
                          if n.getprop("AAPL,phandle") == reference.phandle)
        base, size = controller.get_reg(0)
        pin = int(reference.args[0])
        assert 4 * pin + 4 <= size
        address = base + 4 * pin
        region = f"gpio/{name}"
        capture.gpio_snapshots.append((region, address))
        hv.add_tracer(irange(address, 4), "centauri/" + region,
                      TraceMode.ASYNC, capture.event, capture.event,
                      region=region, base=base)
        capture.emit("region", name=region, physical=hex(address), size=4,
                     pin=pin, controller=controller.name)
    return tracer


if "hv" in globals():
    centauri_power_trace = start_power_capture(hv, centauri_trace)
