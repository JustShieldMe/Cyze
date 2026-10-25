# Privacy

Cyze is a desktop app. It has no accounts, sends no telemetry or analytics,
and its developers receive nothing from it. This page lists everything it sends
over the network, to whom, and what it keeps on your computer.

## What leaves your computer

### The sync server (lightwalletd)

To show your group's balance and send its payments, the wallet talks to a
Zcash sync server: by default the quickest of zec.rocks's servers, which it
measures by contacting each of them briefly; you can choose another in
Settings → Wallet.

- It downloads compact blocks for the whole range since your wallet started,
  not just yours, and finds your group's payments on your computer. The server
  doesn't learn which payments are yours from what you download.
- It fetches the chain's state at the height where your wallet starts, which
  suggests roughly when you began using it.
- It sends your group's signed transactions to the network through the server.
  They're shielded: the server doesn't see amounts, recipients or memos.
- The server sees your IP address and when you sync, unless Tor is on.

**Tor.** With Settings → Wallet → Privacy → **Connect through Tor**, every
connection to a sync server goes through Tor instead, so neither the server
nor anyone watching your network can tie them to your IP address. While it's
on, the wallet never connects to a sync server directly.

### The coordination server (frostd)

Members of a group reach each other through a coordination server, which one of
you runs (Cyze includes one). Everything members send each other through it is
end-to-end encrypted: the server can't read payment details, key material or
approvals. It does see each member's public communication key, their IP
address, and when they connect. This traffic doesn't go through Tor.

When the organizer makes their server reachable through a **Cloudflare tunnel**
or **Tailscale**, that provider carries the (still end-to-end encrypted)
traffic and sees the same kind of connection details.

### Polls and community votes

- **Group polls** travel through your group's coordination server like
  everything else members send each other: end-to-end encrypted, so the server
  sees only that members exchanged messages, not the questions or answers.
- **Zcash community votes**: to show what's being voted on, the app reads the
  public round list and results from ValarGroup's voting config at
  `voting.valargroup.org` and the vote servers it lists. The requests are the
  same for everyone and say nothing about your wallet; they go through Tor
  while it's on.

### Other

- **Name lookups** (ZcashNames, testnet only): the name you type is sent to the
  ZcashNames indexer at `light.zcash.me`. They're off on mainnet, and while Tor
  is on.
- **Notifications** go to your operating system's notification service. Their
  text is generic ("A payment is waiting for your approval.") because it can
  show on a lock screen.
- Cyze doesn't check for updates or contact any other service.

## What stays on your computer

- **Keystore**: your key shares, communication key, contacts and each group's
  viewing seed, encrypted with XChaCha20-Poly1305 under a key protected by your
  passphrase and your recovery code (Argon2id).
- **Wallet databases**: one per group and network, encrypted with SQLCipher
  under a key derived from the keystore.
- **Private settings and history**: per-group settings, contact nicknames, the
  history of payments and approvals, and signed transactions waiting to be
  sent, all encrypted under keys derived from the keystore.
- **Plain settings** (`settings.json`): your network, server addresses,
  certificates you chose to trust, and preferences. Nothing in it names a group
  or a contact.
- **Diagnostics log**: kept in memory only, cleared when the app quits, with
  group ids removed.
- **Backups** are files you save where you choose, encrypted like the keystore.

Anyone who knows a group's id could see the payments of a group created before
Cyze 0.3, until its members make its viewing key private (see the user guide).
