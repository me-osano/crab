#!/usr/bin/env bash
# Enforces the crate/module boundaries from ROADMAP.md §5:
#   * crab-ipc (workspace crate) must not depend on any other crab-* crate;
#   * the pure modules src/{core,config,layout,animation} of the crab package
#     must not mention Smithay/Wayland/system code — checked by scanning their
#     sources, since cargo metadata cannot isolate modules of one package;
#   * those pure modules may only depend *downward* (core → nothing;
#     config/layout/animation → core), and src/lib.rs may expose only them.
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

LEAF = ["crab-ipc"]

# Pure modules of the `crab` package (enforced by source scan, see below), and
# the *downward-only* `crate::` targets each is allowed to name. Anything not
# listed (another pure module, backend, cli, logging, …) is an upward or sideways
# edge and therefore a violation.
LAYERS = {
    "src/core": {"core"},
    "src/config": {"core", "config"},
    "src/layout": {"core", "layout"},
    "src/animation": {"core", "animation"},
}
PURE_MODULES = {"core", "config", "layout", "animation"}

# Code-level needles for the source scan: prefixes/symbols that only appear
# when actually linking against forbidden systems code. Deliberately
# case-sensitive and word-bounded so comments like "no Smithay" and identifiers
# like `input`/`LayoutInput` do not false-positive.
FORBIDDEN_CODE = [
    r"\bsmithay", r"\bwayland", r"\bcalloop", r"\bwinit", r"\blibinput",
    r"\blibseat", r"\bgbm", r"\bdrm", r"\budev", r"\bpixman", r"\bx11",
    r"\begl\b", r"\bgles\b", r"\bglow\b",
]


def crate_refs(code: str):
    """Yield the first segment of every `crate::…` / `crate::{…}` path.

    Best-effort: only literal `crate::` paths are seen, so the point of the check
    is to catch the common mistake (e.g. `core` reaching for `config`) early.
    """
    for brace in re.finditer(r"\bcrate::\{([^}]*)\}", code):
        for item in brace.group(1).split(","):
            name = item.strip().split("::")[0].strip()
            if name:
                yield name
    code = re.sub(r"\bcrate::\{[^}]*\}", "", code)
    for path in re.finditer(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*)", code):
        yield path.group(1)


errors = []
for name in LEAF:
    if name not in packages:
        errors.append(f"expected workspace member `{name}` is missing")
        continue
    for dep in packages[name]["dependencies"]:
        if dep["name"].startswith("crab"):
            errors.append(f"{name} must be a leaf but depends on `{dep['name']}`")

# The library target must expose the pure modules and nothing else, so the bin's
# Smithay glue (backends, handlers, …) cannot leak into the reusable surface.
lib = pathlib.Path("src/lib.rs")
if not lib.is_file():
    errors.append("src/lib.rs is missing — the pure modules need a library target")
else:
    for lineno, line in enumerate(lib.read_text().splitlines(), start=1):
        code = line.split("//", 1)[0]
        declared = re.match(
            r"\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", code
        )
        if declared and declared.group(1) not in PURE_MODULES:
            errors.append(
                f"src/lib.rs:{lineno}: `mod {declared.group(1)};` — the library "
                f"must expose only {sorted(PURE_MODULES)}"
            )

# Source scan for the in-tree pure modules.
for module_dir, allowed in LAYERS.items():
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
            for target in crate_refs(code):
                if target not in allowed:
                    errors.append(
                        f"{source}:{lineno}: `crate::{target}` — {module_dir} may "
                        f"only depend downward ({'/'.join(sorted(allowed))})"
                    )

if errors:
    print("boundary violations:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

print(
    "boundaries OK: src/{core,config,layout,animation} are Smithay-free and "
    "depend downward only, src/lib.rs exposes just them, crab-ipc is a leaf"
)
PY
