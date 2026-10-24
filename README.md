# CYZE — Coordinate Your ZCash Easily ❄️

**A desktop wallet and signing companion for threshold-controlled Zcash funds.**

Cyze puts a graphical interface on [ZF FROST](https://frost.zfnd.org/) threshold
signatures, built on the Zcash Foundation's
[frost-tools](https://github.com/ZcashFoundation/frost-tools). It lets a group of
people jointly control a Zcash wallet where **no single person ever holds the
full private key** — a configurable threshold (e.g. 2-of-3) must cooperate to
authorize any spend.

> ### ⚠️ Beta software — unaudited — use at your own risk
>
> Cyze is in **beta** and has **not been security-audited**. It targets the
> **Ironwood (NU6.3)** network upgrade and runs on the **Zakura** Zcash crates,
> whose wallet libraries are still release candidates. Do not use it
> to hold funds you cannot afford to lose. You are solely responsible for backing
> up your key shares and recovery code, and for any transaction you broadcast.
> **No warranty is provided.** Start on testnet, and if you use mainnet, use
> small amounts.

---

## What it does

- **Key ceremonies (DKG)** — create or join a distributed key generation
  ceremony. The private key is split into shares across participants and never
  exists in one place.
- **Threshold signing** — coordinate a signing session (the coordinator can also
  be a signer), or participate through an inbox with an explicit review/approve
  step before your signature share is produced.
- **Zcash wallet (Ironwood)** — for RedPallas groups: sync from a lightwalletd
  server with a pipelined light-client scanner, view the group's **Ironwood
  shielded balance**, receive to a rotating group address (with QR), and **send** —
  each spend is authorized by a live FROST signing ceremony among the group. Sends
  build **V6 transactions** post-NU6.3. Any legacy **Orchard** funds are surfaced
  only when present, with a one-tap sweep into Ironwood. The app focuses on **one
  active wallet at a time** — selecting a group makes it the sole wallet the app
  syncs and acts on. Includes on-chain and local transaction/message history, and
  an in-app diagnostics log for troubleshooting.
- **Local devnet for development** — besides mainnet and testnet, the wallet can
  run against a local regtest devnet
  ([thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura)), found
  with one click, for Zcash protocol and app development with throwaway funds.
- **Coinholder voting** — cast a Zcash coinholder-poll vote from a group: paste
  the poll's published ballot, answer, and the vote is delivered as a shielded
  memo (Vote Cast Memo v1) to the poll's reception address through the same FROST
  signing path. Vote weight is set by the poll's balance snapshot, not the wallet.
- **ZcashNames (ZNS)** — on testnet, send to a human-readable `name.zcash` in
  the recipient field: it is resolved to a unified address via the public ZNS
  indexer and shown for you to confirm before signing (the resolver is external,
  never an authorization). Off on mainnet until a production indexer exists; the
  only mainnet one is a test service.
- **Server hosting** — run the `frostd` coordination server embedded
  (auto-generated, pinned self-signed TLS), and expose it to off-LAN peers either
  through a built-in **Cloudflare tunnel** (public HTTPS URL, no port-forwarding)
  or over **Tailscale** (a stable `*.ts.net` tailnet address with automatic,
  publicly-trusted TLS — tailnet-only, not public), or point at any external
  `frostd`.
- **Contacts & groups** — a per-group view with public key material, named
  participants, receive addresses, and share-repair guidance.

Supports both frost-tools ciphersuites: **Ed25519** (generic signing) and
**RedPallas** (re-randomized FROST for Zcash shielded spend authorization).

## Security model

- Key shares and contacts live in an **envelope-encrypted keystore**: a random
  data key (XChaCha20-Poly1305) is wrapped under both your passphrase and a
  one-time **12-word BIP-39 recovery code** (Argon2id), so either can unlock, and
  changing one doesn't invalidate the other. The plaintext is byte-compatible
  with upstream `~/.config/frost/credentials.toml`, so an existing frost-client
  config imports in one step.
- The **recovery code is shown once** at setup behind an explicit
  acknowledgement and is never stored. It unlocks the keystore if you forget the
  passphrase.
- **Backups** restore your key shares on another device. A backup file holds the
  encrypted keystore plus the wallet's scan start heights, which are encrypted
  under a key derived from the keystore. It opens only with your passphrase or
  recovery code, and neither half is enough alone. The app reminds you to back
  up whenever a group isn't in any backup yet.
- **A private viewing key per group.** When a group is created, its members
  agree on a random viewing seed: each sends the others a random contribution,
  the seed hashes all of them (so no one member chooses it), and everyone
  compares a digest of the result before the group is saved. The seed lives in
  each member's keystore, so it's encrypted and backed up with their share.
  Knowing the group's id (its Orchard `ak`) is then not enough to see its
  payments. Groups created before this, or with a member using the upstream
  command-line tools (an advanced option), still derive their viewing key
  from `ak`.
- **Nothing outside the keystore names a group.** Per-group settings, contact
  nicknames, signed transactions awaiting broadcast, and the ceremony history
  are encrypted under keys derived from the keystore, and wallet folders have
  opaque names derived the same way. Settings from earlier versions move over
  on the first unlock.
- **Invites never add keys silently.** An invite lists every member's key; the
  app marks those already in your contacts and adds new ones only after you
  confirm, since whoever controls a listed key holds a share.
- Keystore writes are atomic and flushed to disk, and the previous version is
  kept; unlocking falls back to it if the current file is damaged.
- All ceremony messages are **end-to-end encrypted** (Noise, via frost-client);
  `frostd` is an untrusted relay.
- Self-signed server certificates are **pinned exactly** (trust-on-first-use
  with fingerprint display): the app accepts that one certificate and nothing
  else for the server, not even one a public CA issued for the same address. A
  Cloudflare tunnel or Tailscale address presents a publicly valid certificate,
  so peers using it skip cert trust.
- **Safety codes** let people confirm they hold each other's real keys: each
  key has a 30-digit code (an iterated hash, as Signal's safety numbers), you
  read yours aloud, and the other person checks it and marks you verified.
  Invites show which members are verified.
- Key-derivation settings read from a keystore or backup file are bounded, so a
  tampered file can't make an unlock exhaust memory or hang.
- Signers **verify the transaction itself** before approving. The coordinator
  attaches the unsigned transaction (PCZT). Each signer's app checks:
  - the message to sign is that transaction's sighash
  - every input carrying value is one of the group's own notes
  - every recipient and amount matches its on-chain commitment
  - the coordinator's description matches what was proven

  If any check fails, it refuses automatically. A multi-note send needs one
  approval; its other rounds are approved after they verify as the same
  transaction. Wallet keys never sign a request without a transaction
  attached, unless advanced options are on. Round-1 commitments are
  message-independent, so nothing is at risk before approval.

## Getting started

On first launch you set a passphrase and a display name, and are shown a
one-time recovery code to back up. A typical first run:

1. **Contacts** — members send the organizer their contact codes.
2. **New group** — the organizer picks the members and how many must approve;
   the app starts a server members can reach and makes an **invite**. Members
   choose **Join with an invite** and paste it.
3. **Home** — the wallet: its receive address, balance, and history.
4. **Send** — enter a recipient and amount; the app runs a signing ceremony with
   the chosen threshold of members, then broadcasts the transaction.

**📖 [User Walkthrough & Testing Guide](docs/USER_GUIDE.md)** — a detailed,
step-by-step guide covering security setup, contacts, hosting/joining a server,
the DKG ceremony, building and approving transactions, and configuration. Follow
it in order to fully test and vet the application.

## Building

Prerequisites:

- Rust (1.92+), Node 18+
- Tauri Linux system deps:
  `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev librsvg2-dev libayatana-appindicator3-dev build-essential`

```sh
npm install
./scripts/build-sidecar.sh   # REQUIRED — see below
npm run tauri dev            # development
npm run tauri build          # native installers (AppImage/deb/…)
```

**`build-sidecar.sh` is not optional.** The app bundles two sidecar binaries, and
Tauri refuses to build unless both are present for your host's target triple:

- **`frostd`** — the coordination server, built from `frost-tools` at the pinned
  revision (`scripts/PINNED_REV`). It must match the `frost-client` dependency in
  `src-tauri/Cargo.toml` or client and server disagree on the wire format.
- **`cloudflared`** — downloaded from Cloudflare's releases, so the optional
  public-tunnel feature needs no separate install or PATH entry.

The script fetches both. Skip it and the build fails late with
`resource path 'binaries/cloudflared-<triple>' doesn't exist`. The binaries land
in `src-tauri/binaries/` (gitignored), so a fresh clone always needs this step.

Windows builds run natively (MSVC + Node) or via WSL2; build a Windows `frostd`
sidecar and target `nsis`/`msi`. To run two instances on one machine (e.g. to
play coordinator and participant), give the second its own data dir:

```sh
FROST_APP_DATA_DIR=/tmp/frost-app-2 npm run tauri dev
```

See [`docs/RELEASE.md`](docs/RELEASE.md) for packaging and sidecar details.

## Tests

The core crate is Tauri-free and fully testable headlessly:

```sh
cd src-tauri && cargo test -p frost-app-core
```

`tests/ceremony_e2e.rs` spawns a real `frostd` and runs complete 3-party DKG +
2-of-3 signing ceremonies over TLS for both ciphersuites, plus a rejection path;
keystore tests cover the envelope format and recovery code. A headless smoke test
drives the full Tauri command layer (`cargo test -p frost-app --test smoke`).

To time a real wallet sync against lightwalletd, use the sync benchmark (see
[`docs/SYNC_OPTIMIZATION.md`](docs/SYNC_OPTIMIZATION.md#measurements-2026-10-23)):

```sh
cd src-tauri && cargo run --release -p frost-app-core --features wallet --example sync_bench -- --blocks 20000
```

## Layout

- `src-tauri/core` — `frost-app-core`: keystore, frostd transport (pinned-cert
  TLS), DKG/signing ceremony engines, the Zcash wallet/PCZT send path (Ironwood,
  plus legacy Orchard) and pipelined sync driver, coinholder-poll voting
  (`voting.rs`), and ZcashNames resolution (`zns.rs`). No Tauri dependency.
- `src-tauri/src` — Tauri adapter: commands, event forwarding, sidecar lifecycle.
- `src/` — React + TypeScript frontend.
- `scripts/PINNED_REV` — the frost-tools revision used for both the
  `frost-client` library dependency and the `frostd` sidecar build (they must
  match for wire compatibility).

## License

See repository headers. Third-party components (`frostd`, `cloudflared`) are
redistributed under their own licenses.
