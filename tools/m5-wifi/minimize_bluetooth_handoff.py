#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Reduce own Bluetooth metadata to named fields and accessed DMA pages.

For LLMs: this shrinks metadata and Python's allowed physical access set.
It does not free firmware/DART allocations, or claim their contents understood.
"""
import argparse
import json
from pathlib import Path

PAGE = 4096


def minimize(state, auxiliary_rx=(), inactive_tx=(), *, queue_entries=None):
    queue_entries = {} if queue_entries is None else dict(queue_entries)
    queues = []
    for item in state['bluetooth_queue_configs']:
        words, number = item['words'], item['queue']
        if number not in (1, 2, 7, 8) + tuple(auxiliary_rx) + tuple(inactive_tx):
            continue
        original_entries = words[6] & 65535
        entries = queue_entries.get(number, original_entries)
        if type(entries) is not int or not 2 <= entries <= original_entries:
            raise ValueError('Queue slot probe must fit the original allocation')
        encoding = words[0]
        exponent, mantissa = encoding >> 28, (encoding >> 16) & 255
        if encoding & 0x0f00ffff != 1 or exponent > 4 or not mantissa:
            raise ValueError('Unknown inline capacity encoding')
        queues.append(dict(number=number, ring_iova=words[2] | words[3] << 32,
                           entries=entries, capacity=mantissa << (2 + exponent)))
        if number in auxiliary_rx:
            queues[-1]['receive_only'] = True
        if number in inactive_tx:
            queues[-1]['inactive_tx'] = True
        if number == 13:
            queues[-1]['configuration_bit19'] = bool(words[7] & (1 << 19))
    if set(queue_entries) - {q['number'] for q in queues}:
        raise ValueError('Queue slot probe names an unselected queue')
    context = state['bluetooth_context_iova']
    wanted = set(range(context & -PAGE, (context + 0x4000 + PAGE - 1) & -PAGE, PAGE))
    for q in queues:
        start, end = q['ring_iova'], q['ring_iova'] + q['entries'] * (16 + q['capacity'])
        wanted.update(range(start & -PAGE, (end + PAGE - 1) & -PAGE, PAGE))
    translations = []
    for page in sorted(wanted):
        m = next((m for m in state['streams']['3']['mappings']
                  if m['iova'] <= page and page + PAGE <= m['iova'] + m['length']), None)
        if m is None:
            raise ValueError('Missing retained queue translation')
        physical = m['physical'] + page - m['iova']
        if translations and translations[-1]['iova'] + translations[-1]['length'] == page and translations[-1]['physical'] + translations[-1]['length'] == physical:
            translations[-1]['length'] += PAGE
        else:
            translations.append(dict(iova=page, physical=physical, length=PAGE))
    physical = [(m['physical'], m['length']) for m in translations]
    physical.append((state['handoff_nonce_physical'] & -PAGE, PAGE))
    keep = ('system_serial', 'dart_base', 'dram_base', 'dram_size', 'handoff_nonce',
            'handoff_nonce_physical', 'bluetooth_context_iova', 'bluetooth_bar0_physical')
    result = {key: state[key] for key in keep}
    result.update(streams={'3': {'mappings': translations}}, reserved_dram=sorted(physical),
                  bluetooth_queues=queues)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--auxiliary-rx', type=int, nargs='*', choices=(9, 10, 11, 12, 13), default=[],
                        help='Maintain these confirmed RX queues without reading their bodies')
    args = parser.parse_args()
    state = minimize(json.loads(args.input.read_text()), auxiliary_rx=tuple(args.auxiliary_rx))
    args.output.write_text(json.dumps(state, separators=(',', ':')) + '\n')
    print('Bluetooth metadata bytes:', args.output.stat().st_size,
          'allowed RAM bytes:', sum(n for _, n in state['reserved_dram']))


if __name__ == '__main__':
    main()
