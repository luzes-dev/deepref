# Verified dependency backports

The dependency gate runs `python3 scripts/ci/scan-dependencies.py` through
`security.osv`. It preserves the original OSV configuration and raw JSON findings
in the diagnostic log. It never treats scanner execution or parse errors as a
successful scan.

## deepmerge 4.3.1

The Svelte Vite plugin depends on deepmerge 4.3.1. Its
[prototype-poisoning advisory](https://github.com/advisories/GHSA-gcqx-w48m-w2h4)
has no published patched version as of 2026-10-28. The repository applies a small
MIT-licensed dependency patch through pnpm; it does not invent a fixed upstream
version or remove the package's license notices.

The patch filters `__proto__` from the common key enumeration used for both the
source and target copy loops. It covers `index.js`, the CommonJS build and the UMD
build. Nested merges use the same enumeration. Other keys, symbol properties,
functions, regular expressions and custom array replacement retain their existing
behavior. The patch hash is
`658bcc27776e130ae51c0ad9597d4a3a52ac867af0fc014b78714ff162845fe0`.

Before applying the backport disposition, the gate:

1. Verifies the patch bytes, pnpm patch configuration, the only deepmerge package
   version and patched snapshot, and every named dependency edge in the lockfile.
2. Resolves the actual copy loaded by the Svelte plugin and verifies the version
   and SHA-256 of all three patched files.
3. Runs the prototype and ordinary configuration-merge contracts, including
   null-prototype targets, nested null placeholders, target-owned keys and both
   executable builds. It also tests rejection of patch/lockfile drift.
4. Runs the unchanged recursive OSV scan with the existing configuration and
   preserves its raw output. Only the exact advisory for npm deepmerge 4.3.1 with
   its matching PURL in this repository's root `pnpm-lock.yaml` is classified as
   locally patched. Other versions, manifests, packages or advisories still fail.

The raw advisory remains visible alongside the verified patch identity and the
unresolved findings. No global advisory ignore or package exclusion is added.
The classification expires on 2027-01-26; upgrade or review it before that date.
A frozen install without the patch, edited installed bytes, missing or changed
lockfile binding, contract failure or expired disposition fails closed before
classification. An unexplained scanner failure also fails closed.

Run the narrow contracts with:

```sh
node scripts/ci/check-deepmerge-patch.mjs
node --test scripts/ci/test-deepmerge-contract.mjs
python3 -m unittest discover -s scripts/ci/tests -p 'test_*.py'
```

To inspect findings without the backport classification, run the underlying
scanner directly:

```sh
osv-scanner scan source --recursive --config osv-scanner.toml --format=json .
```

A later upstream release requires a normal dependency update, removal or revision
of the patch and identity guard, renewed contracts and independent review. Do not
extend this disposition to another advisory or package without its own evidence.
