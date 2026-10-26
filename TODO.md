# Cyze — tracked follow-ups

## Planned for a future iteration

Larger pieces from external user testing. All are additive — nothing in the
current build depends on them.

- [ ] **ZcashNames (ZNS) integration** — human-readable names for FROST
      groups/wallets (e.g. `treasury.zcash` → the group's shielded receive
      address). Repo: https://github.com/zcashme/zcashnames

      How ZNS works: names are claimed/updated **on-chain via Ed25519-signed
      memos** (`ZNS:CLAIM:<name>:<ua>:<sig>[:<pubkey>]` in an Orchard note), and a
      **ZNS indexer** scans the chain and exposes a JSON-RPC API for resolution —
      so a wallet resolves by querying the indexer, not by scanning itself.

      - **Resolve (read) — DONE (this branch).** `name.zcash` in the Send recipient
        field resolves via the public indexer's JSON-RPC `resolve` method
        (`core/src/zns.rs` + `resolve_zns_name` command; endpoints
        `https://light.zcash.me/zns-{testnet,mainnet-test}`). The resolved address
        is shown for the user to confirm before sending, with a warning when the
        name is listed for sale. Remaining read follow-ups: accept names in
        **contact entries** too, and add a small resolver **cache**.

      - **Register/claim (write) — TODO, higher effort.** Let a group publish a
        name for its receive address. Threshold-authorised on-chain action that
        fits Cyze's FROST send + memo path: build the `ZNS:CLAIM:…` memo with an
        Ed25519 signature over the claim, sign the transaction with the group, and
        send it to the indexer's admin/registration address. Open questions: where
        the claim's Ed25519 key lives (per-group, in the keystore?), the exact
        pricing/fee tiers (indexer `status` method), and who may initiate a
        (re)claim. Note ownership is **sovereign** — once claimed with a key, all
        later actions on the name must be signed by that same key.

- [ ] **User avatars** — let a user pick an avatar, shown next to their name
      everywhere they are referenced (contacts, group participant lists, the
      signer picker, "Signed by" in transaction history, the inbox coordinator
      line).

      Design constraints to settle first, because these surfaces are dense and an
      avatar in each one will crowd them:
      - What *are* the avatars? Options, roughly in order of least effort:
        deterministic identicons derived from the comm pubkey (no asset pipeline,
        and doubles as a **visual key fingerprint** — a signer could spot a
        swapped pubkey at a glance); a fixed set of bundled illustrations; or
        user-uploaded images (needs storage in the keystore, size limits, and
        image decoding — most work, least security value).
      - Sizing: likely a 16–20px circle inline, larger only on the contact card.
        Participant lists and the signer picker are the tightest spots; check
        those before committing to a size.
      - Avatars are **cosmetic and self-asserted** — they must never be the thing
        a user relies on to identify a signer. The comm pubkey remains the
        identity. An identicon derived *from* the pubkey is the one variant that
        strengthens rather than weakens this, which argues for it.

- [x] **Tailscale `serve` as a fourth hosting option** — DONE
      (`feat/tailscale-serve`). A fourth `coordinator_exposure` variant
      (`"tailscale"`) in Session Configuration, alongside Direct URL, Cloudflare
      Tunnel, and NGINX.

      Implementation: `src-tauri/src/tailscale.rs` **detects and drives a system
      `tailscale` CLI** (not bundled — needs the privileged `tailscaled` daemon +
      a logged-in tailnet). `tailscale serve --bg --https=443
      https+insecure://127.0.0.1:<port>` puts the loopback frostd behind the
      machine's stable MagicDNS name on the tailnet, with auto-provisioned public
      TLS (so participants connect with system roots — no cert-trust step). We use
      `serve` (tailnet-only), never `funnel` (public). The MagicDNS name is read
      from `tailscale status --json` (`Self.DNSName`), and `available`/`detail`
      surface *why* it's not ready (not installed / signed out / offline).
      Commands `start_tailscale_serve` / `stop_tailscale_serve` /
      `tailscale_status`; `AppState.tailscale` handle; serve is torn down when the
      sidecar stops and on app exit (`stop_serve_blocking`, since the mapping
      lives in the daemon). UI: a Tailscale tab + `TailscaleExposure` in
      `SessionSetup.tsx`; the stable `.ts.net` URL is (correctly) *not* treated as
      ephemeral, so it saves + reuses like a normal server. `https+insecure` is the
      tailnet equivalent of cloudflared's `--no-tls-verify`; frostd's Noise layer
      still authenticates end-to-end. Compile-verified + unit tests; **needs a
      live run** on a machine with Tailscale installed/signed-in to confirm the
      `serve` invocation against the current CLI.

      Possible follow-ups: bundle/guide an install path if detection proves too
      bare; surface the "all participants must be on the tailnet" constraint even
      more prominently; optional `funnel` toggle for a participant outside the
      tailnet (explicitly public — would need the same loud warning as the
      Cloudflare tunnel).

## Coordination (Safe-style features)

Zcash has no contracts for shielded funds, so these are rules each member's app
applies when it decides whether to sign: they hold unless a threshold of members
act together with modified apps, the trust the wallet already rests on. New
kinds of group record ride on `core/src/polls.rs`, whose older versions keep
and skip kinds they don't know.

- [x] **Shared payees** — `core/src/payees.rs`, `commands/payees.rs`,
      `src/wallet/Payees.tsx`. Author-owned; approvers name a payment from their
      own copy, by the verified address (`verify::address_forms`).
- [x] **Payment reasons** — `SigningContext.reason` for approvers, then a
      `PaymentReason` record keyed by txid once sent; any member can add theirs.
- [ ] **Accounting export** — CSV of history with payees, reasons and signers.
- [ ] **Read-only access** — export the group's viewing key for an accountant.
- [x] **Batch payments** — `wallet::prepare_payments` (single sends are a batch
      of one), `SigningContext.payments` matched as an unordered set by
      `check_coordinator_claims`, `src/wallet/Batch.tsx`. Up to
      `MAX_PAYMENTS` (20); the request is sized exactly against one frostd
      message (`signing::signer_context_budget`) and refused at review when it
      can't fit.
- [ ] **Pay if agreed** — a poll that carries a payment; once agreed, approvers
      see the poll and a check that the transaction matches it exactly.
- [ ] **Trusted payees only** — adding a payee needs a threshold of members to
      accept; apps warn about or refuse payments to anyone else.
- [ ] **Spending limits** — approve automatically up to X per week to trusted
      payees (the approver's app must be open).
- [ ] **Delay with veto** — large payments announced first and signable only
      after a delay; any member can veto meanwhile.
- [ ] **Tiered approvals** — e.g. over 100 ZEC needs 4 of 5 on a 3-of-5 key.
- [ ] **Change members or threshold** — a new group, then move the funds
      (reusing the viewing-key move, `commands/viewing.rs`).
- [ ] **Share repair** — a member who lost their device gets a new share from
      the others (FROST repairable shares), without moving funds.
- [ ] **Recurring payments** — scheduled payments that still need approval.

## Voting (protocol / coinholder governance)

- [ ] **Migrate to ValarGroup Shielded Vote (full rebuild; supersedes memo v1)** —
      the real coinholder-vote protocol we should target is **ValarGroup Shielded
      Vote**: https://valargroup.gitbook.io/shielded-vote-docs . It is a live,
      cryptographically-private, on-chain voting system on a **dedicated vote
      chain**, used infrequently to gauge protocol-upgrade sentiment *before*
      committing engineering resources — exactly Cyze's governance use case.

      **This is not a change to the current memo format — it is a different
      system.** Our shipped `core/src/voting.rs` implements the informal
      **zec-coin-polling "Vote Cast Memo v1"** (a JSON memo cast as a shielded send
      to a reception address, tallied off-chain from a *transparent*-balance
      snapshot). Shielded Vote has no vote memo: a vote is a ZK-proven **Vote
      Commitment** (VAN consumed → new VAN + `H(DOMAIN_VC, round_id, shares_hash,
      proposal_id, vote_decision)`) plus 16 ElGamal share ciphertexts, submitted to
      REST endpoints on the vote chain. **Expect to scrap `voting.rs` and its UI**
      (`VoteTab`/`parseBallot` in `src/screens/Groups.tsx`, `wallet_prepare_vote`,
      `VoteEntry`/`BallotDefinition`) and rebuild around the SDK below. Keep memo v1
      only if a lightweight, no-infra sentiment poll is still wanted; otherwise
      remove it so the two are never confused.

      **Confirmed (2026-08-04, from the user):**
      1. **Infrastructure is live** — vote chain + election authority + PIR fleet
         are running; there is a real network to build/test against.
      2. **FROST-compatible** — the delegation step (ZKP1) takes an externally
         produced re-randomized spend-auth signature via a governance PCZT
         (`(rk, sighash, spend_auth_sig)`), which maps onto Cyze's existing FROST
         re-randomized Orchard signing ceremony. Confirm the exact PCZT hand-off
         when building.
      3. **Ironwood supported going forward** — vote weight snapshots the group's
         shielded note holdings, and Ironwood is covered, so a post-NU6.3 shielded
         treasury can vote (this also fixes memo v1's flaw that only *transparent*
         balances counted — a shielded FROST treasury effectively couldn't vote).
      4. **Crate versions: pin to whatever is in production at build time.** Both
         SDK crates are published and moving fast — snapshot the then-current
         production versions rather than an early rc:
         - `zcash_voting` — client lib (ZKP1/2/3 via Halo2, ElGamal, governance
           PCZT, Merkle witnesses, SQLite round state). Repo:
           https://github.com/valargroup/zcash_voting
         - `pir-client` — nullifier non-membership PIR queries.
         (Swift SDK exists too, but Cyze is Rust — use the crates directly.)

      **Rough shape of the wallet-side flow** (see the Integration Guide): discover
      + validate vote config → `GET /shielded-vote/v1/rounds/active` → PIR
      nullifier proofs → build+prove **ZKP1 delegation** (governance PCZT, FROST
      spend-auth) → `POST /delegate-vote` → sync the vote-commitment tree → per
      proposal, build **ZKP2** and `POST /cast-vote` → split into 16 ElGamal shares
      and `POST /shares` with staggered anti-censorship `submit_at` timing → read
      `GET /tally-results/{round_id}` once the round is `FINALIZED`. Note
      `vote_round_id` encoding is context-sensitive (hex in config/URLs/shares,
      base64 in delegate/cast bodies).

      **Suggested first step — a scoping spike** before any UI: add the production
      `zcash_voting` + `pir-client` crates, hit a live round's `/rounds/active`, and
      prove the FROST-produced re-randomized spend-auth sig feeds ZKP1 end-to-end.
      That de-risks the one genuinely novel part (threshold signing into their
      prover) cheaply. Own branch, own PR; larger effort than any current item.

      **Built (2026-10-25):** the Votes screen reads rounds and results from the
      live configs (`core/src/community_vote.rs`; production for mainnet,
      staging for testnet) and "Decide together" turns proposals into group
      polls. **Still to build: casting** (delegation via the Keystone path with a
      FROST signature, co-signer verification of the governance PCZT, the
      voting hotkey in the keystore, ZKP2 casting and share tracking), to be
      tested against a staging round on testnet with a group holding notes at
      its snapshot. Staging rounds open irregularly (the last was created
      2026-10-16); `GET /shielded-vote/v1/rounds` lists them with their status
      (1 active, 2 tallying, 3 finalized, 4 pending, 5 failed). Tallies are in
      ballots of 0.125 ZEC.

      **Spike findings (2026-10-24):**
      - **The crates fit.** `zcash_voting =5.2.0-rc.2` and `pir-client =0.9.0`
        build in Cyze's tree. They're on the same Zakura 4.0 family
        (`zakura-wallet-lib ^0.1.0-rc9.2` resolves to our rc9.3), with one copy
        of the wallet libraries. 5.1.x is on Zakura 2.x and doesn't fit.
      - **The FROST hand-off is the Keystone path.**
        `DelegationPipeline::keystone_request(bundle)` returns a
        `KeystoneSigningRequest` with the redacted governance PCZT, its ZIP-244
        sighash, `rk` and the action index (α is in the PCZT). The group signs
        with the existing re-randomized ceremony, and the signature goes back
        as `KeystoneSignatureSource::Provided { sig, sighash }`. The SDK
        verifies and stores it, so a restart doesn't need another signature.
        One ceremony per bundle. TX1 is never broadcast.
      - **Encrypted wallets work:** `WalletDbOpener` is a trait, so Cyze
        supplies one that opens its SQLCipher database with the key.
      - **Co-signers need a new check:** "this signature delegates the group's
        voting weight (N ZEC) for round R", recomputing the sighash from the
        redacted PCZT and checking `rk`. The payment verifier would refuse it
        or describe it wrongly.
      - **After delegation, votes are signed by a voting hotkey**, not the group
        key, so casting is one device's job; the hotkey's secret belongs in the
        keystore's `[cyze]` table.
      - **Infrastructure:** the docs' `valargroup.github.io` config URLs are
        stale (404). The configs live at
        `https://voting.valargroup.org/{prod,stage}/dynamic-voting-config.json`:
        prod lists three vote servers and three PIR endpoints, stage has its
        own. No round was active on prod or stage when checked
        (`/shielded-vote/v1/rounds/active` → `round: null`), so an end-to-end
        test waits for one, with a group holding notes unspent at its snapshot.

## Wallet (Zcash)

- [ ] **Record our own sends in the wallet** — Cyze builds sends as PCZTs and
      broadcasts them itself, so the wallet db learns of them only by scanning:
      as a balance change, without the fee or outputs (`sent_notes` stays
      empty; compact blocks can't be decrypted with the OVK). History then
      shows the fee inside the amount and no recipient. The sending device
      fills these in from its saved send (`withWhatWasSent` in Activity.tsx).
      Fix: `extract_and_store_transaction_from_pczt` at broadcast (the sender),
      and for other members either enhancement (fetching full transactions,
      which tells the server which are the group's) or the details in a group
      record.

- [x] **Auto-sync polish** — the backend sync service keeps the active wallet
      current, so Home no longer shows a permanent **Sync Now**: it offers
      **Retry now** after a failed sync and **Restart** after a minute without
      progress (`useSyncStalled` in `src/wallet/Wallet.tsx`).
- [x] **Send history (client-side)** — the "Transaction history" panel
      (`SendHistory` in `src/screens/Groups.tsx`) now lists this device's past
      send ceremonies from the persisted store (time, amount, recipient, status
      + txid). Survives reload.
- [x] **On-chain transaction history** — `wallet_history` command queries
      `orchard_received_notes` (is_change=0) and `sent_notes` directly from the
      wallet sqlite, returning `TxRecord[]` (txid, direction, amount, fee, memo,
      recipient). Rendered in the expandable `GroupHistory` table on the wallet
      page. Refreshes after each sync cycle. Capped at 50 rows displayed.
- [x] **Message history** — memos decoded from `orchard_received_notes.memo`
      and `sent_notes.memo` (0xF6 empty-sentinel + null-padding stripped).
      Rendered in the "Message history" panel below the tx table, filtered to
      entries with non-null memos. Capped at 30 rows.

## Send path (Phase 5.2, in progress)

- [x] 5.2a — build draft Orchard tx (PCZT) + sighash (no funds moved).
- [x] 5.2b — FROST-sign the sighash with `randomizer = α` and
      `apply_orchard_signature` into the PCZT (drive the existing ceremony).
- [x] 5.2c — Orchard proof → `SpendFinalizer` → `TransactionExtractor` →
      lightwalletd `send_transaction` (`wallet::broadcast_signed`). Emits the
      `proving` phase + final txid. **Needs a live testnet end-to-end run** to
      confirm the proof/finalize/extract/broadcast leg (compile-verified only).
- [x] **Multi-spend sends** — `wallet_send` now runs one re-randomized ceremony
      per Orchard spend (each over the shared sighash with that spend's α),
      sequentially, then applies all signatures before proving + broadcasting.
      The UI shows "Signing input i of N". NB: signers approve N times (one
      inbox session per input). Sequential keeps it within the expiry window for
      small N; parallelizing the ceremonies is a future optimization if needed.
- [x] **Signer selection** — the send form now has a threshold-subset signer
      picker (`GroupWallet` in `src/screens/Groups.tsx`), pre-seeded with this
      device's member; the Sign button is gated on `>= threshold`. The chosen
      pubkeys flow through `wallet_send` unchanged (backend already accepted an
      arbitrary signer list).
- [x] **Slow approvals** — decided (2026-10-25) to keep the standard expiry
      (~50 minutes, both before and after NU7) rather than lengthen it. Zakura's
      `create_pczt_from_proposal` now takes an expiry override, but the expiry
      height is public and a longer one would single out Cyze's payments
      on-chain. Instead: `wallet::ApprovalWindow` gives the deadline to the
      sender (Review, then "needed by about 3:42 PM") and to approvers (from
      their own sync server's tip); the send stops when the chain gets within
      `EXPIRING_SOON_BLOCKS` of the expiry, not on a fixed timer; an expired
      send offers **Ask again** (built afresh, same members), and a signed one
      that failed to broadcast offers **Try sending again** (`wallet_rebroadcast`).
      Sends cut off by a lock or restart are marked as such on unlock.
