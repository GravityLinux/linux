# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Build the production versions macro without the kernel's other macros."""

import json
from pathlib import Path
import subprocess
import sys


def build(directory, rustc="rustc"):
    directory = Path(directory)
    source = Path(__file__).resolve().parents[5] / "rust/macros/versions.rs"
    wrapper = directory / "host_versions.rs"
    wrapper.write_text(
        "extern crate proc_macro;\n"
        f"#[path = {json.dumps(str(source))}] mod versions;\n"
        "#[proc_macro_attribute]\n"
        "pub fn versions(a: proc_macro::TokenStream, i: proc_macro::TokenStream)"
        " -> proc_macro::TokenStream { versions::versions(a, i) }\n"
    )
    suffix = ".dylib" if sys.platform == "darwin" else ".so"
    library = directory / ("libhost_versions" + suffix)
    subprocess.run(
        [rustc, "--edition=2021", "--crate-type=proc-macro", "--crate-name=host_versions",
         str(wrapper), "-o", str(library)],
        check=True,
    )
    kernel_wrapper = directory / "kernel_host.rs"
    kernel_wrapper.write_text("pub mod macros { pub use host_versions::versions; }\n")
    kernel_library = directory / "libkernel.rlib"
    macro_flags = ["--extern", f"host_versions={library}"]
    subprocess.run(
        [rustc, "--edition=2021", "--crate-type=rlib", "--crate-name=kernel",
         str(kernel_wrapper), *macro_flags, "-o", str(kernel_library)],
        check=True,
    )
    return [*macro_flags, "--extern", f"kernel={kernel_library}",
            "-L", f"dependency={directory}"]
