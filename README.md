# CYZE — Coordinate Your ZCash Easily ❄️

**A desktop wallet for Zcash funds that a group controls together.**

Cyze puts a graphical interface on [ZF FROST](https://frost.zfnd.org/) threshold
signatures, built on the Zcash Foundation's
[frost-tools](https://github.com/ZcashFoundation/frost-tools). A group jointly
controls a shielded Zcash wallet: **no one ever holds the full private key**,
and a threshold of members (for example 2 of 3) must approve every payment. On
top of that, groups get Safe-style tools to run a treasury together.

> ### ⚠️ Beta software — unaudited — use at your own risk
>
> Cyze has **not been security-audited**, and its Zakura wallet crates are
> still release candidates. Don't hold funds you can't afford to lose, back up
> your key shares and recovery code, and start on testnet. **No warranty is
> provided.**
>
> **Mainnet activates NU7 at block 3,543,405 (around November 5, 2026).** Cyze
> 0.3 supports it; earlier versions can't send on mainnet after that.

## Features

- **A shared shielded wallet:** receive, send one payment, everything, or up to
  20 at once, and export the history as CSV. It syncs in the background,
  optionally over Tor.
- **Verified approvals:** each approver's app checks the transaction itself
  (inputs, recipients, amounts, change) before showing it.
- **Coordination:**
  - shared payees, and reasons on payments;
  - polls, including payments the group agrees to make;
  - scheduled payments;
  - rules every member's app enforces: trusted payees only, more approvals or
    a veto window for larger payments, and automatic approval within a
    spending limit.
- **Groups:**
  - create or join with one invite code;
  - contacts with safety codes;
  - private viewing keys;
  - read-only access for accountants;
  - changing members or the threshold.
- **Recovery:** a passphrase or a 12-word recovery code, encrypted backups,
  and repair of a member's lost share without moving the funds.
- **Votes:** group polls and Zcash community votes.

The full list is in [Features](docs/FEATURE_LIST.md).

## Getting started

Download an installer from the
[Releases page](https://github.com/JustShieldMe/Cyze/releases), or
[build from source](docs/DEVELOPMENT.md). Builds aren't code-signed yet, so
macOS and Windows show a warning the first time you open the app.

1. Set a passphrase and save the one-time recovery code.
2. **Start a shared wallet:** add each member's contact code, choose how many
   must approve, and click **Create invite**.
3. Members paste the invite and click **Join**. The wallet opens on **Home**
   for everyone.
4. Each member saves a backup from **Settings → Security**.
5. **Send** asks the members you choose. They approve under **Approvals**.

The [user guide](docs/USER_GUIDE.md) covers every feature step by step.

## Security in brief

- The full key never exists in one place. Each member's share sits in a
  keystore encrypted under their passphrase and recovery code.
- Approvers sign the transaction they verified, not a description of it.
- Each member's app finds the group's payments under a private viewing key.
  The servers are untrusted relays, and members' messages are end-to-end
  encrypted.
- Rules are enforced by members' apps, not on chain. They hold unless as many
  members as a payment needs act together.

More detail is in the [security model](docs/SECURITY_MODEL.md),
[privacy](PRIVACY.md) and [how it works](docs/HOW_IT_WORKS.md). To report a
vulnerability, see [SECURITY.md](SECURITY.md).

## Documentation

| Document | What's in it |
| --- | --- |
| [User guide](docs/USER_GUIDE.md) | Every feature, step by step |
| [Features](docs/FEATURE_LIST.md) | Everything Cyze does |
| [How it works](docs/HOW_IT_WORKS.md) | Groups, sync, payments, group records |
| [Security model](docs/SECURITY_MODEL.md) | Keys, privacy, approvals, servers |
| [Voting](docs/VOTING.md) | Group polls and Zcash community votes |
| [Privacy](PRIVACY.md) | What the app sends, to whom, and what it keeps |
| [Development](docs/DEVELOPMENT.md) | Building, tests, code layout |
| [Releasing](docs/RELEASE.md) | Packaging, sidecars, code signing, SBOMs |
| [Sync design](docs/SYNC_OPTIMIZATION.md) | How the wallet syncs, with measurements |
| [Changelog](CHANGELOG.md) | What changed in each version |

## Building

```sh
npm install
./scripts/build-sidecar.sh   # required: fetches the frostd and cloudflared sidecars
npm run tauri dev
```

Prerequisites, tests and the code layout are in
[Development](docs/DEVELOPMENT.md).

## License

[MIT](LICENSE). The software Cyze is built on, including the bundled `frostd`
and `cloudflared`, keeps its own licenses. **Settings → About** lists them with
their texts, and each release carries SBOMs.
