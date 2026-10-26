# Security model

What protects a Cyze group's keys, its privacy and its payments. Cyze is beta
software and **has not been security-audited**. To report a vulnerability,
see [SECURITY.md](../SECURITY.md).

## Keys and backups

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
- **Share repair** rebuilds a lost share with as many other members as the
  threshold (FROST's repairable threshold scheme), without moving the funds.
  The rebuilt share is checked against the group's record before it's kept.
  Other members switch to the member's new key only once enough helpers vouch
  for it.
- **Keystore writes** are atomic and flushed to disk, and the previous version
  is kept. Unlocking falls back to it if the current file is damaged.
- **Key-derivation settings** read from a keystore or backup file are bounded,
  so a tampered file can't make an unlock exhaust memory or hang.

## Privacy

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
- **Read-only access** hands over the viewing key only after the member
  confirms what it reveals: every payment, past and future, and it can't be
  taken back.
- **Nothing outside the keystore names a group.**
  - Per-group settings, contact nicknames, group records, signed transactions
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
  on-chain. Reasons, payees and polls never go on chain.

## Approving payments

- **Signers verify the transaction itself.** The sender attaches the unsigned
  transaction (PCZT). Each signer's app checks:
  - the message to sign is that transaction's sighash;
  - every input carrying value is one of the group's own notes;
  - every recipient and amount matches its on-chain commitment;
  - the sender's description matches what was proven (for a batch, every
    payment, in any order).

  If any check fails, it refuses automatically.
- **One approval per payment.** A payment that spends several notes needs one
  approval; its other rounds are approved once they verify as the same
  transaction.
- **Group rules, enforced by every approver's app.** Pay only trusted payees,
  more approvals for larger payments, a wait with veto before larger payments,
  and a spending limit for automatic approval. The sender's and each
  approver's app refuse a payment that breaks the rules as they see them. With
  no contracts on shielded funds, rules hold unless as many members as a
  payment needs act together, like the wallet itself. They stop a careless
  approval, a swapped address or one compromised device.
- **Automatic approval is each member's choice.** An app approves by itself
  only for payments to trusted payees that no other rule holds back, at a
  usual fee, within what's left of the limit. It counts everything the wallet
  sent in the period, and only with a fresh sync.
- **Polls are checked against each approver's own copy.** A payment that
  carries out a poll is refused if another member sends it, if it pays
  anything else, or if the poll was paid already. Under a wait rule, each app
  counts the wait from when it received the announcement.
- **Scheduled payments are ordinary payments.** The author's app starts them;
  every check and rule above applies, and nothing is approved for a schedule
  in advance.
- **Names come from your own app.** A payment to one of the group's payees is
  named from the approver's own copy of the list, by the address the
  transaction verifiably pays. Only the member who added a payee can change it.
  A payment to one of the approver's own groups says so, checked against that
  group's keys on their device. A reason is the sender's word, shown as theirs.
- **No unverified signing.** Wallet keys never sign a request without a
  transaction attached, unless advanced options are on.
- **Nothing is at risk before approval.** Round-1 commitments don't depend on
  the message.

## Servers and people

- **frostd is an untrusted relay.** All ceremony messages and group records
  are end-to-end encrypted (Noise, via frost-client).
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
- **No one can write as another member.** Each member's app sends only the
  records its member wrote, and the encrypted channel authenticates the
  sender, so votes, payees and rules can't be forged.
