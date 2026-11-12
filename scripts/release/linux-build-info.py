# Writes BUILD_INFO.json for a Linux release archive (scripts/release/assemble.sh).
import hashlib, json, os, sys

d, platform, version, commit, src, client, source_asset, rev = sys.argv[1:]


def digest(p):
    with open(p, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


info = {
    'declaredSourceCommit': commit,
    'expectedForkCapability': True,
    'inputEngineSHA256': digest(os.path.join(src, platform, 'lightningpanda')),
    'javascriptCore': 'static-patched',
    'packagedComparatorSHA256': digest(os.path.join(d, 'lp-snapshot-compare')),
    'packagedEngineSHA256': digest(os.path.join(d, 'lightningpanda')),
    'platform': platform,
    'sdkSHA256': digest(client),
    'sourceAsset': source_asset,
    'validation': 'Packaging checks only; the build job smoke-tests and runs the client suite.',
    'version': version,
    'webkitRevision': rev,
}
with open(os.path.join(d, 'BUILD_INFO.json'), 'w') as f:
    json.dump(info, f, indent=2, sort_keys=True)
    f.write('\n')
