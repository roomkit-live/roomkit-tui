"""Check production dependency boundaries from Cargo's resolved graph."""

import json
import subprocess

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
))
packages = {package["id"]: package for package in metadata["packages"]}
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
protocol = next(p for p in packages.values() if p["name"] == "roomkit-tui-protocol")
allowed = {"serde", "serde_json"}
for dep in protocol["dependencies"]:
    if dep["kind"] != "dev" and dep["name"] not in allowed:
        raise SystemExit(f"Unexpected protocol dependency: {dep['name']}")

tui = next(p for p in packages.values() if p["name"] == "roomkit-tui")
pending = [tui["id"]]
seen = set()
while pending:
    current = pending.pop()
    if current in seen:
        continue
    seen.add(current)
    name = packages[current]["name"]
    if name in {"roomkit", "roomkit-core", "roomkit-ai", "roomkit-openai", "reqwest"} or name.startswith("roomkit-tools"):
        raise SystemExit(f"Terminal depends on engine, provider or domain tools: {name}")
    for dep in nodes[current]["deps"]:
        if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
            pending.append(dep["pkg"])
print("Dependency boundaries verified: pure protocol; terminal independent of engines and tools.")

transport = next(p for p in packages.values() if p["name"] == "roomkit-tui-transport")
if any(d["kind"] != "dev" and d["name"] != "roomkit-tui-protocol" for d in transport["dependencies"]):
    raise SystemExit("The in-process transport must remain independent of a terminal or runtime")
adapter = next(p for p in packages.values() if p["name"] == "roomkit-tui-engine")
if any(d["name"] in {"roomkit-tui", "crossterm", "ratatui"} for d in adapter["dependencies"]):
    raise SystemExit("The engine adapter must not depend on terminal rendering")
engine_sources = {p["source"] for p in packages.values() if p["name"] in {"roomkit", "roomkit-core", "roomkit-ai", "roomkit-openai", "roomkit-sqlite"}}
if len(engine_sources) != 1 or not all(s and "?rev=" in s for s in engine_sources):
    raise SystemExit("All engine crates must use one pinned Git revision")
print("Adapter and shared transport boundaries verified; engine source pinned.")
