#!/usr/bin/env bash
# Builds the release's JavaScriptCore source asset: the sparse WebKit tree at
# crates/lp-jsc-sys/webkit/REVISION with every patch applied, the recorded
# inputs, the build script, BUILDING.md (docs/RELEASING.md) and a manifest with
# a hash per file. Also copies the upstream license files the fork archive
# ships under third-party/JavaScriptCore/licenses.
#
#   scripts/release/jsc-source.sh VERSION COMMIT OUT_DIR
#
# Writes OUT_DIR/lightningpanda-jsc-source-vVERSION.tar.gz and
# OUT_DIR/jsc-licenses/. Makes its own checkout in a temporary directory, so it
# does not depend on (or disturb) the tree scripts/jsc/build.sh compiles.
# Needs git, python3 and network access to github.com/WebKit/WebKit.
set -euo pipefail

version="${1:?usage: $0 VERSION COMMIT OUT_DIR}"
commit="${2:?usage: $0 VERSION COMMIT OUT_DIR}"
out="$(mkdir -p "${3:?usage: $0 VERSION COMMIT OUT_DIR}" && cd "$3" && pwd)"
root="$(cd "$(dirname "$0")/../.." && pwd)"
inputs="$root/crates/lp-jsc-sys/webkit"
rev="$(tr -d '[:space:]' < "$inputs/REVISION")"
name="lightningpanda-jsc-source-v$version"

work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT
src="$work/webkit-src"

# The same sparse, blobless checkout as scripts/jsc/build.sh.
git -C "$work" init -q webkit-src
git -C "$src" remote add origin https://github.com/WebKit/WebKit.git
git -C "$src" sparse-checkout set --no-cone '/*' '!/LayoutTests/' '!/JSTests/' '!/PerformanceTests/' \
    '!/Websites/' '!/ManualTests/' '!/Source/WebCore/' '!/Source/WebKit/' '!/Source/WebKitLegacy/' \
    '!/Source/ThirdParty/ANGLE/' '!/Source/ThirdParty/libwebrtc/' '!/Tools/' '/Tools/Scripts/' \
    '/Tools/jsc/' '/Tools/CMakeLists.txt'
git -C "$src" fetch -q --depth 1 --filter=blob:none origin "$rev"
git -C "$src" checkout -q -f "$rev"
for p in "$inputs"/patches/*.patch; do
    git -C "$src" apply --whitespace=nowarn "$p"
done

stage="$work/$name"
mkdir -p "$stage/webkit" "$stage/lightningpanda/crates/lp-jsc-sys/webkit/patches" "$stage/lightningpanda/scripts/jsc"
# Every file of the checkout, including the ones the patches add, without .git.
(cd "$src" && tar cf - --exclude ./.git .) | (cd "$stage/webkit" && tar xf -)
cp "$inputs/REVISION" "$stage/lightningpanda/crates/lp-jsc-sys/webkit/"
cp "$inputs"/patches/*.patch "$stage/lightningpanda/crates/lp-jsc-sys/webkit/patches/"
cp "$root/scripts/jsc/build.sh" "$stage/lightningpanda/scripts/jsc/"
cp "$root/docs/RELEASING.md" "$stage/BUILDING.md"

python3 - "$stage" "$commit" "$rev" <<'EOF'
import hashlib, json, os, sys
stage, commit, rev = sys.argv[1:]
web = os.path.join(stage, 'webkit')
def digest(p):
    h = hashlib.sha256()
    with open(p, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()
files = {}
for top, dirs, names in os.walk(web):
    for n in dirs + names:
        p = os.path.join(top, n)
        rel = os.path.relpath(p, web)
        if os.path.islink(p):
            files[rel] = {'symlink': os.readlink(p)}
        elif os.path.isfile(p):
            files[rel] = {'sha256': digest(p)}
pdir = os.path.join(stage, 'lightningpanda/crates/lp-jsc-sys/webkit')
inputs = {'jsc_input/REVISION': digest(os.path.join(pdir, 'REVISION'))}
for n in sorted(os.listdir(os.path.join(pdir, 'patches'))):
    inputs['jsc_input/patches/' + n] = digest(os.path.join(pdir, 'patches', n))
manifest = {'alreadyPatched': True, 'declaredLightningpandaCommit': commit, 'files': dict(sorted(files.items())),
            'inputs': inputs, 'revision': rev, 'upstream': 'https://github.com/WebKit/WebKit'}
with open(os.path.join(stage, 'SOURCE_MANIFEST.json'), 'w') as f:
    json.dump(manifest, f, indent=2, sort_keys=True)
    f.write('\n')
print(f'{len(files)} source entries, {len(inputs)} inputs', file=sys.stderr)
EOF

# Upstream notices for the fork archive: every license, copying, notice and
# authors file of the shipped tree.
rm -rf "${out:?}/jsc-licenses"
mkdir -p "$out/jsc-licenses"
(cd "$stage/webkit" && find Source Tools -type f \( -iname 'COPYING*' -o -iname 'LICENSE*' -o -iname 'NOTICE*' -o -name AUTHORS \) -print0 \
    | xargs -0 tar cf -) | (cd "$out/jsc-licenses" && tar xf -)

(cd "$work" && COPYFILE_DISABLE=1 tar czf "$out/$name.tar.gz" "$name")
echo "$out/$name.tar.gz"
