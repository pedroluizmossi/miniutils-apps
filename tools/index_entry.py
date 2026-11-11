#!/usr/bin/env python3
"""Monta a entrada de um app para o registry.json da loja do MiniUtils.

Lê o manifest.toml do app e as somas SHA-256 dos executáveis publicados num Release e
imprime o JSON pronto para colar na lista "plugins" do índice da loja (depois, assine o
índice de novo com `miniutils --loja-assinar`).

    python3 tools/index_entry.py --app repos --tag repos-v0.1.0 \\
        --commit <sha> --repo https://github.com/<você>/miniutils-apps --dist dist

Em `--dist` ficam os executáveis com o nome que o workflow de release dá
(`<app>-linux-x86_64`, `<app>-windows-x86_64.exe`) e um `<arquivo>.sha256` ao lado de cada.
"""

import argparse
import json
import pathlib
import re
import sys
import tomllib
from typing import NoReturn

# sistema no manifesto ([app] linux/windows) -> nome do arquivo publicado
ASSETS = {"linux": "{app}-linux-x86_64", "windows": "{app}-windows-x86_64.exe"}


def fail(msg: str) -> NoReturn:
    print(f"index_entry: {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--app", required=True, help="pasta do app (ex.: repos)")
    p.add_argument("--tag", required=True, help="tag do Release (ex.: repos-v0.1.0)")
    p.add_argument("--commit", required=True, help="SHA completo do commit da tag")
    p.add_argument("--repo", required=True, help="https://github.com/<dono>/<repositório>")
    p.add_argument("--dist", required=True, help="pasta com os executáveis e os .sha256")
    p.add_argument("--author", default="", help="autor (padrão: o dono do repositório)")
    a = p.parse_args()

    root = pathlib.Path(__file__).resolve().parent.parent
    manifest = tomllib.loads((root / a.app / "manifest.toml").read_text(encoding="utf-8"))
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    license_ = workspace.get("workspace", {}).get("package", {}).get("license", "")

    if not re.fullmatch(r"[0-9a-f]{40}", a.commit):
        fail(f"--commit precisa ser o SHA completo (40 caracteres hex), não {a.commit!r}")
    version = a.tag.rsplit("-v", 1)[-1]
    app = manifest.get("app")
    if not app:
        fail(f"{a.app}/manifest.toml não tem a seção [app]")
    if manifest.get("permissions"):
        fail("app externo não declara [permissions] (a loja o anuncia como acesso total)")

    repo = a.repo.rstrip("/")
    dist = pathlib.Path(a.dist)
    binaries = {}
    for os_name in ("linux", "windows"):
        if os_name not in app:
            continue
        asset = ASSETS[os_name].format(app=a.app)
        sha_file = dist / f"{asset}.sha256"
        if not (dist / asset).is_file() or not sha_file.is_file():
            fail(f"o manifesto declara {os_name}, mas falta {asset} (ou {asset}.sha256) em {dist}")
        sha = sha_file.read_text(encoding="utf-8").split()[0].lower()
        if not re.fullmatch(r"[0-9a-f]{64}", sha):
            fail(f"{sha_file}: SHA-256 inválido")
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
