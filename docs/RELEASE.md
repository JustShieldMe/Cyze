# Building & Releasing Cyze

Cyze packages as a native desktop app via [Tauri](https://tauri.app). This
document covers producing installable executables for Linux, macOS, and Windows,
and the (optional) code-signing needed for distribution.

## What gets produced

`bundle.targets` is `"all"`, so each platform emits its native installers:

| Platform | Artifacts |
| --- | --- |
| Linux | `.deb`, `.AppImage`, `.rpm` |
| macOS | `.app`, `.dmg` (per-arch: Apple Silicon + Intel) |
| Windows | `.msi` (WiX), `.exe` (NSIS) |

Each bundle embeds two sidecars for that platform, so users don't install
anything extra to host a session:

- **`frostd`** — the local FROST coordination server.
- **`cloudflared`** — Cloudflare's tunnel client, used by the optional "public
  tunnel" coordinator-exposure mode (Apache-2.0, redistributed unmodified). This
  lets a coordinator expose the embedded server to remote participants with no
  install or PATH setup on their end.

## Sidecars

Tauri resolves `bundle.externalBin: ["binaries/frostd", "binaries/cloudflared"]`
to per-target files:

```
src-tauri/binaries/frostd-<target-triple>[.exe]
src-tauri/binaries/cloudflared-<target-triple>[.exe]
```

The binary is built from the same pinned `frost-tools` revision the app links
`frost-client` against (see `frost-client`'s `rev` in `src-tauri/Cargo.toml`).
CI builds it automatically (see below). To produce it locally for your host:

```bash
REV=$(grep -oP 'frost-tools.git", rev = "\K[0-9a-f]+' src-tauri/Cargo.toml)
git clone https://github.com/ZcashFoundation/frost-tools.git /tmp/frost-tools
cd /tmp/frost-tools && git checkout "$REV"
TRIPLE=$(rustc -vV | sed -n 's/host: //p')
cargo build --release -p frostd
mkdir -p "$OLDPWD/src-tauri/binaries"
cp "target/release/frostd" "$OLDPWD/src-tauri/binaries/frostd-$TRIPLE"
```

For local dev you also need a matching `cloudflared` for your host triple.
`scripts/fetch-cloudflared.sh` downloads the release pinned in
`scripts/CLOUDFLARED_VERSION` and checks it against `scripts/cloudflared.sha256`;
a download that doesn't match fails. CI uses the same script for releases.

```bash
TRIPLE=$(rustc -vV | sed -n 's/host: //p')
scripts/fetch-cloudflared.sh "$TRIPLE" "src-tauri/binaries/cloudflared-$TRIPLE"
```

`scripts/build-sidecar.sh` runs both steps for your host.

## Local build

Prerequisites:

- **Rust** (stable) and **Node 20+**.
- A **C toolchain + Perl** — the wallet DB uses SQLCipher with *vendored* OpenSSL
  (`bundled-sqlcipher-vendored-openssl`), which compiles OpenSSL from source.
  - Windows additionally needs **NASM** on `PATH`.
- **Linux** desktop deps: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`.

Then:

```bash
npm ci
npm run tauri build          # bundles for the current OS into src-tauri/target/release/bundle/
```

## Automated cross-platform release

Pushing a version tag runs `.github/workflows/release.yml`, which builds all
four targets (Linux x64, Windows x64, macOS arm64, macOS x64), builds the
matching `frostd` sidecar for each, and attaches the installers to a **draft**
GitHub Release:

```bash
# bump the version in package.json, src-tauri/tauri.conf.json (and its
# numeric MSI version) and src-tauri/Cargo.toml first, then:
git tag v0.3.0
git push origin v0.3.0
```

Review and publish the draft release in the GitHub UI.

## Licenses and SBOMs

Each build writes **third-party notices** (`scripts/third-party-notices.sh`)
and bundles them; the app shows them under **Settings → About**. They list,
with each license's text, the Rust crates in Cyze and in the bundled `frostd`
(from cargo-about, configured in `src-tauri/about.toml`), the bundled
`cloudflared`, and the npm packages in the frontend. The copy in the repository
is a placeholder. To write the real file locally:

```bash
cargo install cargo-about --features cli
scripts/third-party-notices.sh ~/.cargo/git/checkouts/frost-tools-*/<rev>   # frostd's checkout
```

After the builds, the `sbom` job attaches to the draft release a CycloneDX
SBOM for each part (`cyze-<version>-rust.cdx.json`, `-frostd.cdx.json`,
`-npm.cdx.json`) and the notices.

Which licenses may ship is decided in two places that must agree:
`src-tauri/deny.toml` (CI fails on anything else, through
`cargo deny check licenses`) and `src-tauri/about.toml`. Both list the same
permissive licenses, plus MPL-2.0 for unmodified crates whose source is public
on crates.io. `frost-client` and `frostd` declare no license in their manifests;
both config files clarify them as MIT OR Apache-2.0, after the license files at
the frost-tools repository root. Check that again when `scripts/PINNED_REV`
moves.

## Code signing (for distribution)

Unsigned builds run but trigger OS warnings (macOS Gatekeeper, Windows
SmartScreen). To ship signed builds, add these repository **secrets** — the
release workflow enables macOS signing automatically **only when
`APPLE_CERTIFICATE` is set** (it builds unsigned otherwise; empty signing env
vars would make Tauri attempt and fail a keychain import, so it's opt-in).

### macOS (notarized)

Add all six as repository secrets; the `Enable Apple code signing` step turns
signing on when `APPLE_CERTIFICATE` is present.

| Secret | What it is |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of your "Developer ID Application" `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Org (TEAMID)` |
| `APPLE_ID` | Apple ID email |
| `APPLE_PASSWORD` | app-specific password for notarization |
| `APPLE_TEAM_ID` | your Apple Developer Team ID |

### Windows (Authenticode)

Set `bundle.windows.certificateThumbprint` (+ `digestAlgorithm`, `timestampUrl`)
in `tauri.conf.json` to sign with a cert installed on the runner, or use
[Azure Trusted Signing](https://v2.tauri.app/distribute/sign/windows/). Without
this, the `.msi`/`.exe` are unsigned.

### Optional: signed auto-updates

Set `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) and add an `updater` section to
`tauri.conf.json` to enable Tauri's updater. This is worth it given the Zcash
network-upgrade cadence — it lets you push a consensus-critical update (e.g. the
NU6.3 / Ironwood crate bump) to users ahead of a mainnet activation height.

## Icons

The icon set under `src-tauri/icons/` (including `icon.ico` / `icon.icns`) is
generated from `src-tauri/app-icon.svg`:

```bash
cd src-tauri && npx tauri icon app-icon.svg
```

## Caveats / first-run notes

- All four targets build in CI; v0.3.0-rc.2 was the first release built on
  every platform.
- Building `frostd` from source on every release is slower but keeps the sidecar
  locked to the app's `frost-client` protocol revision.
- Before a general **mainnet** release: an independent security audit, code
  signing for macOS and Windows (above), and signed auto-updates, so the next
  network upgrade can reach users before it activates.
