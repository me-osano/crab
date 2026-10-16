#!/usr/bin/env bash
# Enforces the crate/module boundaries from ROADMAP.md §5:
#   * crawm-ipc (workspace crate) must not depend on any other crawm-* crate;
#   * the pure modules src/{core,config,layout,animation} of the crawm package
#     must not mention Smithay/Wayland/system code — checked by scanning their
#     sources, since cargo metadata cannot isolate modules of one package.
#
# Run locally with `./scripts/check-boundaries.sh`; also run in CI.
set -euo pipefail

cd "$(dirname "$0")/.."

python3 - <<'PY'
import json
import pathlib
import re
import subprocess
import sys

metadata = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], text=True
    )
)

packages = {pkg["name"]: pkg for pkg in metadata["packages"]}

LEAF = ["crawm-ipc"]

# Pure modules of the `crawm` package (enforced by source scan, see below).
PURE_DIRS = ["src/core", "src/config", "src/layout", "src/animation"]

# Code-level needles for the source scan: prefixes/symbols that only appear
# when actually linking against forbidden systems code. Deliberately
# case-sensitive and word-bounded so comments like "no Smithay" and identifiers
# like `input`/`LayoutInput` do not false-positive.
FORBIDDEN_CODE = [
    r"\bsmithay", r"\bwayland", r"\bcalloop", r"\bwinit", r"\blibinput",
    r"\blibseat", r"\bgbm", r"\bdrm", r"\budev", r"\bpixman", r"\bx11",
    r"\begl\b", r"\bgles\b", r"\bglow\b",
]

errors = []
for name in LEAF:
    if name not in packages:
        errors.append(f"expected workspace member `{name}` is missing")
        continue
    for dep in packages[name]["dependencies"]:
        if dep["name"].startswith("crawm"):
            errors.append(f"{name} must be a leaf but depends on `{dep['name']}`")

# Source scan for the in-tree pure modules.
for module_dir in PURE_DIRS:
    directory = pathlib.Path(module_dir)
    if not directory.is_dir():
        errors.append(f"expected pure module directory `{module_dir}` is missing")
        continue
    for source in sorted(directory.rglob("*.rs")):
        for lineno, line in enumerate(source.read_text().splitlines(), start=1):
            code = line.split("//", 1)[0]  # drop code comments and doc comments
            for needle in FORBIDDEN_CODE:
                if re.search(needle, code):
                    errors.append(
                        f"{source}:{lineno}: `{needle}` found — "
                        f"{module_dir} must stay Smithay/Wayland-free"
                    )
                    break

if errors:
    print("boundary violations:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

print(
    "boundaries OK: src/{core,config,layout,animation} Smithay-free, "
    "crawm-ipc is a leaf"
)
PY
