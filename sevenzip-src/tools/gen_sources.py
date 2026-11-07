"""Rebuild the vendored 7-Zip source lists from the upstream makefiles.

Two bundles are supported:

* ``alone7z`` -> ``Bundles/Alone7z``, console ``7zr``: 7z/LZMA/XZ/split only,
  the smallest engine (164 units). This is what M0 shipped.
* ``alone2``  -> ``Bundles/Alone2``,  console ``7zz``: the full single-file
  build (7z, zip, rar read, tar, gzip, bzip2, xz, ...), ~300 units. Needed
  for zip creation and rar browsing.

7-Zip builds each bundle from a list of object names (``$O/Name.o``) and
``7zip_gcc.mak`` maps every object to its translation unit with an explicit
rule. We reuse that mapping so cc-rs compiles exactly the same units as the
upstream Linux tools, minus the MinGW-only ones.

Usage: python tools/gen_sources.py [--bundle alone7z|alone2] [--root path] [--out path]
"""
from pathlib import Path
import argparse

HERE = Path(__file__).resolve().parent

BUNDLES = {
    # name -> (directory under CPP/7zip/Bundles, output file)
    "alone7z": ("Alone7z", "sources.txt"),
    "alone2": ("Alone2", "sources-full.txt"),
}

# Only present in the `IS_MINGW` branch of the makefile; on Linux/Android
# SYS_OBJS is just MyWindows.o and these are never compiled.
MINGW_ONLY = {"FileSystem", "Registry", "MemoryLock", "DLL", "DllSecur", "resource", "LzmaDecOpt"}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", choices=sorted(BUNDLES), default="alone7z")
    parser.add_argument("--root", default=None, help="path to the 7-Zip sources")
    parser.add_argument("--out", default=None, help="output list (defaults to vendor/<bundle> list)")
    args = parser.parse_args()

    root = Path(args.root).resolve() if args.root else (HERE.parent / "vendor" / "7zsrc").resolve()
    folder, default_out = BUNDLES[args.bundle]
    bundle = root / "CPP" / "7zip" / "Bundles" / folder
    mak = root / "CPP" / "7zip" / "7zip_gcc.mak"

    text = (bundle / "makefile.gcc").read_text(encoding="utf-8", errors="replace")
    if "Arc_gcc.mak" in text:
        # alone2 pulls the whole format set through Format7zF/Arc_gcc.mak.
        text += "\n" + (bundle / ".." / "Format7zF" / "Arc_gcc.mak").read_text(
            encoding="utf-8", errors="replace"
        )

    import re

    objects = re.findall(r"\$O/([A-Za-z0-9_+.]+)\.o", text)
    objects = [o for o in dict.fromkeys(objects) if o not in MINGW_ONLY]

    rules = {}
    for line in mak.read_text(encoding="utf-8", errors="replace").splitlines():
        m = re.match(r"\$O/([A-Za-z0-9_+.]+)\.o:\s*(\S+)", line)
        if m:
            rules[m.group(1)] = m.group(2)

    resolved, missing = [], []
    for name in objects:
        rel = rules.get(name)
        if not rel:
            missing.append((name, "no rule"))
            continue
        path = (bundle / rel).resolve()
        if not path.is_file():
            missing.append((name, f"missing {path}"))
            continue
        resolved.append(path.relative_to(root).as_posix())

    if missing:
        for name, why in missing:
            print(f"MISSING {name}: {why}")
        return 1

    out = Path(args.out) if args.out else (HERE.parent / "vendor" / default_out)
    out.write_text("\n".join(resolved) + "\n", encoding="utf-8", newline="\n")
    print(
        f"wrote {out.name} ({args.bundle}): {len(resolved)} units "
        f"({sum(p.endswith('.cpp') for p in resolved)} cpp / "
        f"{sum(p.endswith('.c') for p in resolved)} c)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
