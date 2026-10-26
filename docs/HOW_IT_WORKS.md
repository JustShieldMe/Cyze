# How Cyze works

A group's members each hold one share of a FROST key; Cyze runs the ceremonies,
keeps the shared wallet in sync on every member's device, and carries the
group's coordination (polls, payees, rules, schedules) in records the members
send each other. [PRIVACY.md](../PRIVACY.md) lists everything Cyze sends and
keeps; the [security model](SECURITY_MODEL.md) covers what protects each part.

## A group is created

- The members run a distributed key generation (DKG) through the coordination
  server. Each ends up with one key share, and the full private key never
  exists anywhere.
- They also agree on a random viewing seed, so the group's public key isn't
  enough to see its payments. Each member contributes, and everyone compares a
  digest of the result.
- Each member's app records the chain height at that moment as the wallet's
  starting point.

## The wallet stays in sync

- Each member's app downloads compact blocks from a lightwalletd server.
- It finds the group's payments on the member's own computer, with the viewing
  key. The server can't tell which payments are the group's.
- [Sync design](SYNC_OPTIMIZATION.md) has the details and measurements.

## A member sends a payment

1. Their app builds the unsigned transaction (a PCZT) from the group's notes,
   with the standard ZIP-317 fee, checks it against the group's rules, and
   sends it to the approvers they chose.
2. Each approver's app checks the transaction itself before showing it: that
   the hash to sign is the transaction's, that the inputs are the group's
   notes, the real recipients and amounts, and that the change returns to the
   group. It then checks the group's rules, and any poll the payment carries
   out, against its own records.
3. When the approvers approve, their apps contribute signature shares. The
   sender's app combines them, builds the zero-knowledge proof, and broadcasts
   the transaction.

A batch is one transaction with up to 20 payments; a scheduled payment is an
ordinary payment that the author's app starts on its date.

## Group records

Polls, votes, payees, trust, reasons, rules and schedules travel as **group
records** (`core/src/polls.rs`):

- Each member numbers the records they write. Their app sends them to the
  others end-to-end encrypted over the coordination server, and asks for any
  it's missing.
- A member's app only sends records its member wrote, and the encrypted
  channel authenticates the sender, so no one can write as someone else.
- Every member's app works out the same state from the same records: the
  payees and who trusts them, the rules in effect (the latest set the group
  adopted by poll), each schedule and when it was last sent.
- Records of a kind an older version doesn't know are kept and skipped, so
  members on different versions stay in sync.

## Rules are enforced by apps

Zcash has no contracts for shielded funds. A group's rules are enforced by each
member's app: the sender's app refuses to ask for a payment that breaks them,
and each approver's app refuses to sign one. Automatic approval within a
spending limit is each member's own choice, and counts everything the wallet
sent in the period, however it was approved.

## The servers are untrusted relays

- Everything members send each other through the coordination server is
  end-to-end encrypted (Noise). The server sees public communication keys, IP
  addresses and timing.
- The sync server sees which blocks are downloaded and when, and that a
  transaction was broadcast. It sees your IP address unless Tor is on.

The code is laid out in [Development](DEVELOPMENT.md#code-layout).
