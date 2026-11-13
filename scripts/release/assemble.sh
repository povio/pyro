#!/usr/bin/env bash
# Assembles and checks the GitHub release archives (docs/RELEASING.md).
#
#   scripts/release/assemble.sh VERSION COMMIT IN_DIR OUT_DIR
#
# IN_DIR holds the build outputs:
#   arm64/ x64/ fork/                   pyro and pyro-snapshot-compare of each build
#   jsc/                                the patched JavaScriptCore build linked into the fork
#                                       (lib/libJavaScriptCore.a, inputs/REVISION)
#   povio-pyro-VERSION.tgz   the packed client
#   jsc-licenses/                       from scripts/release/jsc-source.sh
# OUT_DIR must already hold pyro-jsc-source-vVERSION.tar.gz (from
# jsc-source.sh). Writes the three platform archives, copies the client, and
# writes SHA256SUMS.txt last, after every check passed. Binaries for the host's
# architecture are also run (version and fork capability); the others are only
# inspected.
set -euo pipefail

version="${1:?usage: $0 VERSION COMMIT IN_DIR OUT_DIR}"
commit="${2:?usage: $0 VERSION COMMIT IN_DIR OUT_DIR}"
in="$(cd "${3:?usage: $0 VERSION COMMIT IN_DIR OUT_DIR}" && pwd)"
out="$(cd "${4:?usage: $0 VERSION COMMIT IN_DIR OUT_DIR}" && pwd)"
root="$(cd "$(dirname "$0")/../.." && pwd)"
client="$in/povio-pyro-$version.tgz"
source_asset="pyro-jsc-source-v$version.tar.gz"
host_arch="$(uname -m)"

fail() {
    echo "error: $*" >&2
    exit 1
}
[[ -f "$client" ]] || fail "missing $client"
[[ -f "$out/$source_asset" ]] || fail "missing $out/$source_asset"

# Architecture, signature, JIT entitlement and dependencies of a binary.
check_binary() {
    local file="$1" want_arch="$2" dep
    [[ "$(lipo -archs "$file")" == "$want_arch" ]] || fail "$file is $(lipo -archs "$file"), want $want_arch"
    codesign --verify --strict "$file" || fail "$file: bad signature"
    codesign -d --entitlements - "$file" 2>/dev/null | grep -q com.apple.security.cs.allow-jit ||
        fail "$file: no JIT entitlement"
    while read -r dep; do
        case "$dep" in
            /System/* | /usr/lib/*) ;;
            *) fail "$file depends on $dep" ;;
        esac
    done < <(otool -L "$file" | tail -n +2 | awk '{print $1}')
}

for variant in arm64 x64 fork; do
    case "$variant" in
        arm64) platform=macos-arm64 arch=arm64 ;;
        x64) platform=macos-x64 arch=x86_64 ;;
        fork) platform=macos-arm64-fork arch=arm64 ;;
    esac
    base="pyro-rust-v$version-$platform"
    dir="$out/$base"
    rm -rf "${dir:?}"
    mkdir -p "$dir/pyro-client"
    cp "$in/$variant/pyro" "$in/$variant/pyro-snapshot-compare" "$dir/"
    cp "$root/LICENSE" "$root/NOTICE" "$dir/"
    tar xzf "$client" -C "$dir/pyro-client" --strip-components=1

    if [[ "$variant" == fork ]]; then
        mkdir -p "$dir/third-party/JavaScriptCore"
        cp -R "$in/jsc-licenses" "$dir/third-party/JavaScriptCore/licenses"
        rev="$(tr -d '[:space:]' < "$in/jsc/inputs/REVISION")"
        cat > "$dir/third-party/JavaScriptCore/SOURCE.txt" <<EOF
WebKit $rev
Patched JavaScriptCore is statically linked into pyro (lib/libJavaScriptCore.a of that build).
Matching already-patched source, original notices, patches and build instructions: $source_asset
https://github.com/povio/pyro/releases/download/v$version/$source_asset
EOF
        # Inside the browser: the system framework of the other variants must not be loaded beside it.
        if otool -L "$dir/pyro" | grep -q JavaScriptCore; then
            fail "the fork loads a JavaScriptCore library"
        fi
    fi
    check_binary "$dir/pyro" "$arch"
    check_binary "$dir/pyro-snapshot-compare" "$arch"
    if grep -rIl -e "$HOME" -e "Library/Caches" --include='*.json' --include='*.txt' "$dir" >/dev/null; then
        fail "$base mentions a build-machine path"
    fi

    if [[ "$arch" == "$host_arch" ]]; then
        report="$("$dir/pyro" version)"
        [[ "$(head -1 <<<"$report")" == "pyro $version" ]] || fail "$platform reports: $(head -1 <<<"$report")"
        want_fork=false
        [[ "$variant" == fork ]] && want_fork=true
        grep -q "fork: $want_fork" <<<"$report" || fail "$platform does not report fork: $want_fork"
    fi

    python3 - "$dir" "$variant" "$platform" "$version" "$commit" "$in" "$client" "$source_asset" <<'EOF'
import hashlib, json, os, sys
d, variant, platform, version, commit, src, client, source_asset = sys.argv[1:]
def digest(p):
    with open(p, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()
fork = variant == 'fork'
info = {
    'declaredSourceCommit': commit,
    'expectedForkCapability': fork,
    'inputEngineSHA256': digest(os.path.join(src, variant, 'pyro')),
    'javascriptCore': 'static-patched' if fork else 'system',
    'linkedJavaScriptCoreArchiveSHA256': digest(os.path.join(src, 'jsc/lib/libJavaScriptCore.a')) if fork else None,
    'packagedComparatorSHA256': digest(os.path.join(d, 'pyro-snapshot-compare')),
    'packagedEngineSHA256': digest(os.path.join(d, 'pyro')),
    'platform': platform,
    'sdkSHA256': digest(client),
    'sourceAsset': source_asset if fork else None,
    'validation': 'Packaging checks only; functional smoke evidence is recorded separately.',
    'version': version,
    'webkitRevision': open(os.path.join(src, 'jsc/inputs/REVISION')).read().strip() if fork else None,
}
with open(os.path.join(d, 'BUILD_INFO.json'), 'w') as f:
    json.dump(info, f, indent=2, sort_keys=True)
    f.write('\n')
EOF
    (cd "$out" && COPYFILE_DISABLE=1 tar czf "$base.tar.gz" "$base")
    echo "$out/$base.tar.gz"
done

# Linux: ICU bundled by scripts/release/bundle-icu-linux.sh in the build job and
# smoke-tested there; here only inspected (this script runs on macOS).
check_elf() {
    local file="$1" want="$2" desc
    desc="$(file -b "$file")"
    [[ "$desc" == ELF* && "$desc" == *"$want"* ]] || fail "$file is $desc, want ELF $want"
}
for arch in x64 arm64; do
    case "$arch" in
        x64) elf=x86-64 ;;
        arm64) elf=aarch64 ;;
    esac
    platform="linux-$arch"
    base="pyro-rust-v$version-$platform"
    dir="$out/$base"
    rm -rf "${dir:?}"
    mkdir -p "$dir/pyro-client" "$dir/third-party/JavaScriptCore" "$dir/third-party/ICU"
    cp -R "$in/$platform/." "$dir/"
    chmod +x "$dir/pyro" "$dir/pyro-snapshot-compare"
    if [[ -f "$dir/lib/ICU-COPYRIGHT.txt" ]]; then
        mv "$dir/lib/ICU-COPYRIGHT.txt" "$dir/third-party/ICU/COPYRIGHT.txt"
    fi
    cp "$root/LICENSE" "$root/NOTICE" "$dir/"
    tar xzf "$client" -C "$dir/pyro-client" --strip-components=1
    cp -R "$in/jsc-licenses" "$dir/third-party/JavaScriptCore/licenses"
    rev="$(tr -d '[:space:]' < "$in/jsc/inputs/REVISION")"
    {
        echo "WebKit $rev"
        echo "Patched JavaScriptCore is statically linked into pyro; the ICU libraries it loads are in lib/."
        echo "Matching already-patched source, original notices, patches and build instructions: $source_asset"
        echo "https://github.com/povio/pyro/releases/download/v$version/$source_asset"
    } > "$dir/third-party/JavaScriptCore/SOURCE.txt"
    for f in "$dir/pyro" "$dir/pyro-snapshot-compare" "$dir"/lib/libicuuc.so*; do
        check_elf "$f" "$elf"
    done
    [[ ! -e "$dir/lib/libJavaScriptCore.so" ]] || fail "$base ships a shared JavaScriptCore"

    if grep -rIl -e "$HOME" -e /home/runner --include='*.json' --include='*.txt' "$dir" >/dev/null; then
        fail "$base mentions a build-machine path"
    fi
    python3 "$root/scripts/release/linux-build-info.py" "$dir" "$platform" "$version" "$commit" "$in" "$client" "$source_asset" "$rev"
    (cd "$out" && COPYFILE_DISABLE=1 tar czf "$base.tar.gz" "$base")
    echo "$out/$base.tar.gz"
done

cp "$client" "$out/"
cd "$out"
shasum -a 256 "$source_asset" \
    "pyro-rust-v$version-macos-arm64-fork.tar.gz" \
    "pyro-rust-v$version-macos-arm64.tar.gz" \
    "pyro-rust-v$version-macos-x64.tar.gz" \
    "pyro-rust-v$version-linux-x64.tar.gz" \
    "pyro-rust-v$version-linux-arm64.tar.gz" \
    "povio-pyro-$version.tgz" > SHA256SUMS.txt
cat SHA256SUMS.txt
