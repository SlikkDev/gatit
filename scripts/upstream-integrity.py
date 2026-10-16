#!/usr/bin/env python3
"""Bind the two unsigned upstream tools to the reviewed source and this build.

This is an integrity receipt, not a claim that an upstream executable is signed.
The signed installer and the release hash receipt protect its distribution.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMES = ("whisper-cli", "whisper-quantize")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def create(directory, receipt):
    lock = json.loads((ROOT / "packaging/windows/upstream.json").read_text())
    files = {f"{name}.exe": digest(directory / f"{name}-x86_64-pc-windows-msvc.exe") for name in NAMES}
    receipt.write_text(json.dumps({"source": lock, "commit": os.environ.get("GITHUB_SHA", "local-development"),
                                  "artifacts": files}, indent=2) + "\n")


def verify(directory, receipt, suffixed=False):
    data = json.loads(receipt.read_text())
    lock = json.loads((ROOT / "packaging/windows/upstream.json").read_text())
    if data["source"] != lock or data["commit"] != os.environ.get("GITHUB_SHA", "local-development"):
        raise ValueError("Upstream receipt has the wrong source or Gatit commit")
    if set(data["artifacts"]) != {f"{name}.exe" for name in NAMES}:
        raise ValueError("Only the two named Whisper binaries are exempt from signing")
    for name in NAMES:
        filename = name + ("-x86_64-pc-windows-msvc" if suffixed else "") + ".exe"
        matches = list(directory.rglob(filename))
        if len(matches) != 1 or matches[0].is_symlink():
            raise ValueError(f"Expected exactly one regular {filename}")
        if digest(matches[0]) != data["artifacts"][f"{name}.exe"]:
            raise ValueError(f"Upstream binary changed: {filename}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["create", "verify"])
    parser.add_argument("directory", type=Path)
    parser.add_argument("receipt", type=Path)
    parser.add_argument("--suffixed", action="store_true")
    args = parser.parse_args()
    if args.command == "create":
        create(args.directory, args.receipt)
    else:
        verify(args.directory, args.receipt, args.suffixed)
