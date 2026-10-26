# Changelog

All notable changes to Cyze are recorded here. Versions follow
[semantic versioning](https://semver.org); pre-1.0 minor bumps may include
breaking changes. Cyze is beta, unaudited software (see the README).

## [Unreleased]

### Added
- **Votes**, a fifth place in the sidebar:
  - **Group polls:** any member asks the group a question, and an option counts
    as agreed once as many members back it as a payment needs.
  - **Zcash community votes:** the coinholder rounds run through Shielded Vote,
    with **Decide together** to turn a round's proposals into group polls.
    Casting the group's vote comes later.
  - Records from a newer version of Cyze are kept and skipped, so members on
    different versions stay in sync.
- **Payees:** a shared list of the addresses a group pays, by name.
  - Pick one in the send form, or save a typed address with **Save as a group
    payee**.
  - Approvers see the payee's name, looked up in their own app from the
    address the transaction verifiably pays.
  - Only the member who added a payee can change or remove it, and no two
    payees share a name or an address.
- **Batch payments:** up to 20 payments in one transaction, approved once.
  - **Pay several at once** on the send form, with rows to fill or **Paste a
    list** (address or payee name, amount, memo per line).
  - Each approver's app checks that the transaction makes exactly the payments
    described, in any order.
  - Activity shows a batch as its payments.
- **Pay if agreed:** a group poll can propose payments, answered **Pay** or
  **Don't pay**.
  - Once Pay is agreed, the member who proposed it sends them with **Send
    payment**, built from the poll itself.
  - Each approver's app checks the request against its own copy of the poll:
    it refuses one sent by another member, one that doesn't make exactly the
    proposed payments, or one already paid, and shows whether the group
    agreed.
  - The poll then shows **Paid**, and can't be sent again.
- **Group rules**, adopted by poll (**Rules** on Home, then **Propose rules**):
  - **Pay only trusted payees.** A payee is trusted once as many members trust
    its address as a payment needs approvals.
  - **Larger payments need more approvals**, for payments over an amount
    together in one transaction.
  - **Small payments can be approved automatically:** within a spending limit
    (counting everything the wallet sent in the period), members who turn it
    on let their app approve payments to trusted payees by itself, and it
    tells them.
  - **Larger payments wait for vetoes:** they're announced in a payment poll
    (**Announce it** on the send form) and can be sent once the wait has
    passed with no member choosing Don't pay. Each app counts the wait from
    when it received the announcement.
  - The sender's app checks the rules before asking anyone, and each
    approver's app checks them again and refuses a payment that breaks them.
    Approvers see which rules a request met.
- **Accounting export:** **Export CSV** on Activity saves the history, one row
  per payment, with fees, payees, memos, reasons and who signed.
- **Read-only access:** **Share read-only access** on a group's page gives an
  accountant or auditor the viewing key, once you confirm what it reveals. The
  key no longer shows under Keys and addresses.
- **Reasons:** say what a payment is for, for the group only.
  - Approvers see it with the request, attributed to the member who asked.
  - Once sent, every member's **Activity** shows it. Any member can add
    theirs to any transaction, received ones too.
- **Approval deadlines.** A payment keeps the standard expiry (about 50
  minutes), so it looks like any other wallet's on-chain.
  - The sender sees how long approvers will have, then when approvals are
    needed by. A warning shows when 10 minutes are left.
  - Approvers see an **Approve by** time. An expired request can only be
    dismissed.
- **Ask again** rebuilds a payment that ran out of time and asks the same
  members.
- **Try sending again** re-sends an approved payment that didn't reach the
  network, without new approvals.
- **[Voting in Cyze](docs/VOTING.md)**, a guide to group polls and community
  votes.

### Changed
- A send stops when the chain gets too close to its expiry for nodes to accept
  it, instead of after a fixed 35 minutes. The request then leaves approvers'
  screens.
- Home shows **Retry now** after a failed sync and **Restart** after a minute
  without progress, instead of a permanent **Sync now**.
- Inline styles moved into stylesheet classes on one spacing scale.
- README rewritten for 0.3, with how the wallet works.

### Removed
- The memo-based coinholder vote (Vote Cast Memo v1), replaced by Votes.

### Fixed
- History showed a payment's fee inside its amount, and no recipient or memo,
  because the wallet learned of its own sends only by scanning. The sending
  device now records each payment in its wallet once the network accepts it.
  Other members' history still shows the wallet's balance change.
- A signing request whose transaction didn't fit one message to an approver
  failed with "cryptography error from snow: input error". The limit didn't
  count the context being hex-encoded, so a send spending many notes could hit
  it. Cyze now sizes the request exactly and, when it can't fit, says so before
  anyone is asked.
- A send that a lock or restart cut off showed "in progress" forever. It now
  says it ended and offers the right retry.
- A failed payment was hidden until Send was opened again. It now stays on Home
  until dismissed.
- The payment panel showed the fee from the review instead of the transaction
  actually built (and the old amount for **Send all**).
- Two colours pointed at CSS variables that don't exist.

## [0.3.0-rc.2] — 2026-10-25

Release candidate for 0.3.0. It replaces 0.3.0-rc.1, which wasn't published.

**Upgrade before NU7.** Mainnet activates NU7 at block 3,543,405 (around
November 5, 2026). After that, earlier versions can't send on mainnet.

**Upgrade together.** Co-signers on 0.3 check every payment they're asked to
approve, and refuse requests from older versions. The wallet rescans once
after the upgrade.

### Added
- **NU7 support** on the Zakura 4.0 crates.
- **Background sync.**
  - The wallet syncs from unlock to lock, whatever screen is open, and scans
    about 4.5 times faster.
  - Each member's app records the wallet's starting block when the group is
    created, so payments received before a member first opened the wallet are
    found.
- **Automatic server choice.** Cyze picks the quickest of zec.rocks's
  servers, and switches if one stops answering.
- **Optional Tor** for the wallet's traffic, through Arti, with no direct
  fallback.
- **Signers verify the transaction** (PCZT) before approving: the sighash,
  that the inputs are the group's notes, the real recipients and amounts, and
  that the change returns to the group.
- **One approval per payment,** however many notes it spends.
- **Approvals are joined in the background,** with system notifications.
- **Encrypted key backups**, restorable on a new device, with a reminder while
  a group isn't backed up.
- **Private viewing keys.** New groups get one, agreed by their members. An
  older group's members can agree on one later and move the funds across.
- **Invite codes.** One code carries everything a member needs to join.
- **Safety codes** to confirm contacts' keys.
- **Send all**, with the fee taken from the amount.
- **Local devnet** (thus-spoke-zakura 0.3 or 0.4), found with **Detect local
  devnet**, with the chain's upgrade read from the node.
- **Third-party notices** under Settings → About, and SBOMs on each release.
- **SECURITY.md** with private vulnerability reporting, and a **PRIVACY.md**
  notice.
- **Tests:** frontend smoke tests, and end-to-end devnet tests (a send, a
  reorg, making a group private).

### Changed
- **A simpler app.**
  - Home, Approvals, Groups and Settings replace nine screens.
  - Home has one balance, with Receive and Send.
  - Payments take two steps.
  - The main screens use plain words.
  - Expert options sit behind **Show advanced options**.
- **Privacy at rest.**
  - Nothing outside the keystore names a group.
  - The ceremony history left the web view's storage.
  - Wallet folders have opaque names.
  - Logs leave out group ids.
- **Server certificates are pinned exactly,** and key-derivation settings read
  from files are bounded.
- **The bundled cloudflared is pinned and checked against its SHA-256.**
- **CI runs** the Rust tests, clippy, cargo-deny and an npm audit.
- **Licensed under MIT.**

### Removed
- The separate Server and Session screens, now Settings → Connection.
- ZcashNames lookups on mainnet, until a production directory exists.

### Fixed
- Approvals showed amounts as "ZEC" on every network. They now use the
  network's ticker (ZEC, TAZ or rZEC).
- The group page encoded addresses for testnet when no network had been saved.
- Members ticked to approve before your identity loaded were unticked.

## [0.2.0-rc.1] — 2026-08-16

First release candidate for the **Ironwood (NU6.3)** feature wave. Still targets
release-candidate Zcash libraries; test on testnet first.

### Added
- **Ironwood (NU6.3) wallet.** Post-NU6.3 sends build **V6 transactions**;
  balances read the Ironwood pool. Any legacy Orchard funds are surfaced only
  when present, with a one-tap sweep into Ironwood.
- **Pipelined sync — now the standard driver.** Overlaps block download with CPU
  trial-decryption and streams blocks straight from the network to the scanner,
  hiding network latency behind scanning (biggest win on high-latency links).
- **Tailscale serve hosting.** Coordinators can publish the embedded `frostd` to
  their tailnet at a stable `*.ts.net` address with automatic, publicly-trusted
  TLS (tailnet-only, not public), with in-app **Get Tailscale** / **Sign in**
  helpers — alongside the existing Cloudflare tunnel, Direct, and NGINX options.
- **Active-wallet model.** The app works on one wallet at a time; a **Zcash →
  Wallets** switcher selects the active group and stops syncing the previous one.
- **In-app Diagnostics log** (Wallet Settings) — captures the app's runtime logs
  for easy copy/share while troubleshooting; in-memory only.

### Changed
- **Ironwood-first wallet UX.** The headline total is the Ironwood balance; the
  always-on Orchard/Ironwood pool split is gone, and user-facing "Orchard"
  wording was removed in favor of Ironwood / neutral terms.
- **Calmer mainnet UX.** Removed the passive "you are on mainnet" banners and the
  network-switch modal; the wallet page now shows a small **Mainnet/Testnet**
  pill, and a mainnet send keeps one slim confirmation before broadcasting.
- Trimmed verbose in-app copy; DKG wizard section titles are now bold headings.
- Bumped the Zcash crate cohort to the latest Ironwood release candidates
  (`zcash_client_backend` 0.24.0-rc.7, `zcash_client_sqlite` 0.22.0-rc.8, with
  `zcash_protocol` 0.10.4, `pczt` 0.9.3, `orchard` 0.15.5).

### Fixed
- Wallet setup could sit on "Setting up the group's view-only wallet…" forever
  with nothing in the logs. Setup now logs each step and **times out** its
  lightwalletd RPCs, and a failed wallet-status read surfaces an error with a
  Retry instead of a permanent spinner.
- Syncing against a **pre-Ironwood testnet** lightwalletd no longer aborts: the
  driver skips the unsupported Ironwood subtree-roots request and keeps going.
- "database is locked" during overlapping syncs — syncs are serialized behind an
  app-wide gate so a restart can't race a cancelled sync's open connection.

### Removed
- The experimental pipelined-sync toggle (pipelined is now the only path) and,
  with it, the stock `zcash_client_backend::sync::run` path and its on-disk
  block cache.

## [0.1.0]

Initial pre-release: threshold DKG and signing over `frostd`, envelope-encrypted
keystore with a one-time recovery code, embedded server with Cloudflare-tunnel
exposure, and the first Zcash wallet support.
