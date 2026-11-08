#!/usr/bin/env python3
"""Generate the hostile archive fixtures Lambda's regression checklist needs.

Usage:
    python tools/make_hostile_fixtures.py [--out ../lambda-fixtures/hostile]
                                           [--many N] [--bomb-mb N]
                                           [--depth N] [--png-mp N]

`names.zip` carries entries that used to be parsed as 7-Zip switches or as a
list-file reference (`-pwrong`, `-oescape`, `@list.txt`), a wildcard name and
a symbolic-link entry. `deep.zip` has one 1000-level nested path; `huge.png`
declares a 50 MP image in its header; `old-encoding.zip` has a GBK name with
no UTF-8 flag. `bomb.zip` (optional) is a real high-ratio archive of
`--bomb-mb` MiB zeros; `many.zip` (optional) creates N tiny entries for the
listing cap.

The fixture is deliberately a ZIP: it can be produced without a 7-Zip binary
and is accepted by the vendored 7zz on both platforms.
"""

import argparse
import os
import struct
import zlib
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


def deep_zip(path, depth):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as archive:
        archive.writestr("/".join(["d"] * depth) + "/leaf.txt", b"deep")


def huge_png(path, megapixels):
    width = 10000
    height = max(1, megapixels * 1_000_000 // width)

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    with open(path, "wb") as handle:
        handle.write(b"\x89PNG\r\n\x1a\n")
        handle.write(chunk(b"IHDR", ihdr))
        handle.write(chunk(b"IEND", b""))


def old_encoding_zip(path):
    # Python's zipfile always writes UTF-8 names, so build the minimal
    # single-entry ZIP by hand to keep the original GBK bytes and flags=0.
    name = "汉字.txt".encode("gbk")
    data = b"legacy name"
    crc = zlib.crc32(data) & 0xFFFFFFFF
    local = (
        struct.pack(
            "<IHHHHHIIIHH",
            0x04034B50,
            20,
            0,
            0,
            0,
            0,
            crc,
            len(data),
            len(data),
            len(name),
            0,
        )
        + name
        + data
    )
    central = (
        struct.pack(
            "<IHHHHHHIIIHHHHHII",
            0x02014B50,
            20,
            20,
            0,
            0,
            0,
            0,
            crc,
            len(data),
            len(data),
            len(name),
            0,
            0,
            0,
            0,
            0,
            0,
        )
        + name
    )
    eocd = struct.pack(
        "<IHHHHIIH",
        0x06054B50,
        0,
        0,
        1,
        1,
        len(central),
        len(local),
        0,
    )
    with open(path, "wb") as handle:
        handle.write(local + central + eocd)


def bomb_zip(path, megabytes):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        with archive.open("zeros.bin", "w") as handle:
            chunk = b"\0" * (1024 * 1024)
            for _ in range(megabytes):
                handle.write(chunk)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="../lambda-fixtures/hostile")
    parser.add_argument("--many", type=int, default=0)
    parser.add_argument("--bomb-mb", type=int, default=0)
    parser.add_argument("--depth", type=int, default=1000)
    parser.add_argument("--png-mp", type=int, default=50)
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)
    names_zip(os.path.join(args.out, "names.zip"))
    old_encoding_zip(os.path.join(args.out, "old-encoding.zip"))
    if args.depth > 0:
        deep_zip(os.path.join(args.out, "deep.zip"), args.depth)
    if args.png_mp > 0:
        huge_png(os.path.join(args.out, "huge.png"), args.png_mp)
    if args.bomb_mb > 0:
        bomb_zip(os.path.join(args.out, "bomb.zip"), args.bomb_mb)
    if args.many > 0:
        many_zip(os.path.join(args.out, "many.zip"), args.many)


if __name__ == "__main__":
    main()
