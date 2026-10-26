# Features

Everything Cyze does, by area. The [user guide](USER_GUIDE.md) shows how to
use each one; the [security model](SECURITY_MODEL.md) says what protects it.

The app has five places: **Home** (the active group's wallet), **Approvals**,
**Votes**, **Groups** and **Settings**.

## Wallet

- **Receive** shows a rotating shielded address, with a QR code.
- **Send** takes two steps: enter the payment, then review it and choose who
  approves.
  - **Send all** pays everything, the fee coming out of the amount.
  - **Pay several at once:** up to 20 payments in one transaction, approved
    once. Enter them one by one or paste a list.
  - Transparent addresses are detected, with a note that the payment will be
    public.
  - **ZcashNames (testnet):** send to `name.zcash`. The resolved address is
    shown for you to confirm before anything is signed. Lookups are off on
    mainnet until a production directory exists.
- **Activity** lists payments with their memos, fees, payees, reasons and who
  signed. **Export CSV** saves it for accounting.
- **Sync** runs in the background from unlock to lock, on any screen.
  - Each member's app records the wallet's starting block when the group is
    created, so no payment to it is missed.
  - Scanning is about 4.5 times faster on the Zakura crates.
  - Cyze picks the quickest of zec.rocks's servers, and switches if one stops
    answering. **Tor** for the wallet's traffic is optional.
- With several groups, a switcher at the top picks the one wallet the app syncs
  and acts on.

## Approvals

- The app joins signing requests by itself and shows what it verified from the
  transaction: who's paid, how much, and what returns to the group. A system
  notification says when a request is waiting.
- Each approver approves a payment once, however many notes it spends.
- Approvals must come within the transaction's standard expiry (about 50
  minutes). Both sides see the deadline.
- **Ask again** rebuilds a payment that ran out of time, for the same members.
  **Try sending again** re-sends an approved payment that didn't reach the
  network, without new approvals.
- Approvers see a payee's name from their own copy of the list, the reason the
  sender gave, which of the group's rules the payment meets, and whether it
  goes to one of their own groups.

## Coordination

Tools for running a treasury together, in the spirit of Safe. Zcash has no
contracts for shielded funds, so each member's app enforces them when deciding
whether to sign (see [Group rules](SECURITY_MODEL.md#approving-payments)).

- **Payees:** a shared list of the addresses the group pays, by name. Only the
  member who added a payee can change or remove it. A payee is **trusted** once
  as many members trust its address as a payment needs approvals.
- **Reasons:** a payment can say what it's for. Approvers see it, and so does
  every member's history. It never goes on chain.
- **Polls:** any member asks the group a question. An option is agreed once as
  many members back it as a payment needs.
- **Pay if agreed:** a poll can propose payments, answered **Pay** or **Don't
  pay**. Once Pay is agreed, its author sends exactly those, once, and each
  approver's app checks the transaction against its own copy of the poll.
- **Scheduled payments:** payments that repeat weekly, monthly or yearly. On
  each date the author's app asks the approvers like any other payment. A
  payment more than 3 days late is skipped.
- **Rules**, adopted by poll:
  - **Pay only trusted payees.**
  - **More approvals for larger payments**, from an amount you choose.
  - **A wait with veto for larger payments:** they're announced in a payment
    poll, and can be sent once the wait has passed with nobody choosing Don't
    pay.
  - **Automatic approval within a spending limit:** members who turn it on let
    their app approve payments to trusted payees by itself while the wallet
    has sent less than the limit in the period.
- **Read-only access:** share the wallet's viewing key with an accountant or
  auditor, after a plain warning of what it reveals.

## Groups

- **Joining takes one invite code.** Creating a group starts the app's
  coordination server and a link members can reach (Tailscale if it's set up,
  otherwise a Cloudflare tunnel).
- **Contacts** carry safety codes, so people can confirm they hold each other's
  real keys. Invites add new keys only after you confirm them.
- **Private viewing keys:** new groups agree on one when they're created, and
  older groups can be made private later.
- **Change members or threshold:** a guide on the group's page moves the group
  to a new one: create it, copy the payees, propose the same rules, and send
  everything to it for the usual approvals.
- **Share repair:** a member who lost their device and backup gets their share
  back from as many other members as the threshold, without moving the funds.

## Votes

- **Group polls**, including payment and rules polls (above).
- **Zcash community votes:** the coinholder rounds run through
  [Shielded Vote](https://valargroup.gitbook.io/shielded-vote-docs).
  **Decide together** turns a round's proposals into group polls. Casting the
  group's vote with its balance comes in a later version. See
  [Voting in Cyze](VOTING.md).

## Keys and backups

- A passphrase or the 12-word recovery code unlocks the keystore.
- Encrypted backup files restore your shares on a new device. The app reminds
  you while a group isn't in any backup yet.

## Networks and hosting

- **Hosting:** run the bundled `frostd` coordination server and reach it
  through a Cloudflare tunnel, Tailscale, a direct address (pinned self-signed
  TLS) or your own domain; or point at any external `frostd`.
- **Networks:** mainnet, testnet, and a local
  [thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura) regtest
  devnet for development (0.3 runs NU6.3, 0.4 runs NU7), found with one click.
  Cyze supports NU6.3 (Ironwood) and NU7.
- **Advanced options**, off by default: test networks and the devnet, a
  diagnostics log, Ed25519 groups and raw-message signing.
- **Ciphersuites:** RedPallas (re-randomized FROST for Zcash shielded spends;
  every wallet group) and Ed25519 (general-purpose signing, an advanced
  option).
