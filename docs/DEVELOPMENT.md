# Development

Building Cyze, running its tests, and finding your way around the code.
Packaging and releases are in [RELEASE.md](RELEASE.md).

## Building

Prerequisites:

- Rust 1.92+ (stable) and Node 20+.
- A C toolchain and Perl: the wallet database uses SQLCipher with vendored
  OpenSSL, compiled from source.
- Tauri's Linux system packages:
  `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev librsvg2-dev libayatana-appindicator3-dev build-essential`

```sh
npm install
./scripts/build-sidecar.sh   # REQUIRED — see below
npm run tauri dev            # development
npm run tauri build          # native installers (AppImage/deb/…)
```

**`build-sidecar.sh` is not optional.** The app bundles two sidecar binaries,
and Tauri refuses to build unless both are present for your host's target
triple:

- **`frostd`**, the coordination server, built from `frost-tools` at the
  pinned revision (`scripts/PINNED_REV`). It must match the `frost-client`
  dependency in `src-tauri/Cargo.toml`, or client and server disagree on the
  wire format.
- **`cloudflared`**, for the optional public tunnel: the release pinned in
  `scripts/CLOUDFLARED_VERSION`, checked against `scripts/cloudflared.sha256`.

The script fetches both. Skip it and the build fails late with
`resource path 'binaries/cloudflared-<triple>' doesn't exist`. The binaries land
in `src-tauri/binaries/` (gitignored), so a fresh clone always needs this step.

Windows builds run natively (MSVC, Node and NASM) or through WSL2. To run two
instances on one machine (for example, to play organizer and member), give
the second its own data directory:

```sh
FROST_APP_DATA_DIR=/tmp/frost-app-2 npm run tauri dev
```

## Tests

```sh
npm test                                  # frontend tests
cd src-tauri && cargo test --workspace    # Rust: core, commands, ceremonies
```

- **Frontend tests** (`src/test`) drive the real interface against a fake
  backend (`src/test/fake.tsx`): onboarding, sending, batches, approvals,
  payees, polls and paying what was agreed, rules, scheduled payments, share
  repair, moving to a new group, sync status and settings. The router keeps
  the first path per test file, so tests of different screens go in separate
  files.
- **`src-tauri/core/tests/ceremony_e2e.rs`** spawns a real `frostd` and runs
  complete 3-party DKG and 2-of-3 signing over TLS for both ciphersuites, plus
  a rejection path.
- **The app crate's tests** (`src-tauri/tests`) run members' apps against a
  real `frostd`: approvals, invites, backups and restore, private settings,
  viewing keys, group polls, payees and rules, scheduled payments, share
  repair, and moving to a new group. They skip themselves if the sidecar isn't
  built.
- **Devnet tests**, ignored by default, run against a
  [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) chain:
  real 2-of-2 sends (payees, a batch of 20, pay if agreed, rules and automatic
  approval), recovery from a reorg, and making a group private. The header of
  `src-tauri/tests/devnet_send.rs` says how to run them.

To time a real wallet sync against lightwalletd, use the sync benchmark (see
[SYNC_OPTIMIZATION.md](SYNC_OPTIMIZATION.md#measurements-2026-10-23)):

```sh
cd src-tauri && cargo run --release -p frost-app-core --features wallet --example sync_bench -- --blocks 20000
```

[UAT.md](UAT.md) and [SYNC_PIPELINE_UAT.md](SYNC_PIPELINE_UAT.md) are the
manual acceptance checklists.

## Code layout

- **`src-tauri/core`** is `frost-app-core`, with no Tauri dependency:
  - the keystore, backups, and encryption at rest (`keystore.rs`, `backup.rs`,
    `sealed.rs`);
  - the frostd transport with pinned-certificate TLS (`transport.rs`,
    `tls.rs`);
  - the DKG, signing and share-repair engines (`dkg.rs`, `signing.rs`,
    `repair.rs`);
  - invites, safety codes and viewing seeds (`invite.rs`, `safety.rs`,
    `viewing.rs`);
  - the Zcash wallet: sync, the send path, and Tor (`wallet.rs`,
    `wallet/tor.rs`), plus signer verification (`wallet/verify.rs`);
  - group records and what they add up to: polls (`polls.rs`), payees and
    reasons (`payees.rs`), rules (`rules.rs`) and schedules (`schedules.rs`);
  - community votes (`community_vote.rs`) and ZcashNames (`zns.rs`).
- **`src-tauri/src`** is the Tauri adapter:
  - commands (`commands/`) and the background sync service
    (`sync_service.rs`); the approval gate is in `commands/signing.rs`;
  - the `frostd` sidecar, Cloudflare tunnel and Tailscale (`sidecar.rs`,
    `tunnel.rs`, `tailscale.rs`);
  - devnet discovery (`devnet.rs`) and encrypted private settings
    (`private.rs`).
- **`src/`** is the React + TypeScript frontend:
  - the five places and their screens (`screens/`);
  - the wallet's panels (`wallet/`);
  - ceremony state (`stores/`), and the root listeners: ceremony events
    (`CeremonyListener.tsx`) and scheduled payments (`Scheduler.tsx`);
  - tests (`test/`).
- **`scripts/PINNED_REV`** is the frost-tools revision used for both the
  `frost-client` library dependency and the `frostd` sidecar build (they must
  match for wire compatibility).
