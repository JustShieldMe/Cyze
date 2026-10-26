# CYZE — Coordinate Your ZCash Easily ❄️

**A desktop wallet for Zcash funds that a group controls together.**

Cyze puts a graphical interface on [ZF FROST](https://frost.zfnd.org/) threshold
signatures, built on the Zcash Foundation's
[frost-tools](https://github.com/ZcashFoundation/frost-tools). It lets a group of
people jointly control a Zcash wallet where **no single person ever holds the
full private key**. A configurable threshold of members (for example 2 of 3)
must approve every payment.

> ### ⚠️ Beta software — unaudited — use at your own risk
>
> Cyze is in **beta** and has **not been security-audited**. It runs on the
> **Zakura** Zcash crates, whose wallet libraries are still release
> candidates. Do not use it to hold funds you cannot afford to lose. You alone
> are responsible for backing up your key shares and recovery code, and for
> any transaction you broadcast. **No warranty is provided.** Start on
> testnet. If you use mainnet, use small amounts.
>
> **Mainnet activates NU7 at block 3,543,405 (around November 5, 2026).** Cyze
> 0.3 supports it; earlier versions can't send on mainnet after that.

---

## What it does

The app has five places: **Home**, **Approvals**, **Votes**, **Groups** and
**Settings**.

- **A shared Zcash wallet.** Home shows the active group's balance, with
  **Receive** (a rotating shielded address, with QR code) and **Send**.
  - **Sending takes two steps:** enter the payment, then review it and choose
    who approves.
  - **Send all** pays everything, with the fee taken out of the amount.
  - **Pay several at once:** up to 20 payments in one transaction, approved
    once. Enter them one by one or paste a list.
  - **Transparent addresses** are detected automatically, with a note that the
    payment will be public.
  - **Activity** lists payments with their memos and fees, and exports them as
    CSV for accounting.
  - **Read-only access:** share the wallet's viewing key with an accountant or
    auditor, after a plain warning of what it reveals.
  - **Payees:** the group keeps a shared list of the addresses it pays, by
    name. Pick one when sending; approvers and the history show its name.
  - **Pay if agreed:** a group poll can propose payments. Once the group
    agrees, its author sends exactly those, and each approver's app checks the
    transaction against the poll.
  - **Rules:** pay only trusted payees, more approvals for larger payments, a
    wait with veto before larger payments, and a spending limit within which
    members' apps can approve by themselves, adopted by poll.
  - **Reasons:** a payment can say what it's for. Approvers see it, and so
    does every member's history. It never goes on chain.
  - With several groups, a switcher at the top picks the one wallet the app
    syncs and acts on.
- **A wallet that's always current.**
  - Sync runs in the background from unlock to lock, on any screen.
  - Scanning is about 4.5 times faster on the Zakura crates.
  - Each member's app records the wallet's starting block when the group is
    created, so no payment to it is missed.
  - Cyze picks the quickest of zec.rocks's servers, and switches if one stops
    answering.
  - **Tor** for the wallet's traffic is optional.
- **Approvals.**
  - The app joins signing requests by itself and shows what it verified from
    the transaction.
  - Each approver approves a payment once, however many notes it spends. A
    system notification says when a request is waiting.
  - Approvals must come within the transaction's standard expiry (about 50
    minutes). Both sides see the deadline.
  - If time runs out, **Ask again** rebuilds the payment for the same members.
    If a payment was approved but didn't reach the network, **Try sending
    again** sends it without new approvals.
- **Groups.**
  - **Joining takes one invite code.** Creating a group starts the app's
    coordination server, and a link members can reach (Tailscale if it's set
    up, otherwise a Cloudflare tunnel). Members paste the invite to join.
  - **Contacts** carry safety codes, so people can confirm they hold each
    other's real keys.
  - **New groups get a private viewing key,** and older groups can be made
    private (see [Security model](#security-model)).
- **Votes.** Group polls let any member ask the group a question. Zcash
  community votes show the coinholder rounds run through
  [Shielded Vote](https://valargroup.gitbook.io/shielded-vote-docs), and
  **Decide together** turns a round's proposals into group polls. Casting the
  group's vote with its balance comes in a later version. See
  [Voting in Cyze](docs/VOTING.md).
- **Backups.** Encrypted key backups restore your shares on a new device. The
  app reminds you while a group isn't in any backup yet.
- **ZcashNames (testnet).** Send to `name.zcash`. The resolved address is
  shown for you to confirm before anything is signed. Lookups are off on
  mainnet until a production directory exists.
- **Hosting choices.** Run the bundled `frostd` coordination server and reach
  it through a Cloudflare tunnel, Tailscale, a direct address (pinned
  self-signed TLS) or your own domain; or point at any external `frostd`.
- **A local devnet for development.** Run against a
  [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) regtest
  chain (0.3 runs NU6.3, 0.4 runs NU7), found with one click.
- **Advanced options**, off by default: test networks and the devnet, a
  diagnostics log, Ed25519 groups and raw-message signing.

Cyze supports both frost-tools ciphersuites: **RedPallas** (re-randomized FROST
for Zcash shielded spends; every wallet group) and **Ed25519** (general-purpose
signing, an advanced option).

## How it works

1. **A group is created.**
   - The members run a distributed key generation (DKG) through the
     coordination server. Each ends up with one key share, and the full
     private key never exists anywhere.
   - They also agree on a random viewing seed, so the group's public key isn't
     enough to see its payments. Each member contributes, and everyone compares
     a digest of the result.
   - Each member's app records the chain height at that moment as the wallet's
     starting point.
2. **The wallet stays in sync.**
   - Each member's app downloads compact blocks from a lightwalletd server.
   - It finds the group's payments on the member's own computer, with the
     viewing key. The server can't tell which payments are the group's.
3. **A member sends a payment.**
   - Their app builds the unsigned transaction (a PCZT) from the group's notes,
     with the standard ZIP-317 fee, and sends it to the approvers they chose.
   - Each approver's app checks the transaction itself before showing it: that
     the hash to sign is the transaction's, that the inputs are the group's
     notes, the real recipients and amounts, and that the change returns to
     the group.
   - When the approvers approve, their apps contribute signature shares. The
     sender's app combines them, builds the zero-knowledge proof, and
     broadcasts the transaction.
4. **The servers are untrusted relays.**
   - Everything members send each other through the coordination server is
     end-to-end encrypted (Noise). The server sees public communication keys,
     IP addresses and timing.
   - The sync server sees which blocks are downloaded and when, and that a
     transaction was broadcast. It sees your IP address unless Tor is on.

[PRIVACY.md](PRIVACY.md) lists everything Cyze sends and keeps.

## Security model

### Keys and backups

- **An envelope-encrypted keystore.** Key shares and contacts live in a
  keystore whose random data key (XChaCha20-Poly1305) is wrapped under both
  your passphrase and a one-time **12-word BIP-39 recovery code** (Argon2id).
  Either can unlock it, and changing one doesn't invalidate the other. The
  plaintext is byte-compatible with upstream
  `~/.config/frost/credentials.toml`, so an existing frost-client config
  imports in one step.
- **The recovery code is shown once** at setup, behind an explicit
  acknowledgement, and is never stored.
- **Backups restore key shares on another device.** A backup file holds the
  encrypted keystore, plus the wallets' scan start heights encrypted under a
  key derived from the keystore. It opens only with your passphrase or
  recovery code; the file and the secret are each useless alone.
- **Keystore writes** are atomic and flushed to disk, and the previous version
  is kept. Unlocking falls back to it if the current file is damaged.
- **Key-derivation settings** read from a keystore or backup file are bounded,
  so a tampered file can't make an unlock exhaust memory or hang.

### Privacy

- **A private viewing key per group.**
  - When a group is created, its members agree on a random viewing seed: each
    sends the others a random contribution, and the seed hashes all of them, so
    no one member chooses it.
  - The seed lives in each member's keystore, encrypted and backed up with
    their share. Knowing the group's id (its Orchard `ak`) is then not enough
    to see its payments.
  - Groups where a member uses the upstream command-line tools (an advanced
    option) still derive their viewing key from `ak`.
- **Older groups can go private.**
  - A group created before Cyze 0.3 can run the same agreement later, over a
    session that holds exactly the group's members.
  - The funds move to the new address with an ordinary payment the group
    approves. Co-signers' apps count only outputs to the new key as change, so
    nothing can be routed back to the old, publicly viewable address
    unnoticed.
  - Payments made before the move stay visible to anyone who knows the group's
    id.
- **Nothing outside the keystore names a group.**
  - Per-group settings, contact nicknames, poll records, signed transactions
    awaiting broadcast and the ceremony history are encrypted under keys
    derived from the keystore.
  - Wallet databases are encrypted with SQLCipher, in folders with opaque
    names.
  - The diagnostics log has group ids removed, and stays in memory only.
- **Optional Tor for wallet traffic.**
  - With Tor on, every connection to a sync server, broadcasts included, goes
    through an embedded Tor client (Arti). Nothing falls back to a direct
    connection.
  - Coordination traffic and a local devnet are not routed through Tor.
- **Payments look like any other wallet's.** Transactions keep the standard
  expiry instead of a longer one that would single out Cyze's payments
  on-chain.

### Approving payments

- **Signers verify the transaction itself.** The sender attaches the unsigned
  transaction (PCZT). Each signer's app checks:
  - the message to sign is that transaction's sighash;
  - every input carrying value is one of the group's own notes;
  - every recipient and amount matches its on-chain commitment;
  - the sender's description matches what was proven.

  If any check fails, it refuses automatically.
- **One approval per payment.** A payment that spends several notes needs one
  approval; its other rounds are approved once they verify as the same
  transaction.
- **Group rules, enforced by every approver's app.** A group can adopt rules
  by poll: pay only trusted payees (those as many members trust as a payment
  needs), more approvals for larger payments, a wait before larger payments,
  during which any member can veto them, and a spending limit within which
  members who choose to can have their app approve payments to trusted payees
  by itself. The sender's and each
  approver's app refuse a payment that breaks them. With no contracts on
  shielded funds, rules hold unless that many members act together, like the
  wallet itself.
- **Names come from your own app.** A payment to one of the group's payees is
  named from the approver's own copy of the list, by the address the
  transaction verifiably pays. Only the member who added a payee can change
  it. A reason for a payment is the sender's word, shown as theirs.
- **No unverified signing.** Wallet keys never sign a request without a
  transaction attached, unless advanced options are on.
- **Nothing is at risk before approval.** Round-1 commitments don't depend on
  the message.

### Servers and people

- **frostd is an untrusted relay.** All ceremony and poll messages are
  end-to-end encrypted (Noise, via frost-client).
- **Self-signed server certificates are pinned exactly** (trust on first use,
  with the fingerprint shown). The app accepts that one certificate and
  nothing else for the server, not even one a public CA issued for the same
  address. A Cloudflare tunnel or Tailscale address presents a publicly valid
  certificate, so members using it skip this step.
- **Invites never add keys silently.** An invite lists every member's key. The
  app marks those already in your contacts, and adds new ones only after you
  confirm, since whoever controls a listed key holds a share.
- **Safety codes** let people confirm they hold each other's real keys. Each
  key has a 30-digit code (an iterated hash, like Signal's safety numbers):
  you read yours aloud, and the other person checks it and marks you verified.
- **No one can forge a vote.** Each member's app sends only the poll records
  its member wrote, and the encrypted channel authenticates the sender.

To report a vulnerability, see [SECURITY.md](SECURITY.md).

## Getting started

Download an installer from the
[Releases page](https://github.com/JustShieldMe/Cyze/releases), or build from
source (below). Builds aren't code-signed yet, so macOS and Windows show a
warning the first time you open the app.

1. **First run.**
   - Set a passphrase and a display name.
   - Save the one-time recovery code.
   - Choose **Start a shared wallet** or **Join with an invite**.
2. **Start a group.** The organizer adds each member's contact code, chooses
   how many must approve, and clicks **Create invite**. The app starts a
   server members can reach and makes the invite.
3. **Join.** Members paste the invite, check the keys listed, and click
   **Join**. The group's wallet opens on **Home** for everyone.
4. **Back up.** Each member saves a backup file from **Settings → Security**.
5. **Receive and send.**
   - **Receive** shows the group's address.
   - **Send** builds a payment and asks the members you choose. They approve it
     in **Approvals**, and the app sends it once enough have.

## Documentation

- **[User guide](docs/USER_GUIDE.md):** every feature step by step, in the
  order to test a build.
- **[Voting in Cyze](docs/VOTING.md):** group polls and Zcash community votes.
- **[Privacy](PRIVACY.md):** what the app sends, to whom, and what it keeps.
- **[Security policy](SECURITY.md):** how to report a vulnerability.
- **[Changelog](CHANGELOG.md):** what changed in each version.
- **[Building and releasing](docs/RELEASE.md):** packaging, sidecars, code
  signing, licenses and SBOMs.
- **[Sync design](docs/SYNC_OPTIMIZATION.md):** how the wallet syncs, with
  measurements.

**🗳️ [Voting in Cyze](docs/VOTING.md)**: group polls and Zcash community
votes, step by step.

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
npm test                                  # frontend smoke tests
cd src-tauri && cargo test --workspace    # Rust: core, commands, ceremonies
```

- **Frontend smoke tests** (`src/test`) drive the real interface against a fake
  backend: onboarding, sending and asking again, approving, making an older
  group private, votes, sync status, settings.
- **`src-tauri/core/tests/ceremony_e2e.rs`** spawns a real `frostd` and runs complete
  3-party DKG and 2-of-3 signing over TLS for both ciphersuites, plus a
  rejection path.
- **The app crate's tests** (`src-tauri/tests`) cover the Tauri command layer:
  approvals, invites, backups and restore, private settings, viewing keys and
  group polls.
- **Devnet tests**, ignored by default, run against a
  [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) chain:
  a real 2-of-2 send, recovery from a reorg, and making a group private. See
  the header of `src-tauri/tests/devnet_send.rs` to run them.

To time a real wallet sync against lightwalletd, use the sync benchmark (see
[`docs/SYNC_OPTIMIZATION.md`](docs/SYNC_OPTIMIZATION.md#measurements-2026-10-23)):

```sh
cd src-tauri && cargo run --release -p frost-app-core --features wallet --example sync_bench -- --blocks 20000
```

## Layout

- **`src-tauri/core`** is `frost-app-core`, with no Tauri dependency:
  - the keystore, backups, and encryption at rest (`keystore.rs`, `backup.rs`,
    `sealed.rs`);
  - the frostd transport with pinned-certificate TLS (`transport.rs`,
    `tls.rs`);
  - the DKG and signing engines (`dkg.rs`, `signing.rs`);
  - invites, safety codes and viewing seeds (`invite.rs`, `safety.rs`,
    `viewing.rs`);
  - the Zcash wallet: sync, the send path, and Tor (`wallet.rs`,
    `wallet/tor.rs`), plus signer verification (`wallet/verify.rs`);
  - group polls and community votes (`polls.rs`, `community_vote.rs`);
  - ZcashNames (`zns.rs`).
- **`src-tauri/src`** is the Tauri adapter:
  - commands (`commands/`) and the background sync service
    (`sync_service.rs`);
  - the `frostd` sidecar, Cloudflare tunnel and Tailscale (`sidecar.rs`,
    `tunnel.rs`, `tailscale.rs`);
  - devnet discovery (`devnet.rs`) and encrypted private settings
    (`private.rs`).
- **`src/`** is the React + TypeScript frontend:
  - the five places and their screens (`screens/`);
  - the wallet's panels (`wallet/`);
  - ceremony state (`stores/`);
  - smoke tests (`test/`).
- **`scripts/PINNED_REV`** is the frost-tools revision used for both the
  `frost-client` library dependency and the `frostd` sidecar build (they must
  match for wire compatibility).

## License

[MIT](LICENSE). The software Cyze is built on, including the bundled `frostd`
and `cloudflared`, keeps its own licenses. **Settings → About** lists them with
their texts, and each release carries SBOMs.
