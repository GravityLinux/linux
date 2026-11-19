# SPDX-License-Identifier: GPL-2.0-only
"""Small native Linux GPIO-v2 probe using include/uapi/linux/gpio.h.

The caller selects a GPIO chip and line explicitly. With no output option,
request the existing direction unchanged. This tool has no device identity or
board-specific register assumptions.
"""
import argparse
import ctypes as C
import fcntl
import json
import os


class ChipInfo(C.Structure):
    _fields_ = [('name', C.c_char * 32), ('label', C.c_char * 32), ('lines', C.c_uint32)]


class Attribute(C.Structure):
    _fields_ = [('id', C.c_uint32), ('padding', C.c_uint32), ('value', C.c_uint64)]


class ConfigAttribute(C.Structure):
    _fields_ = [('attribute', Attribute), ('mask', C.c_uint64)]


class Config(C.Structure):
    _fields_ = [('flags', C.c_uint64), ('num_attrs', C.c_uint32),
                ('padding', C.c_uint32 * 5), ('attrs', ConfigAttribute * 10)]


class Request(C.Structure):
    _fields_ = [('offsets', C.c_uint32 * 64), ('consumer', C.c_char * 32),
                ('config', Config), ('num_lines', C.c_uint32),
                ('event_buffer_size', C.c_uint32), ('padding', C.c_uint32 * 5),
                ('fd', C.c_int32)]


class Values(C.Structure):
    _fields_ = [('bits', C.c_uint64), ('mask', C.c_uint64)]


def ioctl(fd, number, value, *, read_only=False):
    direction = 2 if read_only else 3  # asm-generic/ioctl.h: READ / READ|WRITE
    command = (direction << 30) | (C.sizeof(value) << 16) | (0xb4 << 8) | number
    data = bytearray(bytes(value))
    fcntl.ioctl(fd, command, data, True)
    C.memmove(C.addressof(value), bytes(data), len(data))


def chip_info(path):
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
    try:
        info = ChipInfo()
        ioctl(fd, 1, info, read_only=True)
        return {'path':os.fspath(path), 'name':info.name.decode(),
                'label':info.label.decode(), 'lines':info.lines}
    finally:
        os.close(fd)


class Line:
    def __init__(self, path, offset, *, initial_output=None):
        info = chip_info(path)
        if not 0 <= offset < info['lines'] or initial_output not in (None, 0, 1):
            raise ValueError('Invalid GPIO line or output value')
        request = Request()
        request.fd = -1
        request.offsets[0] = offset
        request.consumer = b'centauri-lab'
        request.num_lines = 1
        self.output = initial_output is not None
        if self.output:
            request.config.flags = 1 << 3  # GPIO_V2_LINE_FLAG_OUTPUT
            request.config.num_attrs = 1
            request.config.attrs[0].attribute.id = 2  # OUTPUT_VALUES
            request.config.attrs[0].attribute.value = initial_output
            request.config.attrs[0].mask = 1
        fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
        try:
            ioctl(fd, 7, request)
        finally:
            os.close(fd)
        self.fd = request.fd
        if self.fd < 0:
            raise RuntimeError('GPIO request returned no file descriptor')

    def get(self):
        value = Values(0, 1)
        ioctl(self.fd, 0x0e, value)
        return value.bits & 1

    def set(self, value):
        if not self.output or value not in (0, 1):
            raise ValueError('An explicit output request and binary value are required')
        ioctl(self.fd, 0x0f, Values(value, 1))

    def close(self):
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('chip')
    parser.add_argument('line', type=int, nargs='?')
    parser.add_argument('--initial-output', type=int, choices=(0, 1))
    parser.add_argument('--value', type=int, choices=(0, 1))
    args = parser.parse_args()
    result = chip_info(args.chip)
    if args.line is not None:
        with Line(args.chip, args.line, initial_output=args.initial_output) as line:
            result['line'] = args.line
            result['before'] = line.get()
            if args.value is not None:
                line.set(args.value)
                result['after'] = line.get()
    elif args.initial_output is not None or args.value is not None:
        parser.error('An explicit line is required for output operations')
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
