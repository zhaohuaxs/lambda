#!/usr/bin/env python3
"""Generate the hostile archive fixtures Lambda's regression checklist needs.

Usage:
    python tools/make_hostile_fixtures.py [--out ../lambda-fixtures/hostile]
                                           [--many N]

`names.zip` carries entries that used to be parsed as 7-Zip switches or as a
list-file reference (`-pwrong`, `-oescape`, `@list.txt`), a wildcard name and
a symbolic-link entry. `many.zip` (optional) creates N tiny entries for the
listing cap; N defaults to 0.

The fixture is deliberately a ZIP: it can be produced without a 7-Zip binary
and is accepted by the vendored 7zz on both platforms.
"""

import argparse
import os
import zipfile


def names_zip(path):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("-pwrong", "password-looking name")
        archive.writestr("-oescape", "output-looking name")
        archive.writestr("@list.txt", "list-file-looking name")
        archive.writestr("*.txt", "wildcard name")
        archive.writestr("normal/a.txt", "a")
        link = zipfile.ZipInfo("link-to-passwd")
        link.external_attr = 0o120777 << 16
        archive.writestr(link, "../../../../etc/passwd")


def many_zip(path, count):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as archive:
        for index in range(count):
            archive.writestr(f"bulk/{index:06d}.txt", b"")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="../lambda-fixtures/hostile")
    parser.add_argument("--many", type=int, default=0)
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)
    names_zip(os.path.join(args.out, "names.zip"))
    if args.many > 0:
        many_zip(os.path.join(args.out, "many.zip"), args.many)


if __name__ == "__main__":
    main()
