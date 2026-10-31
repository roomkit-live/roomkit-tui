"""Inspect actual Cargo package file lists, including local untracked files."""

from pathlib import PurePosixPath
import subprocess

allowed_root = {"Cargo.toml", "Cargo.toml.orig", "Cargo.lock", "README.md", "LICENSE", ".cargo_vcs_info.json"}
for package in ["roomkit-tui-protocol", "roomkit-tui-transport", "roomkit-tui", "roomkit-tui-engine"]:
    result = subprocess.run(
        ["cargo", "package", "--locked", "--list", "--allow-dirty", "-p", package],
        capture_output=True, text=True, check=True,
    )
    names = result.stdout.splitlines()
    for name in names:
        path = PurePosixPath(name)
        if name not in allowed_root and (not path.parts or path.parts[0] != "src"):
            raise SystemExit(f"Unexpected package content in {package}: {name}")
    if "LICENSE" not in names or "src/lib.rs" not in names:
        raise SystemExit(f"Missing package essentials: {package}")
    print(f"{package}: {len(names)} permitted package files; no local planning or credentials.")
