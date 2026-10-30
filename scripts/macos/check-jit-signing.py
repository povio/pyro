#!/usr/bin/env python3
"""Exercise the repository's release profile/linker with debug info disabled.

Uses a tiny temporary executable so this regression does not rebuild the browser.
Run on macOS with cargo/rustc/codesign available. No system/signing identity setup.
"""
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile

if sys.platform != "darwin":
    raise SystemExit("This check requires macOS codesign.")
root = Path(__file__).resolve().parents[2]
manifest = (root / "Cargo.toml").read_text()
profile = re.search(r"(?ms)^\[profile\.release\]\n.*?(?=^\[|\Z)", manifest)
if profile is None:
    raise SystemExit("Repository release profile missing.")
with tempfile.TemporaryDirectory(prefix="lp-jit-signing-") as directory:
    work = Path(directory)
    (work / "src").mkdir()
    (work / ".cargo").mkdir()
    (work / "src/main.rs").write_text('fn main() { println!("signature-check"); }\n')
    (work / "Cargo.toml").write_text(
        '[package]\nname="lp-jit-signing-check"\nversion="0.0.0"\nedition="2024"\n\n'
        + profile.group(0)
    )
    # Absolute here: the temporary crate is not in the repository's workspace.
    flag = "link-arg=-B" + str(root / "scripts/macos/linker")
    # TOML basic strings and JSON strings share the escaping needed for this path.
    (work / ".cargo/config.toml").write_text("\n".join(
        f'[target.{target}]\nrustflags=["-C", {json.dumps(flag)}]\n'
        for target in ["aarch64-apple-darwin", "x86_64-apple-darwin"]
    ))
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(work / "target")
    env["CARGO_PROFILE_RELEASE_DEBUG"] = "0"
    # Check the repository profile, not a caller's explicit strip override.
    env.pop("CARGO_PROFILE_RELEASE_STRIP", None)
    subprocess.run(["cargo", "build", "--release", "--offline"], cwd=work, env=env, check=True, timeout=120)
    binary = work / "target/release/lp-jit-signing-check"
    result = subprocess.run(
        ["codesign", "--display", "--entitlements", "-", "--xml", str(binary)],
        capture_output=True, check=True, timeout=10,
    )
    if not result.stdout.strip():
        raise SystemExit("FAIL: release DEBUG=0 lost all entitlements after linking.")
    entitlements = plistlib.loads(result.stdout)
    if entitlements.get("com.apple.security.cs.allow-jit") is not True:
        raise SystemExit("FAIL: release DEBUG=0 lost allow-jit after linking.")
    subprocess.run(["codesign", "--verify", "--strict", str(binary)], check=True, timeout=10)
    output = subprocess.check_output([str(binary)], text=True, timeout=10).strip()
    if output != "signature-check":
        raise SystemExit("FAIL: signed executable did not run correctly.")
print("PASS: release DEBUG=0 keeps a valid allow-jit signature.")
