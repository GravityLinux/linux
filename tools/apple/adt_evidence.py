#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Collect read-only Apple Silicon IODeviceTree evidence from macOS."""

import argparse
import json
import platform
import plistlib
import subprocess
import sys

OMIT = object()

SAFE_PROPERTIES = {
    "name",
    "compatible",
    "device_type",
    "reg",
    "ranges",
    "bus-range",
    "interrupts",
    "interrupt-parent",
    "interrupt-map",
    "interrupt-map-mask",
    "msi-ranges",
    "#address-cells",
    "#size-cells",
    "#interrupt-cells",
    "#gpio-cells",
    "#iommu-cells",
    "gpio-ranges",
    "apple,npins",
    "pinmux",
    "iommus",
    "iommu-map",
    "iommu-map-mask",
    "power-domains",
    "power-domain-names",
    "power-gates",
    "clock-gates",
    "clocks",
    "clock-names",
    "resets",
    "reset-gpios",
    "pwren-gpios",
    "max-link-speed",
    "expected-link-speed",
    "num-lanes",
    "lane-count",
    "port-number",
    "phandle",
    "AAPL,phandle",
    "vendor-id",
    "device-id",
    "class-code",
}

REDACT_TERMS = (
    "serial",
    "mac-address",
    "bd-address",
    "bluetooth-address",
    "calibration",
    "unique",
    "uuid",
    "imei",
)

STRING_PROPERTIES = {
    "name",
    "compatible",
    "device_type",
    "power-domain-names",
    "clock-names",
}

PROFILES = {
    "j614s-pcie-sd": {
        "description": "J614s/T6040 PCIe and SD-reader bring-up evidence",
        "target": {
            "board": "J614s",
            "soc": "T6040",
            "model": "Mac16,8",
        },
        "expected_model": "Mac16,8",
        "expected_cpu": "Apple M4 Pro",
        "exact_names": {
            "apcie0",
            "pci-bridge1",
            "pcie-sdreader",
            "dart-apcie0",
            "dart-apcie1",
            "mapper-apcie1",
            "mapper-apcie1-piodma",
            "apcie1-piodma",
            "smc-gpio0",
            "sd-card",
            "pcie-sdreader-helper",
        },
    },
}


def run_bytes(command):
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout if result.returncode == 0 else None


def run_text(command):
    output = run_bytes(command)
    if output is None:
        return None
    return output.decode(errors="replace").strip()


def decode_dt_strings(value):
    if not isinstance(value, (bytes, bytearray)):
        return value
    parts = [part for part in bytes(value).split(b"\0") if part]
    if not parts:
        return ""
    try:
        decoded = [part.decode("utf-8") for part in parts]
    except UnicodeDecodeError:
        return None
    return decoded[0] if len(decoded) == 1 else decoded


def bytes_to_cells(value):
    raw = bytes(value)
    if len(raw) % 4:
        return None
    return [
        f"0x{int.from_bytes(raw[i:i + 4], 'big'):08x}"
        for i in range(0, len(raw), 4)
    ]


def should_redact(key):
    lower = key.lower()
    return any(term in lower for term in REDACT_TERMS)


def sanitize_value(name, value):
    if isinstance(value, (bytes, bytearray)):
        if name in STRING_PROPERTIES:
            decoded = decode_dt_strings(value)
            if decoded is not None:
                return decoded
        cells = bytes_to_cells(value)
        if cells is not None and len(value) <= 256:
            return {"bytes": len(value), "cells": cells}
        return {"bytes": len(value), "omitted": True}

    if isinstance(value, dict):
        cleaned = {}
        for key, child in value.items():
            key = str(key)
            if key == "IORegistryEntryChildren" or should_redact(key):
                continue
            normalized = sanitize_value(key, child)
            if normalized is not OMIT:
                cleaned[key] = normalized
        return cleaned

    if isinstance(value, (list, tuple)):
        cleaned = []
        for item in value:
            normalized = sanitize_value(name, item)
            if normalized is not OMIT:
                cleaned.append(normalized)
        return cleaned

    if isinstance(value, (str, int, float, bool)) or value is None:
        return value

    return OMIT


def node_name(node):
    name = node.get("IORegistryEntryName")
    if isinstance(name, str) and name:
        return name
    decoded = decode_dt_strings(node.get("name"))
    if isinstance(decoded, str) and decoded:
        return decoded
    if isinstance(decoded, list) and decoded:
        return decoded[0]
    return "<unnamed>"


def compatible_strings(node):
    decoded = decode_dt_strings(node.get("compatible"))
    if decoded is None:
        return []
    if isinstance(decoded, str):
        return [decoded]
    if isinstance(decoded, list):
        return [str(item) for item in decoded]
    if isinstance(decoded, (bytes, bytearray)):
        return []
    return [str(decoded)]


def profile_matches(profile, node):
    return node_name(node).lower() in profile["exact_names"]


def extract_nodes(archive, profile):
    if isinstance(archive, dict):
        roots = [archive]
    elif isinstance(archive, list):
        roots = archive
    else:
        raise ValueError("Unexpected IORegistry archive root type")

    found = []
    seen_paths = set()

    def walk(node, parent_path=""):
        if not isinstance(node, dict):
            return

        name = node_name(node)
        path = f"{parent_path}/{name}" if parent_path else f"/{name}"

        if profile_matches(profile, node) and path not in seen_paths:
            seen_paths.add(path)
            properties = {}
            for key, value in node.items():
                key = str(key)
                if key in {"IORegistryEntryChildren", "IORegistryEntryName"}:
                    continue
                if key not in SAFE_PROPERTIES or should_redact(key):
                    continue
                normalized = sanitize_value(key, value)
                if normalized is not OMIT:
                    properties[key] = normalized

            found.append({
                "path": path,
                "name": name,
                "compatible": compatible_strings(node),
                "properties": properties,
            })

        children = node.get("IORegistryEntryChildren", [])
        if isinstance(children, list):
            for child in children:
                walk(child, path)

    for root in roots:
        walk(root)

    return found


def read_iodevicetree(run=run_bytes):
    raw = run(["/usr/sbin/ioreg", "-p", "IODeviceTree", "-l", "-a"])
    if not raw:
        return None
    try:
        return plistlib.loads(raw)
    except (plistlib.InvalidFileException, ValueError, TypeError):
        return None


def host_info(profile, run=run_text):
    model = run(["/usr/sbin/sysctl", "-n", "hw.model"])
    cpu_brand = run(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"])
    return {
        "model": model,
        "cpu_brand": cpu_brand,
        "macos_version": run(["/usr/bin/sw_vers", "-productVersion"]),
        "macos_build": run(["/usr/bin/sw_vers", "-buildVersion"]),
        "matches_profile": (
            model == profile["expected_model"]
            and cpu_brand == profile["expected_cpu"]
        ),
    }


def collect(profile_name, archive=None, run_binary=run_bytes,
            run_string=run_text, system=None):
    profile = PROFILES[profile_name]
    system = platform.system() if system is None else system

    if archive is None and system != "Darwin":
        return {
            "report_version": 3,
            "status": "unsupported_host",
            "profile": profile_name,
            "ready_to_boot": False,
            "reason": "Live collection must run on macOS.",
        }

    if archive is None:
        host = host_info(profile, run_string)
        archive = read_iodevicetree(run_binary)
        if archive is None:
            return {
                "report_version": 3,
                "status": "ioreg_failed",
                "profile": profile_name,
                "host": host,
                "ready_to_boot": False,
                "reason": "Could not read or parse the IODeviceTree archive.",
            }
    else:
        host = {
            "model": None,
            "cpu_brand": None,
            "macos_version": None,
            "macos_build": None,
            "matches_profile": None,
        }

    nodes = extract_nodes(archive, profile)
    property_nodes = sum(bool(node["properties"]) for node in nodes)
    reason = None

    if not nodes:
        status = "no_matching_nodes"
    elif property_nodes == 0:
        status = "evidence_incomplete"
        reason = (
            "Matching IODeviceTree nodes were found, but none included exported "
            "properties."
        )
    elif host["matches_profile"] is False:
        status = "model_unconfirmed"
    else:
        status = "evidence_collected"

    report = {
        "report_version": 3,
        "status": status,
        "profile": profile_name,
        "profile_description": profile["description"],
        "target": profile["target"],
        "host": host,
        "source": "ioreg -p IODeviceTree -l -a",
        "read_only": True,
        "node_count": len(nodes),
        "property_nodes": property_nodes,
        "nodes": nodes,
        "ready_to_boot": False,
        "notice": (
            "This report is hardware evidence only. It does not approve a boot, "
            "modify disks, change boot policy, load firmware, or write NVRAM."
        ),
    }
    if reason is not None:
        report["reason"] = reason
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        choices=sorted(PROFILES),
        default="j614s-pcie-sd",
        help="Evidence profile to collect (default: %(default)s).",
    )
    parser.add_argument(
        "--list-profiles",
        action="store_true",
        help="List available evidence profiles and exit.",
    )
    parser.add_argument(
        "--input",
        metavar="PLIST",
        help="Parse a saved ioreg plist instead of querying macOS.",
    )
    parser.add_argument(
        "--output",
        metavar="JSON",
        help="Write JSON to this file instead of stdout.",
    )
    args = parser.parse_args()

    if args.list_profiles:
        for name in sorted(PROFILES):
            print(f"{name}: {PROFILES[name]['description']}")
        return 0

    archive = None
    if args.input:
        with open(args.input, "rb") as handle:
            archive = plistlib.load(handle)

    report = collect(args.profile, archive=archive)
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"

    if args.output:
        with open(args.output, "w", encoding="utf-8") as handle:
            handle.write(rendered)
    else:
        sys.stdout.write(rendered)

    return 0 if report["status"] == "evidence_collected" else 2


if __name__ == "__main__":
    sys.exit(main())
