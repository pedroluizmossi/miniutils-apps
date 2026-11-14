#!/usr/bin/env python3
"""Builds an app's entry for the MiniUtils store registry.json.

Reads the app's manifest.toml and the SHA-256 sums of the executables published in a
Release and prints the JSON ready to paste into the "plugins" list of the store index
(then sign the index again with `miniutils --store-sign`).

    python3 tools/index_entry.py --app repos --tag repos-v0.1.0 \\
        --commit <sha> --repo https://github.com/<you>/miniutils-apps --dist dist

`--dist` holds the executables with the names the release workflow gives them
(`<app>-linux-x86_64`, `<app>-windows-x86_64.exe`) and a `<file>.sha256` next to each one.

Only the executables are release assets: the store installs the rest of the app folder
(manifest.toml, view.slint, i18n/*.po, …) from the pinned commit, so nothing here lists
those files.
"""

import argparse
import json
import pathlib
import re
import sys
import tomllib
from typing import NoReturn

# system in the manifest ([app] linux/windows) -> name of the published file
ASSETS = {"linux": "{app}-linux-x86_64", "windows": "{app}-windows-x86_64.exe"}


def fail(msg: str) -> NoReturn:
    print(f"index_entry: {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--app", required=True, help="app folder (e.g. repos)")
    p.add_argument("--tag", required=True, help="Release tag (e.g. repos-v0.1.0)")
    p.add_argument("--commit", required=True, help="full SHA of the tag's commit")
    p.add_argument("--repo", required=True, help="https://github.com/<owner>/<repository>")
    p.add_argument("--dist", required=True, help="folder with the executables and the .sha256 files")
    p.add_argument("--author", default="", help="author (default: the repository owner)")
    a = p.parse_args()

    root = pathlib.Path(__file__).resolve().parent.parent
    manifest = tomllib.loads((root / a.app / "manifest.toml").read_text(encoding="utf-8"))
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    license_ = workspace.get("workspace", {}).get("package", {}).get("license", "")

    if not re.fullmatch(r"[0-9a-f]{40}", a.commit):
        fail(f"--commit must be the full SHA (40 hex characters), not {a.commit!r}")
    version = a.tag.rsplit("-v", 1)[-1]
    app = manifest.get("app")
    if not app:
        fail(f"{a.app}/manifest.toml has no [app] section")
    if manifest.get("permissions"):
        fail("an external app does not declare [permissions] (the store lists it as full access)")

    repo = a.repo.rstrip("/")
    dist = pathlib.Path(a.dist)
    binaries = {}
    for os_name in ("linux", "windows"):
        if os_name not in app:
            continue
        asset = ASSETS[os_name].format(app=a.app)
        sha_file = dist / f"{asset}.sha256"
        if not (dist / asset).is_file() or not sha_file.is_file():
            fail(f"the manifest declares {os_name}, but {asset} (or {asset}.sha256) is missing in {dist}")
        sha = sha_file.read_text(encoding="utf-8").split()[0].lower()
        if not re.fullmatch(r"[0-9a-f]{64}", sha):
            fail(f"{sha_file}: invalid SHA-256")
        binaries[os_name] = {"url": f"{repo}/releases/download/{a.tag}/{asset}", "sha256": sha}

    entry = {
        "id": manifest["id"],
        "name": manifest["name"],
        "description": manifest.get("description", ""),
        "author": a.author or repo.split("/")[-2],
        "license": license_,
        "version": version,
        "repo": repo,
        "path": a.app,
        "commit": a.commit,
        "uses": manifest.get("uses", []),
        "full_access": True,
        "binaries": binaries,
    }
    print(json.dumps(entry, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
