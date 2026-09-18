# Changelog

All notable changes to Cyze are recorded here. Versions follow
[semantic versioning](https://semver.org); pre-1.0 minor bumps may include
breaking changes. Cyze is beta, unaudited software (see the README).

## [Unreleased]

### Added
- **Local devnet (regtest) network.** Point the wallet at a local
  [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) regtest
  devnet for Zcash development. **Detect local devnet** finds its randomly
  assigned lightwalletd port; the chain's network upgrade is read from the node
  automatically (or forced in settings). Devnet wallets use `uregtest1…`
  addresses, an `rZEC` ticker, their own wallet data, and scan the whole devnet
  chain so faucet funds show up right away. On a devnet below NU6.2 (the stock
  launcher runs NU6), syncing and receiving work but sends will likely be
  rejected: Cyze can only build Orchard proofs with the NU6.2+ circuit.

### Fixed
- The approver's Inbox showed amounts as "ZEC" on every network; it now uses the
  request's ticker (ZEC / TAZ / rZEC).
- The group page encoded addresses for testnet when no network had been saved,
  while the wallet used mainnet; both now resolve the network the same way.

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
