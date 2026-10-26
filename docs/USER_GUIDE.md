# Cyze — User Walkthrough & Testing Guide

A step-by-step guide to exercising every major feature end to end. Follow it in
order to vet a build.

> **Beta software, unaudited.** Start on **testnet** (free faucet funds). Only
> move to mainnet once you trust the flow, and then with small amounts.

## Getting the app

**Option A — download a prebuilt installer (recommended).** Grab the build for
your operating system from the
**[Releases page](https://github.com/JustShieldMe/Cyze/releases)** (open the latest
release and download the matching asset):

| OS | Download |
| --- | --- |
| **Windows** | `.msi` (or the `.exe` installer) |
| **macOS** | `.dmg` — Apple Silicon or Intel, whichever matches your Mac |
| **Linux** | `.AppImage` (portable), or `.deb` / `.rpm` for your package manager |

The FROSTd and tunnel helpers are bundled inside, so there is nothing else to
install. Builds aren't code-signed yet, so macOS and Windows warn the first
time you open the app. On macOS, open **System Settings → Privacy & Security**
and click **Open Anyway**; on Windows, choose **More info → Run anyway**.

**Option B — build from source.** For the latest code or an unlisted platform:

```sh
git clone https://github.com/JustShieldMe/Cyze.git
cd Cyze
git checkout main
npm install
./scripts/build-sidecar.sh   # fetches both bundled sidecars (required)
npm run tauri build          # native installer, or `npm run tauri dev` to run
```

See the [README](../README.md) for build prerequisites (Rust, Node, and the
Linux system packages).

**Testing setup.** Cyze coordinates multiple people, so most flows need two
parties. The easiest way to test alone is two instances on one machine — give the
second its own data directory (works with an installed build or a dev build):

```sh
# installed build (example paths — use your platform's launch command)
cyze                                  # instance A (coordinator)
FROST_APP_DATA_DIR=/tmp/cyze-b cyze   # instance B (participant)

# or from source
npm run tauri dev                                # instance A
FROST_APP_DATA_DIR=/tmp/cyze-b npm run tauri dev # instance B
```

Throughout, **Coordinator** = the person who hosts the server and drives a
ceremony; **Participant** = someone who connects to that server and approves.

---

## 1. High-level features

Cyze is a desktop wallet for Zcash where **no one person holds the private key**.
A group jointly controls the funds using [FROST](https://frost.zfnd.org/)
threshold signatures: a set number of members (for example 2 of 3) must
approve every payment.

The app has five places:
- **Home:** the group's wallet.
- **Approvals:** payments waiting for you.
- **Votes:** polls in your groups, and Zcash community votes.
- **Groups:** your groups and contacts.
- **Settings:** connection, wallet, security, advanced options and licenses.

It covers the whole lifecycle:
- creating a group (distributed key generation, with one invite code);
- a shared Zcash wallet that syncs in the background (receive, balances,
  history);
- payments that each approver's app verifies before they approve;
- a shared list of the group's payees, and reasons on payments, that every
  member sees;
- batch, scheduled and poll-approved payments, under rules the group adopts
  (trusted payees, more approvals or a wait for larger payments, automatic
  approval within a limit);
- accounting export, read-only access, and repairing a member's lost share.

It supports NU6.3 (Ironwood) and NU7, which mainnet activates at block
3,543,405 (around November 5, 2026).

---

## 2. Application security & passphrase

On first launch you create your local keystore.

1. **Set a passphrase** and a **display name**, then confirm the passphrase.
2. You are shown a **one-time 12-word recovery code**. Copy it and store it
   somewhere safe — it is displayed **only once** and is never saved by the app.
3. Tick the acknowledgement and click **"I have saved my recovery code —
   continue."**

**What to verify:**
- Lock the app (or restart) and **unlock with your passphrase**.
- Lock again and **unlock with the 12-word recovery code** instead — both work
  independently, so either can recover access.
- A wrong passphrase or code is rejected cleanly.

Your key shares, contacts, and everything that names a group (wallets, signing
history, contact nicknames) are stored encrypted at rest. The passphrase and the
recovery code each unlock the keystore.

**Back up your keys** once you're in a group. A banner reminds you, or use
**Settings → Security**. The backup file is encrypted and opens only with
your passphrase or recovery code, so keep it somewhere other than this computer.
To restore on a new device, choose **Restore from a backup** on the first screen,
then unlock with your passphrase, or with your recovery code if you've forgotten
it. Without a backup file, the recovery code alone can't bring back a lost
device's key shares.

### Repairing a lost share

If you lost your device and have no backup, the other members can **repair**
your share: as many of them as the group's threshold rebuild it on your new
device, without learning it and without moving the funds. A group where every
member must approve (2 of 2, 3 of 3) can't repair a share.

**On your new device:**
1. Set up Cyze (a new keystore), then open **Groups → Recover a group**.
2. Send a member your **contact code**, and read them its **safety code** on a
   call or in person, so they know the new key is yours.
3. Paste the **repair code** they send back and click **Recover**. Keep Cyze
   open until the helpers have approved; the group then appears under Groups,
   its wallet syncing from where it started.

**As a helper:**
1. On the group's page, open **Recovery & share repair → Repair a member's
   share**, choose the member, and paste their new contact code.
2. Read its safety code with them and tick that it matches. Choose the helpers
   (at least the threshold, you included), then **Start repair**, and send the
   member the repair code it shows.
3. The other helpers get the request under **Approvals**, with the same safety
   code to compare, and click **Approve and help**.

Your app checks the rebuilt share against the group's public record of it
before keeping it. Afterwards the group uses your new key: the helpers' apps at
once, and the other members' apps once as many helpers vouch for it as a
payment needs.

**Lost or stolen?** Repair rebuilds the *same* share. If your old device might
have been stolen, whoever has it still holds the share: don't repair, form a
new group and move the funds instead.

---

## 3. Managing contacts

Contacts are the people you run ceremonies with, identified by a communication
public key. They're listed under **Groups**, below your groups.

**Share your contact:**
1. Under **Your contact code**, enter your name and click **Show** to get your
   contact code (begins with `zffrost1…`).
2. Copy it and send it to the others out-of-band (chat, email).

**Import a contact:**
1. Paste the `zffrost1…` code a peer shared into **Add a contact.**
2. Optionally add a nickname (a local label only you see) and save.

Imported contacts appear in your **address book** and become selectable when you
create a group. Exchange contacts in both directions before continuing.

**Check safety codes.** A contact code passed through chat could be swapped by
someone in between. Click **Check** next to a contact, ask them to read you
their safety code by phone or in person, and tick **It matches** if it's the
number shown. Read them yours too (it's under **Your contact code**). Verified
contacts are marked in the list and in invites.

---

## 4. Setting up the FROSTd server

All ceremony traffic flows through a FROSTd coordination server. It is an
untrusted relay — messages are end-to-end encrypted — so one party hosts it and
the others connect.

**You usually don't need this section.** Creating a group starts your server and
a public link automatically (or Tailscale, if you chose it here), and an invite
tells members how to connect. Use **Settings → Connection** to choose Tailscale,
or to set things up by hand as below.

### As the host (coordinator)

1. **Start the embedded server.** It binds to loopback on your machine.
2. Choose how remote participants reach it:
   - **Cloudflare Tunnel** *(quickest for remote testing)* — click **Open
     public tunnel** to get a public `https://…trycloudflare.com` URL with valid
     TLS. Share that URL with participants. **Note:** the URL is disposable — a
     new one is generated every time you restart the tunnel, so always share the
     current one.
   - **Tailscale** *(recommended for a stable link)* — if you and your
     participants are on the same tailnet, click **Publish to tailnet** to serve
     the embedded server at a stable `https://<name>.ts.net` address with
     automatic, publicly-trusted TLS. The URL is **reusable across launches** (no
     cert-trust step), and access stays **tailnet-only** — not public. If
     Tailscale isn't installed or signed in, the tab offers **Get Tailscale** /
     **Sign in** to get there.
   - **Direct URL / IP** — for participants on your LAN or reachable by IP. Share
     the URL, the certificate **fingerprint**, and the self-signed **certificate**
     (participants must trust it once).
   - **NGINX reverse proxy** — for a stable domain with your own TLS.

#### Tailscale prerequisites

Tailscale serving needs a few things enabled once — without them, **Publish to
tailnet** fails (often silently on older builds). Set these up first:

1. **Install Tailscale and sign in** on the coordinator's machine.
2. In the [Tailscale admin console](https://login.tailscale.com/admin/dns) → **DNS**, enable:
   - **MagicDNS** — gives the machine its stable `<name>.<tailnet>.ts.net` name.
   - **HTTPS Certificates** — **required**; `tailscale serve` uses it to present
     valid TLS on port 443. This is the most common cause of a failed publish.
3. **Serve permission (operator).** On **Windows/macOS** the signed-in desktop
   user is normally the operator, so nothing to do. On **Linux**, grant it once:
   `tailscale set --operator=$USER`.
4. **Every participant must be signed in to the same tailnet** — Tailscale serve
   is tailnet-only (not public).

Once these are set, **Publish to tailnet** shows a green *"Verified: serving …"*
confirmation. If it fails, the app names the reason (HTTPS Certificates not
enabled, or operator permission). To check from a terminal, using the port shown
in the server status (`https://127.0.0.1:<PORT>`):

```sh
tailscale serve --bg --https=443 https+insecure://127.0.0.1:<PORT>
tailscale serve status    # should list the mapping
```

### As a participant

An invite sets this up for you. To connect by hand instead:

1. In **Connect to the organizer's server**, paste the URL the coordinator gave
   you. It looks like `https://frost.example.com`, `https://203.0.113.7:2744`,
   `https://…trycloudflare.com`, or a Tailscale `https://<name>.ts.net`.
2. Click **Test connection** — a success message confirms the server, its TLS
   trust, and latency.
3. For a Direct-URL (self-signed) server, expand **"Trust its certificate,"**
   paste the PEM the coordinator shared, verify the fingerprint out-of-band, and
   trust it. (Not needed for a Tunnel, Tailscale, or NGINX server — those already
   present publicly-trusted TLS.)
4. **Save & use this server.**

**What to verify:** the participant's Test connection succeeds against the
coordinator's live server before moving on.

---

## 5. DKG ceremony (create a group)

Distributed Key Generation creates the shared key. Everyone ends up with one
share; the full private key never exists anywhere. Go to **Groups → New group**
(or, from an empty Home screen, **Start a shared wallet**).

### Organizer

1. Collect each member's **contact code** (they find it under **Groups →
   Contacts**, or on the **Join with an invite** screen) and add them under
   **Members besides you**, either from your contacts or by pasting the code.
2. Enter a **name** and choose how many members must **approve a payment**.
3. Groups are Zcash wallets (RedPallas). With **Show advanced options** on, you
   can choose **Ed25519** instead, for general-purpose signing without a wallet,
   and tick **A member uses the frost-client command-line tools** if one does.
   That tool can't help agree on a private viewing key, so such a group's
   payments are visible to anyone who learns its group key.
4. Click **Create invite.** The app starts your server and a link members can
   reach, then shows the **invite**. Copy it and send it to the members.
5. Keep the app open until everyone has joined.

### Member

1. Choose **Join with an invite** (on Home, or under **Groups**).
2. Send the organizer **your contact code** (step 1 on that screen). They need
   it before they create the invite.
3. Paste the **invite** they send back. The app shows the group, how many must
   approve, and every member's key. Members already in your contacts are
   marked; anyone new is added only after you tick the box. If you can, compare
   the new keys with those people.
4. Click **Join.**

When all parties have joined, the ceremony completes and the **group** appears in
**Groups** for everyone, and its wallet opens on **Home**.

**What to verify:** the group appears on every participant's device with the same
threshold and participant count.

---

## 6. Building transactions

Spending requires the coordinator to draft a transaction and a threshold of
members to approve it.

### Coordinator — send a payment

1. Open **Home**. With several wallets, pick this group's from the switcher at
   the top.
2. Fund the group first if needed: click **Receive** and send ZEC to the
   address shown (a `u1…` on mainnet / `utest1…` on testnet). Received funds
   need **10 confirmations** before they are spendable (your own change needs
   3); until then the balance shows them as waiting for confirmations.
3. Click **Send** and enter who to pay: paste an address, or choose one of the
   group's payees from **Pay a saved payee**. Then enter the **amount**, an
   optional **memo** (only the recipient reads it), and an optional **reason**
   (only the group reads it; see [Payees and reasons](#payees-and-reasons)). A
   transparent (`t…`) address is detected automatically: the app notes that
   the payment will be public, and memos aren't available. The **Review
   payment** button explains why when it's disabled.
4. Check the payment, fee, and total, and choose which members to **ask to
   approve** (any set that meets the threshold; you are pre-selected). On
   mainnet, tick **The recipient is correct**.
5. Click **Request approvals.** The payment shows who has approved, then its
   progress to **Sent**.

**Pay several at once.** For payroll, grants or a stack of invoices, click
**Pay several at once** on the send form. Enter up to 20 payments, each with
its own recipient (typed, or a saved payee), amount and optional memo, or click
**Paste a list** and paste one per line: `address or payee name, amount, memo`.
One reason covers them all. The group approves the whole batch once, as a single
transaction, and each approver's app checks every payment in it.

A signing request has to fit in one message to each approver. Twenty payments
fit; memos make it larger, so with a memo on every payment about ten fit. If a
batch is too large, review says so before anyone is asked: split it in two.

**Approvals have about 50 minutes.** Every Zcash transaction carries a public
expiry height, and Cyze keeps the standard one other wallets use, so the group's
payments look like anyone else's on-chain. The payment shows when approvals are
needed by, and warns when 10 minutes are left. If time runs out, nothing is
sent: **Ask again** builds the same payment afresh and asks the same members.
If the payment was approved but couldn't reach the network, **Try sending
again** sends the signed transaction without asking anyone again.

### Participant — review and approve

1. A red **!** badge appears next to **Approvals** in the sidebar, with a
   system notification if Cyze isn't in front. Open it. The app joins the
   request in the background, which only shares message-independent
   commitments.
2. The request shows what your app **verified from the transaction itself**: who
   gets paid and how much, any memo, what goes back to the group, and the fee.
   If the transaction doesn't match what the coordinator described, or its
   contents don't match its commitments, your app **refuses automatically** and
   says why. A payment to one of the group's payees shows the payee's name, and
   a reason shows as "Alice says it's for: …". The reason is the coordinator's
   word: it isn't part of the transaction, so nothing checks it.
3. Click **Approve and sign** to release your signature share, before the
   **Approve by** time the request shows. Nothing is signed until you approve.
   A request that has expired can only be dismissed. A send that spends several
   notes needs this one approval; its other notes are approved automatically
   once they verify as the same transaction.

Once a threshold of members approves, the coordinator's app aggregates the
signatures, builds the proof, broadcasts the transaction, and shows the **txid**.
The transaction then appears under **Activity** on both sides, with its fee and
memo shown inline, the payee's name under **With**, and its reason.

**What to verify:** the txid confirms on a block explorer, and the sender's
balance and history update.

### Payees and reasons

**Payees** are a group's shared list of the addresses it pays, by name. They're
under **Payees** on Home.

- **Add one** with **Add a payee**, or from the send form: type an address, then
  **Save as a group payee**.
- **Everyone in the group sees it,** with who added it, within about 10 seconds
  for members who are online. Members who are offline get it later, the same
  way they get polls ([Voting in Cyze](VOTING.md#if-you-were-offline)).
- **Only the member who added a payee can change or remove it.** No two payees
  can have the same name or the same address.
- **Approvers see the name** when a payment goes to a payee. Their own app looks
  it up from the address the transaction verifiably pays, so a request can't
  pass off one address under another's name.
- **Payees belong to a network.** One saved on testnet doesn't show on mainnet.
- **Trusted payees.** A payee is *trusted* once as many members trust its
  address as a payment needs approvals. The member who added it trusts it; the
  others click **Trust** next to it under **Payees**. A trust is in the
  address: if the payee's address changes, the others have to trust it again.

A **reason** says what a payment was for. Only members of the group see it,
and it never goes on chain.

- **When sending,** fill in **Reason**. Approvers see it with the request.
- **Once it's sent,** every member's **Activity** shows it.
- **Any member can add theirs** to any transaction, received ones included:
  open the transaction under **Activity** and click **Say what it was for**.
  **Change yours** edits it; saving it empty takes it back.

### Rules

A group can adopt **rules** for its payments, under **Rules** on Home:

- **Pay only trusted payees.** Approvers' apps refuse a payment to any address
  that isn't a trusted payee.
- **Larger payments need more approvals.** For example, in a 3-of-5 group,
  payments of 100 ZEC or more (together, in one transaction) need 4 approvals.
- **Larger payments wait for vetoes.** Payments over an amount have to be
  announced first, and can be sent only after a wait (an hour to a week) in
  which no member vetoed them.
- **Small payments can be approved automatically.** Members who turn it on let
  their app approve payments to trusted payees by itself, while the wallet has
  sent less than a limit in a period (for example 1 ZEC a week).

With a spending limit in effect, each member chooses whether their own app
approves automatically: **Approve automatically on this device** under
**Rules**, which also shows how much of the limit the wallet has used. An app
approves by itself only when every payment goes to a trusted payee, no other
rule needs more approvals or a wait, the fee is usual, and the payment and its
fee fit in what's left of the limit. It counts everything the wallet sent in
the period, however it was approved, so the limit holds however many members
turn this on. It still verifies the transaction and checks every other rule
first, needs Cyze open and unlocked, and tells you when it approves. If the
wallet isn't synced, it doesn't approve anything by itself.

With a wait in effect, the send form offers **Announce it** instead of
**Review payment** for a payment over the amount. That starts a payment poll
(see [Propose a payment](VOTING.md#propose-a-payment)). Any member can veto it
by choosing **Don't pay**; nobody has to choose Pay. Once the wait has passed
with no veto, the member who announced it sends it from **Votes**, with the
usual approvals. Each member's app counts the wait from when it received the
announcement, so every member gets the whole wait to object.

To adopt or change them, click **Propose rules**, choose them, and **Ask the
group**. That starts a poll with **Adopt** and **Don't adopt**; the new rules
replace the old ones once as many members adopt them as a payment needs. A rule
set with nothing ticked removes all rules the same way.

The sender's app checks a payment against the rules before asking anyone, and
each approver's app checks it again, against the rules as it sees them, before
showing it. A payment that breaks them is refused automatically, saying why.

Rules are enforced by each member's app, not on chain: Zcash has no contracts
for shielded funds. They stop a careless approval, a swapped address or one
compromised device. They can't stop as many members as a payment needs
acting together, who could change the rules or run apps that ignore them.

### Scheduled payments

**Scheduled payments** on Home repeat a payment, like payroll or rent. Click
**Schedule a payment**, then fill in:

- the payments (one or several, as in a batch);
- an optional reason;
- how often it repeats (every week, two weeks, month, three months or year);
- the date of the first payment;
- who approves each one.

Every member sees the schedule, with its next date, who approves, and when it
was last sent.

On each date, around midday, your app asks the approvers, exactly as if you'd
sent the payment yourself. Approvers see it as an ordinary request, with the
reason starting "Scheduled:", and each one's app checks it against the group's
rules. Within a spending limit, members who turned on automatic approval have
their app approve payments to trusted payees by itself. With a limit, trusted
payees and enough members approving automatically, a schedule pays without
anyone clicking.

Your app has to be open and unlocked around each date. A payment more than 3
days late is skipped, not sent late, and the schedule says which dates it
skipped. Only one payment from a wallet runs at a time, so a scheduled one
waits for any payment in progress. One that fails, for example because
approvals didn't arrive in time, isn't asked again by itself: use **Ask again**
on Home. One that couldn't start, say because the wallet was still syncing, is
tried again a few minutes later.

A monthly schedule pays on the same day each month, or the month's last day
when it's shorter. Only the member who made a schedule can stop it (**Stop**);
payments already sent stay sent. A schedule can't make a payment the group's
rules would hold for vetoes, and Cyze refuses one the rules would refuse when
you make it.

### Accounting export and read-only access

- **Export CSV**, at the top of **Activity**, saves the wallet's history as a
  spreadsheet: date, block, type, amount and fee (with all eight decimals),
  payee, address, memo, the reasons members gave, who signed (for payments
  this device sent) and the transaction ID. A batch has one row per payment,
  with its fee on the first. The file isn't encrypted: keep it somewhere safe.
- **Share read-only access**, on the group's page under **Groups**, gives an
  accountant or auditor the wallet's viewing key. With it, a Zcash wallet that
  can watch a unified viewing key (`uview…`) shows the balance and every
  payment in and out, with amounts and memos, but can't spend. Cyze shows the
  key only after you confirm you understand that whoever has it sees all of the
  wallet's payments, past and future, and that it can't be taken back: cutting
  someone off means moving the funds to a new wallet. Agree with the group
  before sharing it.

### Make an older group private

A group created before Cyze 0.3 derives its viewing key from its group ID, so
anyone who learns the ID can see its balance and payments. Home says so for
such a group, with **Make private**:

1. Make sure every member has Cyze open and unlocked: every one of them has to
   take part, and their apps join by themselves. Click **Make private**, then
   **Start**. Home shows who the app is still waiting for.
2. When everyone has joined, the wallet gets a new, private address, and its
   balance starts again from zero. What the group holds is still at the old
   address, and Home shows how much.
3. Click **Move funds**, choose who approves, and **Request approvals**. Each
   approver's app checks that the payment only moves the group's own funds to
   its private address. The fee comes out of the amount moved.
4. **Save a new backup** on every member's device: an older backup doesn't have
   the new key, and the app shows it as out of date.

The app keeps watching the old address, so anything sent there later shows up
on Home to move as well. Payments made before the move stay visible to anyone
who knows the group ID.

### Change members or threshold

A group's members and threshold are fixed when it's created. To add or remove
members, or change how many must approve, create a new group and move the
funds to it. **Change members or threshold** on the group's page walks
through it:

1. **Create the new group** with the members and threshold you want (see
   [DKG ceremony](#5-dkg-ceremony-create-a-group)).
2. Choose the new group from your other Zcash groups.
3. **Copy payees** adds this group's payees to the new one, as yours. They
   arrive untrusted, so the new group's members trust them again.
   **Propose the same rules** asks the new group to adopt this group's rules
   in a poll. Scheduled payments stay behind: stop them here, and schedule them
   again in the new group.
4. **Move the funds** opens this wallet on Home with a payment of everything to
   the new group's address, for review. It's approved by this group's members,
   under this group's rules. If a rule makes larger payments wait for vetoes,
   announce it first.
5. Once the funds have arrived, members can remove the old group.

An approver whose app also holds the new group sees **Your group “…”** on the
request. Their app checks the address against that group's keys itself. The
new group's viewing key has to be private before the move, so the funds land
at its private address.

---

## 7. Votes

**Votes** in the sidebar has two kinds, open ones first. A red **!** means a
poll is waiting for your answer.

### Group polls

Ask your group a question: a signal, a decision to coordinate, anything you'd
otherwise settle in a chat.

1. Click **New poll**, choose the group if you're in more than one, and write
   the question, any details, and the options (Yes / No to start; up to 10).
2. Choose how long it stays open, then **Ask the group**. Every member's app
   shows it, with a system notification if Cyze isn't in front.
3. Each member picks an option and clicks **Vote**. You can **Change vote**
   until it closes. Everyone in the group sees who chose what.
4. An option is marked **agreed** once as many members choose it as the group
   needs to approve a payment (for a 2-of-3 group, two). The member who asked
   can **Close poll** early.

A poll can also **propose a payment** (tick **Propose a payment** when asking).
Members choose **Pay** or **Don't pay**; once Pay is agreed, the member who
proposed it clicks **Send payment**, and each approver's app checks the
transaction against the poll before they approve. See
[Propose a payment](VOTING.md#propose-a-payment).

Polls go through the group's coordination server, encrypted so only members
can read them, and nothing goes on chain. A member who was offline gets each
member's polls and votes once that member's app has been online too.
[Voting in Cyze](VOTING.md) covers polls and community votes in full.

### Zcash community votes

Zcash coinholders vote on proposals for the network through
[Shielded Vote](https://valargroup.gitbook.io/shielded-vote-docs): privately,
with the balance they held at the round's snapshot. Votes shows what's open
(mainnet rounds on mainnet, test rounds on testnet) and recent results.

- **Decide together** turns the proposals you pick into group polls, closing an
  hour before the vote does, so the group can agree on how it would vote.
- Casting the group's vote with its balance from Cyze comes in a later version,
  once it's been tested on a live round.

---

## 8. Settings & configuration

Network and light-client endpoint are set in **Settings → Wallet**.

**Network.** Cyze defaults to **Mainnet**. The active network is shown as a
small **Testnet / Devnet** label next to the wallet name, and a mainnet payment
asks you to confirm the recipient before it's sent for approval. Switch to
**Testnet** to test with faucet funds, and back to Mainnet when ready. **Local
devnet** is for development (see below). Balances, addresses and history are
kept entirely separate per network.

**Sync server.** By default it's **Automatic**: the app measures the known
servers (zec.rocks and its regional servers on mainnet) and uses the quickest
one that's up to date, and switches to another if that one stops answering.
Settings shows which it's using. You can pick one of them instead, or enter a
**custom** `https://host:port` URL to use your own node. Plaintext `http://`
endpoints are refused. Use **Test connection** to confirm reachability, then
**Save**.

**Tor.** Under **Privacy**, **Connect through Tor** sends the wallet's traffic
(syncing and the payments it sends) through Tor, so the sync server, and anyone
watching your network, can't tie them to your IP address. Tor takes a few
seconds to start, and syncing is slower through it. While it's on, the wallet
never connects directly: if Tor can't start, the wallet waits and says so until
it does or you turn it off. Connecting to your group's coordination server
isn't affected, and name lookups (testnet) are off while Tor is on.

**Sync.** The active wallet syncs in the background from unlock to lock, and
Home shows whether it's up to date. If a sync fails, Home says so and offers
**Retry now**; if one stops making progress for a minute, it offers
**Restart**, which restarts the sync and refreshes every panel.

**Active wallet.** With more than one group, Cyze works on **one wallet at a
time**. Use the switcher at the top of the window to change wallets. Selecting
one makes it the active wallet, and the app stops syncing the previous one, so
all processing stays focused on your choice.

**Security.** **Settings → Security** changes your passphrase and saves a
backup file. It shows whether your backup covers every group.

**About.** **Settings → About** shows the version and lists the open-source
software Cyze is built on, with each license.

### Local devnet (regtest)

For Zcash development, Cyze can run against a private regtest chain on your own
machine, using
[thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura). Nothing on
a devnet is real money: addresses start with `uregtest1…` and amounts show as
**rZEC**.

1. **Install and start the devnet** (Linux or macOS, needs Docker):
   ```sh
   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh | sh
   ths start            # keep it running; Ctrl-C deletes the devnet
   ```
2. In **Settings → Wallet**, choose **Local devnet** and click **Detect local
   devnet**. Cyze asks the devnet's command-line tool (`ths`, or
   `thus-spoke-zakura` before 0.4) where lightwalletd is listening (it picks a
   random port), and checks that it answers. Then click **Save**. If Cyze
   can't find the tool, run `ths endpoints` and paste the `lightwalletd` URL
   (`http://127.0.0.1:<port>`) into the endpoint field. Plain `http://` is
   allowed only for local addresses. On Windows, run the devnet in WSL2 and
   enter the URL by hand.
3. **Network upgrade.** A devnet picks its own upgrades, and the wallet must
   match the node. **Auto** reads the upgrade from the node when you click
   Detect or Test connection. You can also force one, from NU6 to NU7.
4. **Fund the group.** Open the group's wallet, copy its `uregtest1…` address,
   and use the faucet on the devnet dashboard (the link appears after Detect).
   Mine a few blocks there so the funds confirm. Devnet wallets scan the whole
   chain, so funds sent before you opened the wallet still show up.

**Limitations:**
- **Sends need NU6.3 or NU7.** Cyze builds only Ironwood-era transactions.
  thus-spoke-zakura 0.4 starts new chains at NU7 and 0.3 at NU6.3; both work
  (**Detect local devnet** reads which). Older launchers (0.2.x) run NU6,
  where syncing, receiving and balances work but sending is refused, and the
  settings page says so.
- **Pick free ports for a second devnet.** `ths start --port-offset 700` shifts
  every port if another node already uses the defaults (18232, 9067).
- **Before Ironwood, the balance is the Orchard pool.** Until the devnet
  activates NU6.3, the balance shows the Orchard pool, because that's where
  devnet funds land.
- ZcashNames don't resolve on a devnet.
- **Resetting the devnet starts a brand-new chain.** A group's devnet wallet
  still holds the old chain, so after `ths reset --force` (or recreating
  it), delete the devnet wallet data: the `regtest` folders inside `wallets/`
  in Cyze's data directory (the folders above them have random-looking names).
  They are rebuilt on the next sync. This only affects devnet data. Testnet
  and mainnet wallets are stored separately.

**Diagnostics log.** With **Show advanced options** on, **Settings → Advanced** has a **Diagnostics log** card that captures
what the app logs while running (sync steps, errors). Use **Copy all** to grab it
for troubleshooting; it's in memory only and clears on restart.

