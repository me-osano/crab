#!/usr/bin/env bash
# Enforces the crate boundaries from ROADMAP.md §5:
#   * crawm-core / crawm-layout / crawm-anim / crawm-config must not depend on
#     Smithay, Wayland, or any other system graphics/input crate;
#   * crawm-ipc must not depend on any other crawm-* crate.
#
# Run locally with `./scripts/check-boundaries.sh`; also run in CI.
set -euo pipefail

cd "$(dirname "$0")/.."

python3 - <<'PY'
import json
import subprocess
import sys

metadata = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], text=True
    )
)

packages = {pkg["name"]: pkg for pkg in metadata["packages"]}

PURE = ["crawm-core", "crawm-layout", "crawm-anim", "crawm-config"]
LEAF = ["crawm-ipc"]

# Substrings that indicate a Smithay/Wayland/system dependency.
FORBIDDEN = [
    "smithay", "wayland", "calloop", "drm", "gbm", "egl", "gles", "glow",
    "libseat", "udev", "x11", "winit", "pixman", "input",
]

errors = []
for name in PURE + LEAF:
    if name not in packages:
        errors.append(f"expected workspace member `{name}` is missing")
        continue

for name in PURE:
    for dep in packages[name]["dependencies"]:
        dep_name = dep["name"].lower()
        for needle in FORBIDDEN:
            if needle in dep_name:
                errors.append(f"{name} must stay pure but depends on `{dep['name']}`")
                break

for name in LEAF:
    for dep in packages[name]["dependencies"]:
        if dep["name"].startswith("crawm"):
            errors.append(f"{name} must be a leaf but depends on `{dep['name']}`")

if errors:
    print("boundary violations:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

print("boundaries OK: pure crates are pure, crawm-ipc is a leaf")
PY
