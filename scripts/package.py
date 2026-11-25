"""Package a release binary, documentation, and a SHA-256 checksum."""

import argparse
import hashlib
from pathlib import Path
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", help="Rust target triple")
    parser.add_argument("--binary", type=Path, help="Override the binary path")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    name = "repoo.exe" if "windows" in args.target else "repoo"
    binary = args.binary or root / "target" / args.target / "release" / name
    if not binary.is_file():
        parser.error(f"release binary not found: {binary}")
    dist = root / "dist"
    dist.mkdir(exist_ok=True)
    package = f"repoo-{args.target}"
    archive = dist / f"{package}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
        bundle.write(binary, f"{package}/{name}")
        for document in ("README.md", "LICENSE", "NOTICE", "docs/schema.md", "docs/schema.json"):
            bundle.write(root / document, f"{package}/{document}")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8"
    )
    print(archive)


if __name__ == "__main__":
    main()
