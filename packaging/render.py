#!/usr/bin/env python3
"""Render the Scoop, winget, and Chocolatey manifests for one release.

    python3 packaging/render.py 0.1.0 SHA256SUMS.txt out/

reads the release's SHA256SUMS.txt and writes:

    out/scoop/topos.json
    out/winget/TUIGSE.Topos*.yaml
    out/chocolatey/  (ready for `choco pack`)

The templates sit next to this script. {{VERSION}} is filled in, and for
each of the four downloads (X64_ZIP, X86_ZIP, X64_SETUP, X86_SETUP) its
{{..._URL}} and {{..._SHA256}}. The Microsoft Store package is not
rendered here: packaging/msix/build-msix.ps1 builds it in the release
workflow.
"""

import re
import shutil
import sys
from pathlib import Path

REPO = "https://github.com/bradwilliamson/Topos"
HERE = Path(__file__).resolve().parent
CHANNELS = ("scoop", "winget", "chocolatey")


def read_sums(path: Path) -> dict[str, str]:
    """File name -> lower-case SHA-256, from `sha256sum` output.

    A `*` before the name (sha256sum's binary-mode marker) is dropped;
    blank lines are skipped; anything else malformed stops.
    """
    sums = {}
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2 or not re.fullmatch(r"[0-9A-Fa-f]{64}", parts[0]):
            sys.exit(f"{path}:{number}: not a sha256sum line: {line!r}")
        sums[parts[1].strip().lstrip("*")] = parts[0].lower()
    return sums


def main() -> None:
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    version, sums_path, out = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        sys.exit(f"expected a plain X.Y.Z version, not {version!r}")
    tag = f"v{version}"
    downloads = {
        "X64_ZIP": f"Topos-{tag}-x86_64-windows.zip",
        "X86_ZIP": f"Topos-{tag}-x86-windows.zip",
        "X64_SETUP": f"Topos-{tag}-x86_64-setup.exe",
        "X86_SETUP": f"Topos-{tag}-x86-setup.exe",
    }

    sums = read_sums(sums_path)
    values = {"{{VERSION}}": version}
    for key, name in downloads.items():
        if name not in sums:
            sys.exit(f"{sums_path} has no checksum for {name}")
        values[f"{{{{{key}_URL}}}}"] = f"{REPO}/releases/download/{tag}/{name}"
        values[f"{{{{{key}_SHA256}}}}"] = sums[name]

    for channel in CHANNELS:
        dest = out / channel
        if dest.exists():
            shutil.rmtree(dest)
        for src in sorted((HERE / channel).rglob("*")):
            if src.is_dir():
                continue
            target = dest / src.relative_to(HERE / channel)
            target.parent.mkdir(parents=True, exist_ok=True)
            text = src.read_text(encoding="utf-8")
            for key, value in values.items():
                text = text.replace(key, value)
            if "{{" in text:
                sys.exit(f"{src} has a placeholder this script does not fill")
            target.write_text(text, encoding="utf-8")
            print(target)


if __name__ == "__main__":
    main()
