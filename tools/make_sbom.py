#!/usr/bin/env python3
"""Write a package/license inventory for the Rust half of Lambda.

Usage:
    python tools/make_sbom.py [--out docs/SBOM.csv]

`cargo metadata` carries each package's SPDX license expression; this is the
information an SBOM needs. The 7-Zip sources and the NDK runtime are listed in
docs/THIRD-PARTY-NOTICES.md instead, because they are not Cargo packages.
"""

import argparse
import csv
import json
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="docs/SBOM.csv")
    args = parser.parse_args()
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        encoding="utf-8",
    )
    metadata = json.loads(output)
    packages = sorted(metadata["packages"], key=lambda item: (item["name"], item["version"]))
    with open(args.out, "w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(["name", "version", "license", "source"])
        for package in packages:
            writer.writerow(
                [
                    package["name"],
                    package["version"],
                    package.get("license") or "",
                    package.get("source") or "workspace",
                ]
            )


if __name__ == "__main__":
    main()
