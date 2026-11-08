#!/usr/bin/env python3
"""Write a package/license inventory for Lambda.

Usage:
    python tools/make_sbom.py [--out docs/SBOM.csv]

`cargo metadata` carries each package's SPDX license expression. The vendored
7-Zip sources and the NDK's shared C++ runtime are not Cargo packages, so they
are appended explicitly; leaving them out would hide the two largest
statically/shared-linked third-party components.
"""

import argparse
import csv
import json
import os
import subprocess

NON_CARGO = (
    "7-Zip",
    "26.04",
    "LGPL-2.1-or-later",
    "vendored: sevenzip-src/vendor/7zsrc (unRAR restriction applies)",
)


def ndk_component():
    home = os.environ.get("ANDROID_NDK_HOME", "").rstrip("/\\")
    version = os.path.basename(home) if home else "28.2.13676358"
    return (
        "Android NDK libc++_shared",
        version or "28.2.13676358",
        "Apache-2.0 WITH LLVM-exception",
        "Android NDK (LLVM)",
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="docs/SBOM.csv")
    args = parser.parse_args()
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        encoding="utf-8",
    )
    metadata = json.loads(output)
    rows = [
        [
            package["name"],
            package["version"],
            package.get("license") or "",
            package.get("source") or "workspace",
        ]
        for package in metadata["packages"]
    ]
    rows.extend([list(NON_CARGO), list(ndk_component())])
    rows.sort(key=lambda row: (row[0].lower(), row[1]))
    with open(args.out, "w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["name", "version", "license", "source"])
        writer.writerows(rows)


if __name__ == "__main__":
    main()
