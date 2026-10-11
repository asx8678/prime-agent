"""Remove build output that an unused Rust compiler left in a cargo target dir.

Usage: python3 sweep_target.py <target-dir> <version-to-keep> [--apply]
  e.g. python3 sweep_target.py target 1.99.0

Dry run unless --apply. Only target/debug is swept. Compiled artifacts are
grouped by the compiler hash in their cargo fingerprint; each group's
compiler version is read from one of its .rmeta/.rlib files.
Incremental-compilation sessions record their compiler in the dep-graph
header. Everything another compiler built is removed; the kept compiler's
cache is untouched. Run it when no cargo build is running.
"""
import collections
import glob
import json
import os
import re
import shutil
import sys

target, keep_version = sys.argv[1], sys.argv[2]
apply = "--apply" in sys.argv
debug = os.path.join(target, "debug")
GB = 1 << 30
VERSION = re.compile(rb"rustc (\d+\.\d+\.\d+)")


def size(path):
    if os.path.islink(path) or not os.path.isdir(path):
        return os.lstat(path).st_size
    return sum(os.lstat(os.path.join(root, name)).st_size for root, _, names in os.walk(path) for name in names)


def remove(path):
    if apply:
        if os.path.isdir(path) and not os.path.islink(path):
            shutil.rmtree(path)
        else:
            os.remove(path)


# Fingerprint hash -> compiler hash.
compiler_of = {}
for entry in os.scandir(os.path.join(debug, ".fingerprint")):
    for path in glob.glob(os.path.join(entry.path, "*.json")):
        try:
            compiler_of[entry.name.rsplit("-", 1)[-1]] = json.load(open(path))["rustc"]
            break
        except (OSError, ValueError, KeyError):
            pass

# Compiler hash -> version, read from one metadata file per compiler.
version_of = {}
for entry in os.scandir(os.path.join(debug, "deps")):
    if not entry.name.endswith((".rmeta", ".rlib")):
        continue
    compiler = compiler_of.get(entry.name.split(".", 1)[0].rsplit("-", 1)[-1])
    if compiler is None or compiler in version_of:
        continue
    with open(entry.path, "rb") as artifact:
        found = VERSION.search(artifact.read(1 << 16))
    if found:
        version_of[compiler] = found.group(1).decode()
print("compilers found:", sorted(set(version_of.values())))
keep = {compiler for compiler, version in version_of.items() if version == keep_version}
if not keep:
    sys.exit(f"no artifacts built by rustc {keep_version}; nothing removed")
stale = {fingerprint for fingerprint, compiler in compiler_of.items() if compiler not in keep}

freed = collections.Counter()
for folder in ("deps", "build", ".fingerprint"):
    for entry in os.scandir(os.path.join(debug, folder)):
        if entry.name.split(".", 1)[0].rsplit("-", 1)[-1] in stale:
            freed[folder] += size(entry.path)
            remove(entry.path)

incremental = os.path.join(debug, "incremental")
for crate in os.scandir(incremental) if os.path.isdir(incremental) else []:
    for session in list(os.scandir(crate.path)):
        try:
            header = open(os.path.join(session.path, "dep-graph.bin"), "rb").read(64)
        except OSError:
            continue
        found = re.search(rb"(\d+\.\d+\.\d+) \(", header)
        if found and found.group(1).decode() != keep_version:
            freed["incremental"] += size(session.path)
            remove(session.path)
    if apply and not os.listdir(crate.path):
        os.rmdir(crate.path)

print(("removed" if apply else "would remove"), {k: f"{v / GB:.1f} GB" for k, v in freed.items()},
      f"total {sum(freed.values()) / GB:.1f} GB")
