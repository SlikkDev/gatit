#!/usr/bin/env python3
"""Bind platform verification to exact artifact hashes; reject incomplete releases."""
import hashlib
import json
import os
from pathlib import Path
import sys

EXPECTED = {
    "Gatit-macos-apple-silicon": "notarized",
    "Gatit-macos-intel": "notarized",
    "Gatit-windows": "authenticode",
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(stem, status, publisher, filenames):
    files = [Path(f) for f in filenames if Path(f).is_file()]
    if not files or not publisher or EXPECTED.get(stem) != status:
        raise ValueError("Incomplete platform verification")
    data = {"status": status, "publisher": publisher, "commit": os.environ["GITHUB_SHA"],
            "artifacts": {f.name: digest(f) for f in files}}
    Path(stem + ".verification.json").write_text(json.dumps(data, indent=2) + "\n")


def verify(dist):
    allowed = set()
    for stem, status in EXPECTED.items():
        receipt = dist / (stem + ".verification.json")
        data = json.loads(receipt.read_text())
        if data["status"] != status or not data["publisher"] or data["commit"] != os.environ["GITHUB_SHA"]:
            raise ValueError(f"Invalid verification for {stem}")
        primary = stem + ".zip" if stem.startswith("Gatit-macos-") else "Gatit-windows-setup.exe"
        names = {primary}
        update = stem + ".app.tar.gz" if stem.startswith("Gatit-macos-") else primary
        if update + ".sig" in data["artifacts"]:
            names.update([update, update + ".sig"])
        if set(data["artifacts"]) != names:
            raise ValueError(f"Incomplete or unexpected artifacts for {stem}")
        for name, sha in data["artifacts"].items():
            path = dist / name
            if not path.is_file() or path.stat().st_size == 0 or digest(path) != sha:
                raise ValueError(f"Artifact changed after verification: {name}")
        allowed.update(names)
        allowed.add(receipt.name)
    if {p.name for p in dist.iterdir()} != allowed:
        raise ValueError("Unverified files in release directory")
    if len(list(dist.glob("*.sig"))) not in (0, 3):
        raise ValueError("Updater signatures must cover all three platforms or none")
    print("All three platforms verified; artifact hashes match.")


if __name__ == "__main__":
    try:
        if sys.argv[1] == "write":
            write(*sys.argv[2:5], sys.argv[5:])
        elif sys.argv[1] == "verify":
            verify(Path(sys.argv[2]))
        else:
            raise ValueError("Expected write or verify")
    except (KeyError, ValueError, OSError) as error:
        sys.exit(f"Release verification failed: {error}")
