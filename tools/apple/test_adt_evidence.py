#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
import importlib.util
from pathlib import Path
import plistlib
import unittest

MODULE_PATH = Path(__file__).with_name("adt_evidence.py")
SPEC = importlib.util.spec_from_file_location("adt_evidence", MODULE_PATH)
REPORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPORT)


class AdtEvidenceTests(unittest.TestCase):
    def profile(self):
        return REPORT.PROFILES["j614s-pcie-sd"]

    def sample_archive(self):
        return [{
            "IORegistryEntryName": "Root",
            "IORegistryEntryChildren": [{
                "IORegistryEntryName": "device-tree",
                "IORegistryEntryChildren": [{
                    "IORegistryEntryName": "arm-io",
                    "compatible": b"arm-io,t6040\0",
                    "IORegistryEntryChildren": [{
                        "IORegistryEntryName": "apcie0",
                        "compatible": b"apcie,t6040\0",
                        "reg": bytes.fromhex(
                            "0000001c b0000000 00000000 10000000"
                        ),
                        "interrupts": (
                            (1723).to_bytes(4, "big") +
                            (1732).to_bytes(4, "big")
                        ),
                        "serial-number": "do-not-export",
                        "ranges": {
                            "safe": 1,
                            "device-serial-number": "also-do-not-export",
                        },
                        "IORegistryEntryChildren": [{
                            "IORegistryEntryName": "pci-bridge0",
                            "IORegistryEntryChildren": [{
                                "IORegistryEntryName": "wlan",
                                "reg": (0x100).to_bytes(4, "big"),
                            }],
                        }, {
                            "IORegistryEntryName": "pci-bridge1",
                            "reg": (0x800).to_bytes(4, "big"),
                            "IORegistryEntryChildren": [{
                                "IORegistryEntryName": "pcie-sdreader",
                                "vendor-id": (0x17a0).to_bytes(4, "big"),
                                "device-id": (0x9755).to_bytes(4, "big"),
                            }],
                        }],
                    }, {
                        "IORegistryEntryName": "dart-apcie0",
                        "compatible": b"dart,t8110\0",
                        "reg": (
                            (0x4).to_bytes(4, "big") +
                            (0x10000000).to_bytes(4, "big")
                        ),
                    }, {
                        "IORegistryEntryName": "dart-apcie1",
                        "compatible": b"dart,t8110\0",
                        "reg": (
                            (0x4).to_bytes(4, "big") +
                            (0x11000000).to_bytes(4, "big")
                        ),
                        "IORegistryEntryChildren": [{
                            "IORegistryEntryName": "mapper-apcie1",
                            "iommus": (1).to_bytes(4, "big"),
                        }],
                    }, {
                        "IORegistryEntryName": "dart-jpeg0",
                        "reg": (0xdeadbeef).to_bytes(4, "big"),
                    }, {
                        "IORegistryEntryName": "smc-gpio0",
                        "phandle": (0x1234).to_bytes(4, "big"),
                    }, {
                        "IORegistryEntryName": "unrelated",
                        "reg": b"x" * 300,
                    }],
                }, {
                    "IORegistryEntryName": "pcie-sdreader-helper",
                    "pwren-gpios": (
                        (0x1234).to_bytes(4, "big") +
                        (25).to_bytes(4, "big")
                    ),
                }],
            }],
        }]

    def test_profile_is_narrow(self):
        nodes = REPORT.extract_nodes(self.sample_archive(), self.profile())
        names = [node["name"] for node in nodes]
        self.assertIn("apcie0", names)
        self.assertIn("pci-bridge1", names)
        self.assertIn("pcie-sdreader", names)
        self.assertIn("dart-apcie0", names)
        self.assertIn("dart-apcie1", names)
        self.assertIn("mapper-apcie1", names)
        self.assertIn("smc-gpio0", names)
        self.assertIn("pcie-sdreader-helper", names)
        self.assertNotIn("pci-bridge0", names)
        self.assertNotIn("wlan", names)
        self.assertNotIn("dart-jpeg0", names)
        self.assertNotIn("unrelated", names)

    def test_duplicate_paths_are_collapsed(self):
        archive = [{
            "IORegistryEntryName": "Root",
            "IORegistryEntryChildren": [{
                "IORegistryEntryName": "apcie0",
                "reg": (1).to_bytes(4, "big"),
            }, {
                "IORegistryEntryName": "apcie0",
                "reg": (1).to_bytes(4, "big"),
            }],
        }]
        nodes = REPORT.extract_nodes(archive, self.profile())
        self.assertEqual([node["path"] for node in nodes], ["/Root/apcie0"])

    def test_decodes_cells_and_compatible_strings(self):
        nodes = REPORT.extract_nodes(self.sample_archive(), self.profile())
        by_name = {node["name"]: node for node in nodes}
        self.assertEqual(by_name["apcie0"]["compatible"], ["apcie,t6040"])
        reg = by_name["apcie0"]["properties"]["reg"]
        self.assertEqual(reg["cells"][0], "0x0000001c")
        self.assertEqual(reg["cells"][1], "0xb0000000")

    def test_sensitive_keys_are_redacted_at_every_depth(self):
        nodes = REPORT.extract_nodes(self.sample_archive(), self.profile())
        apcie = next(node for node in nodes if node["name"] == "apcie0")
        self.assertNotIn("serial-number", apcie["properties"])
        self.assertEqual(apcie["properties"]["ranges"], {"safe": 1})
        self.assertNotIn("do-not-export", repr(apcie))

    def test_large_binary_values_are_omitted_without_hashing(self):
        result = REPORT.sanitize_value("reg", b"x" * 300)
        self.assertEqual(result, {"bytes": 300, "omitted": True})
        self.assertNotIn("sha256", result)

    def test_unknown_types_are_omitted(self):
        class Unknown:
            pass

        self.assertIs(REPORT.sanitize_value("reg", Unknown()), REPORT.OMIT)

    def test_ioreg_requests_properties(self):
        calls = []
        payload = plistlib.dumps(self.sample_archive())

        def fake_run(command):
            calls.append(command)
            return payload

        archive = REPORT.read_iodevicetree(run=fake_run)
        self.assertIsNotNone(archive)
        self.assertEqual(calls[0], [
            "/usr/sbin/ioreg", "-p", "IODeviceTree", "-l", "-a"
        ])

    def test_archive_mode_never_claims_boot_readiness(self):
        report = REPORT.collect(
            "j614s-pcie-sd",
            archive=self.sample_archive(),
            system="Linux",
        )
        self.assertEqual(report["status"], "evidence_collected")
        self.assertFalse(report["ready_to_boot"])
        self.assertTrue(report["read_only"])
        self.assertGreater(report["node_count"], 0)
        self.assertEqual(report["property_nodes"], report["node_count"])

    def test_empty_property_capture_is_rejected(self):
        archive = [{
            "IORegistryEntryName": "Root",
            "IORegistryEntryChildren": [{
                "IORegistryEntryName": "apcie0",
            }],
        }]
        report = REPORT.collect(
            "j614s-pcie-sd",
            archive=archive,
            system="Linux",
        )
        self.assertEqual(report["status"], "evidence_incomplete")
        self.assertEqual(report["property_nodes"], 0)
        self.assertFalse(report["ready_to_boot"])

    def test_live_mode_rejects_non_macos_hosts(self):
        report = REPORT.collect("j614s-pcie-sd", system="Linux")
        self.assertEqual(report["status"], "unsupported_host")
        self.assertFalse(report["ready_to_boot"])


if __name__ == "__main__":
    unittest.main()
