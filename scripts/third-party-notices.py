#!/usr/bin/env python3
"""Write THIRD-PARTY-NOTICES.txt: the license and copyright notices of the
crates linked into Topos.exe and of the Rust standard library.

Run from the repository root after changing Cargo.lock:

    python3 scripts/third-party-notices.py

The linked crates are what `cargo tree` reports for the Windows targets
with normal edges only and no procedural macros (those run in the compiler,
so none of their code is in the program); both architectures link the same
set, which the script checks. Each crate's MIT license text comes from its
source in Cargo's registry (fetched with `cargo fetch`), and the standard
library's notices from the toolchain's documentation
(`rustc --print sysroot`). CI reruns this script and fails when the file
in the repository differs, and the test `notices_name_locked_versions`
fails when a listed version is no longer in Cargo.lock.
"""
import html
import os
import re
import subprocess
import sys
from pathlib import Path

TARGETS = ("x86_64-pc-windows-msvc", "i686-pc-windows-msvc")
SELF = "topos"


def linked_crates(target):
    lines = subprocess.check_output(
        [
            "cargo", "tree", "--offline", "--locked", "--target", target,
            "--edges", "normal,no-proc-macro", "--prefix", "none",
            "--format", "{p}",
        ],
        text=True,
    ).splitlines()
    crates = set()
    for line in lines:
        match = re.match(r"(\S+) v(\S+)", line)
        if match and match.group(1) != SELF:
            crates.add((match.group(1), match.group(2)))
    return crates


def crate_dir(name, version):
    home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for index in sorted((home / "registry" / "src").glob("*")):
        path = index / f"{name}-{version}"
        if path.is_dir():
            return path
    sys.exit(f"{name} {version} is not in Cargo's registry; run `cargo fetch` first")


def mit_text(path):
    files = {f.name.lower(): f for f in path.iterdir() if f.is_file()}
    for want in ("license-mit", "license-mit.md", "license-mit.txt", "license", "license.md",
                 "license.txt"):
        if want in files:
            text = files[want].read_text(errors="replace").strip()
            if "Permission is hereby granted" in text:
                return text
    sys.exit(f"no MIT license text in {path}")


def license_of(path):
    match = re.search(r'^license\s*=\s*"([^"]+)"', (path / "Cargo.toml").read_text(), re.M)
    return match.group(1) if match else "unknown"


def main():
    sets = [linked_crates(target) for target in TARGETS]
    if sets[0] != sets[1]:
        sys.exit(f"the targets link different crates: {sorted(sets[0] ^ sets[1])}")
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    docs = sysroot / "share" / "doc" / "rust"
    out = [
        "Third-party notices for Topos",
        "",
        "Topos includes the open-source software below. Each is used under the",
        "MIT license (most offer MIT or Apache-2.0; MIT is chosen), and its",
        "copyright and license notice is reproduced here as that license requires.",
        "",
    ]
    for name, version in sorted(sets[0]):
        path = crate_dir(name, version)
        lic = license_of(path)
        if "MIT" not in lic:
            sys.exit(f"{name} {version} is not available under MIT ({lic}); review it by hand")
        out += ["=" * 76, f"{name} {version} (license: {lic}; used under MIT)", "=" * 76, "",
                mit_text(path), ""]
    copyright = (docs / "COPYRIGHT-library.html").read_text()
    copyright = html.unescape(re.sub(r"<[^>]+>", "", copyright))
    copyright = "\n".join(line.rstrip() for line in copyright.splitlines() if line.strip())
    out += ["=" * 76, "The Rust Standard Library", "=" * 76, "", copyright, ""]
    for license_file in ("MIT.txt", "Unicode-3.0.txt"):
        out += ["-" * 76, license_file, "-" * 76, "",
                (docs / "licenses" / license_file).read_text().strip(), ""]
    Path("THIRD-PARTY-NOTICES.txt").write_text("\n".join(out) + "\n")
    print(f"wrote THIRD-PARTY-NOTICES.txt ({len(sets[0])} crates)")


if __name__ == "__main__":
    main()
