# Permissions — where they are, where they leak, and what to build

The question this started from: **could all script and user management be done
through the agent interface, with the agent calling the engine's own MCP API on
`manage.softagen.com`?** An editor or administrator would ask for what they
want in prose, and see the current state — the list of users, the list of
scripts — in the same place.

The answer is yes, and most of the mechanism already exists. What is missing is
not wiring. It is that **the engine has no way to express a credential holding
less than its holder's roles**, which is the thing that makes an agent with
management tools either useless or unbounded.

This file is the plan. The one design worked up in full lives in
[`docs/SESSION_ELEVATION.md`](docs/SESSION_ELEVATION.md).

---

## 1. What works today, with no engine change at all

- `McpClient(serverUrl, secretName)` is in the JavaScript API
  (`src/security/secure_globals.rs:2647`), gated on `UseNetwork` **and**
  `ReadSecrets`. A delegated turn holds both, deliberately
  (`src/delegation.rs:173`).
- Secret resolution is `user_secrets` first, then script secrets, so each
  editor stores **their own** token exactly the way the agent already has them
  store their Anthropic key — and `secretStorage` never hands it to JavaScript.
- `/mcp` exposes 52 native tools (`src/engine_api.rs:9319`), each authorized
  per call against the calling user: capability, ownership
  (`repository::user_owns_script`), and `AdministerEngine` for anything they do
  not own.

So the mechanical path is: the agent page runs the OAuth2 code flow against the
engine (registration is open, PKCE, `resource=https://manage.softagen.com/mcp`),
stores the access token as a per-person secret, and mounts `/mcp`. Reads and
writes both work. Roughly a day of work in `main.ts`.

**Do not just do that.** The next section is why.

## 2. The three reasons not to, in order of severity

### 2.1 It launders the delegation cap

`delegation::context_for` (`src/delegation.rs:215`) caps background work at
`UserContext::authenticated`, and that cap is enforced on the **in-process
`UserContext`**. It says nothing about an outbound `Authorization` header.

An editor's token sitting in `user_secrets` makes a delegated turn an editor
while the person is asleep, and `context_for` goes on refusing `WriteScripts`
to a context that no longer needs it. The gate that protects the _key_ — no
read from JavaScript — does nothing for _authority_: the model never needs to
see the token, it only needs to name the tool.

This is written up from the agent's side in
`../aiwebengine-agent/TODO-improvements.md`, section "The engine is already an
MCP server". It is a design decision to make rather than drift into.

### 2.2 There is no smaller credential to give it

A session carries roles, not scopes (`SessionData.is_admin` / `is_editor`,
`src/security/session.rs:168`). `oauth_client_grants.scope` is recorded at
consent (`src/auth/routes.rs:4198`) and **gates nothing** — it is read only to
compare a new request against a stored grant.

So the minimum credential the engine can mint is "everything that person can do
on that host": write any script they own **and** `list_users`, `add_user_role`,
`write_secret`, `eval_script`. "My scripts, not the user table" is not
expressible.

### 2.3 The refresh token has nowhere safe to live

`{{secret:...}}` substitutes into header values and, since `resolve_url`, into
URL paths (`src/http_client.rs:1244`, `:1403`) — **not into bodies**. OAuth
refresh posts `refresh_token` in a form body, and the engine's refresh tokens
are single-use and rotating (`src/auth/refresh_tokens.rs`).

So the agent would have to keep the rotating credential in `personalStorage` in
the clear, readable by the script and by anything holding `read_storage`. That
is a straight regression against the property the Anthropic key currently has.

### 2.4 And one practical blocker

`HttpClient::is_private_ip` (`src/http_client.rs:627`) refuses loopback and
private addresses, so **an engine cannot call its own `/mcp` in local
development**. Any design built on the outbound-HTTP path can only be developed
against production.

---

## 3. The plan

### Phase 1 — an in-process engine API, authorized against the caller

**The single change that removes 2.1, 2.3 and 2.4 at once.**

CLAUDE.md says engine administration is deliberately not exposed to JavaScript.
Read the reason: _all scripts are equal_, so exposing it would have meant every
script seeing it. That argument has since been answered by the capability model
itself.

A global — `engine.call(tool, args)` — dispatching to the same `native_tools()`
functions, authorized against the **calling user's `UserContext`**, identically
to how `/mcp` authorizes them. A script "having" it holds nothing its caller
does not. That is consistent with "all scripts are equal", and it means:

- no credential to store, no rotation, no laundering;
- the delegation cap actually binds, because the call is in-process;
- it works on localhost.

Work:

- `engine.call(name, argsJson)` in `secure_globals`, dispatching through the
  existing `native_tools()` table so there is one implementation of each tool.
- A refusal that carries structure, not a string (see Phase 2's
  `AppError::ElevationRequired` — the same variant serves both).
- `engine.tools(area?)` for discovery, so a script never embeds a tool list
  that drifts.
- Deliberately **not** reachable from `sandbox.run`: model-authored code must
  never see it, or the argument in `../aiwebengine-agent/agent/capabilities.ts`
  collapses.

### Phase 2 — session elevation (step-up)

Full design: [`docs/SESSION_ELEVATION.md`](docs/SESSION_ELEVATION.md).

In one paragraph: a session starts at the `authenticated` floor whatever roles
the account holds, and `author` / `administer` are granted for minutes at a
time by a deliberate act on a session that has just re-authenticated. It needs
no migration (the session is an encrypted JSONB blob) and no revocation
statement (`delete_sessions_for_user` already takes elevations with the
sessions). `oauth_client_grants.scope` finally gates something: a token's
elevation.

This is what makes an agent holding management tools a **bounded** thing. The
person consents to `author`, the token carries `author`, and no arrangement of
that token reaches `administer`.

Sub-steps, in order:

1. `UserContext::for_session` — collapse the four duplicated tier matches into
   one place, so elevation has somewhere to land. **← done**
2. A capability refusal that keeps its capabilities. **← done**, as
   `AppError::InsufficientCapabilities` rather than as `ElevationRequired`:
   naming it for the elevation would have been a promise the engine cannot yet
   keep, and a refusal that says "go and elevate" with nowhere to go is worse
   than one that says what is missing. The variant carries the list, renders
   403 with `required_capabilities` in the response's `context` map, and is
   where the `elevate` hint is added when there is one. `SecurityError` now
   names capabilities by `Capability::as_str` as well — `{:?}` printed
   `WriteScripts`, which is the Rust variant and not a name any surface of the
   engine accepts.

   Still string-shaped, and each needs its own decision rather than a sweep:
   `FileReadError::AccessDenied`, `TestRunRefusal::AccessDenied` and
   `CheckRefusal::AccessDenied` each flatten a capability refusal into a
   bespoke variant, and `authorize_script_write` returns `Err(String)`. Sharper
   than any of those: `engine_api.rs:742`, `:805` and `:1070` answer a refusal
   with `None` or an empty `Vec`, so "you may not see this" and "there is
   nothing here" are the same answer — which is right for an enumeration that
   must not leak what exists, and wrong for a caller who could have elevated.

3. `SessionData.elevation`, checked in `validate_session` **and**
   `refresh_session` (the two places `realm` is checked, for the same reason).
   **← done.** `security::elevation` holds the vocabulary and the composition;
   `UserContext::for_session` applies it; `[security.elevation] gated` is the
   one dial and is empty by default, which is the engine as it behaved before.
   `SecureSessionManager::set_elevation` is the write the endpoints below are
   both made of.

   `reauthenticated_at` is **not** done and belongs with the endpoints: it is
   only meaningful next to something that checks it, and a field nothing reads
   is a claim the engine does not keep.

4. `GET|POST /auth/elevate`, `POST /auth/elevate/drop`, and the third panel on
   `/auth/account` — a copy of `delegate_page`. **← done**, with
   `reauthenticated_at` set at sign-in and by `mark_reauthenticated`, and the
   POST refusing outside `reauth_window_secs`.

   A local account proves presence with its password, throttled per address
   _and_ per account exactly as `login_local` is, since a guess here is worth
   more than a sign-in rather than less. A federated one can only elevate
   inside the window after signing in — see step 5, which is what makes that
   smooth.

5. `prompt=login` threaded into `get_authorization_url` (a fifth argument;
   `extra_params` is per-provider config and this is per-request). **Next, and
   it is what makes elevation usable on a federated deployment.** The page
   deliberately offers no "re-authenticate" button without it: a round trip
   the provider answers from its own cookie proves nothing, and an engine
   calling that re-authentication would be claiming something it cannot back.

   The refusal hint belongs here too — `AppError::InsufficientCapabilities`
   gaining the `elevate` URL that `routes::elevate_url` already builds, so a
   refused caller is sent to the page rather than left to find it.

6. `scope=` honoured at `/auth/oauth2/authorize`, re-read on refresh.
7. `[security.elevation]` with `enabled = false` as the code default, and a
   `gated` list so `administer` can move before `author`.

### Phase 3 — a delegation verb for management

`Scope::Write` was the verb `delegation.rs` grew for storage and tables.
Management needs its own — something like `manage_own_scripts`, capped to
scripts the person owns, never reaching `list_users` / `write_secret` /
`eval_script`.

Until it exists, **writes belong where `write_skill` already puts them**: in
the approving request, under the person's own rights. Plan mode is already the
seam (`plans.ts`, `POST /agent/decide`), so this costs almost nothing and is
honest about what it is. The consequence is documented and correct: a
Telegram-button approval is refused, because a delegated turn can never hold
editor authority.

### Phase 4 — the status half

Reads are cheap; presentation is not.

- **Tool-list cost.** 52 tool schemas is roughly 10–15k tokens resent every
  turn. Do not hand the model the list: two meta-tools (`engine_tools(area)`,
  `engine_call(name, args)`) keep the prompt flat and let the list be filtered
  by the caller's role, so an editor is never shown `add_user_role`.
- **Rendering.** `agent/app.js` writes plain text. Two hundred users as prose
  is worse than the table in `aiwebengine-dev/admin/main.js`. Either
  `write_response` grows a structured `blocks` argument, or the agent answers
  with deep links into the existing dev UI and the UIs stay the _view_ while
  the agent is the _verb_. The second is much less work and probably right
  first.
- **Turn budget.** `NATIVE_TOOL_CEILING_MS` is 30 s (`engine_api.rs:10275`)
  against a 60 s default job budget. `run_tests`, `pull_from_git` and
  `check_script` can eat a whole turn, and `maxAttempts: 1` means the run just
  stops. Raise `jobTimeoutMs` first; treat long tools as enqueue-and-report.

---

## 4. Gaps in the MCP surface itself

Found while comparing `native_tools()` against the `/engine/*` routes:

- `/engine/health/cluster` has no MCP tool.
- Neither stream has one (`/engine/script_updates`,
  `/engine/script_logs/stream`), so "watch the log while it deploys" is not
  expressible over MCP.
- Delegation grants are listed nowhere but `/auth/account`.
- There is no user creation or deletion over HTTP either — accounts appear by
  signing in. Fine, but worth knowing before promising "all user management".

---

## 5. Cross-cutting, and small

- **Audit has no actor dimension.** `audit.rs` records the _person_, so a
  change made by an agent acting as them is indistinguishable from one they
  made by hand. `UserContext.attenuated` (`capabilities.rs:196`) already exists
  to explain _why_ a context that belongs to an administrator is refusing
  something an administrator may do — carry it, and the elevation method, into
  the audit line.
- **No rate-limit key for native tool calls.** `GitSync` and `ChannelTrigger`
  are bounded; an agent loop calling `write_file` is not.
- **The agent can rewrite the agent.** The seatbelt already exists: pin it with
  `deploy_script`, so a self-edit records a revision and changes nothing until
  a human unpins. Make it a documented precondition rather than a denylist.

---

## 6. Moving security out of scripts and into the engine

The pattern behind `personalStorage` is sharper than "the engine provides it":
**the engine owns the key, so the script cannot get it wrong.**
`personalStorage` is keyed `(script_uri, user_id, key)` by the engine, so a
script does not _fail_ to read another person's data — it cannot name it. The
best instance in the codebase is `personalTasks.enqueueFrom`, which takes a
_sender_ rather than a user id: the worst a buggy webhook can do is claim the
wrong sender, because naming an account is not expressible.

The test that follows: **can a script get this wrong in a way that harms
someone other than its own author's solution?** If yes, the engine should own
it.

Already done, and worth listing so the pattern is visible: `secretStorage`
(write-only from JS), `{{secret:}}` resolved host-side, the channel-link
invitation flow, JSX `h()` escaping by default with an explicit safe-HTML
marker, `client_ip.rs` rewriting the forwarding headers so scripts read an
already-judged address.

Not done:

1. ~~**No crypto whatsoever.**~~ **Done** — see
   [`docs/SCRIPT_CRYPTO.md`](docs/SCRIPT_CRYPTO.md). `crypto.hmacVerify` and
   `crypto.secretEquals` resolve the key host-side the way `fetch` resolves
   `{{secret:}}`, so the secret is never in JavaScript and the comparison is
   constant-time; `crypto.randomToken` mints the webhook secret that was being
   typed by hand. Both halves of the split matter: `randomUUID`,
   `randomToken` and `constantTimeEqual` take no capability, since randomness
   is not authority, while the two that resolve a secret take `read_secrets` —
   which is what stops `run_js` turning a comparison into an oracle.

   Deliberately absent, each for its own reason: signing (verification answers
   a question, signing produces a credential, and `{{secret:}}` already serves
   the outbound cases), prefix stripping (only the sender's documentation says
   what `sha256=` or `v0=` is), and a general hashing API (nothing has needed
   one).

2. **No rate-limit primitive for scripts.** `rate_limiting.rs` protects engine
   endpoints; a script with a public form writes its own, or does not.
   `limits.consume(key, budget)` with the engine choosing the bucket (per
   person, per judged IP) is the same move as `personalStorage`.
3. **No CSRF for script routes.** `csrf.rs` protects engine forms. The
   headers/CORS argument — the engine speaks only for pages it wrote — is about
   _policy_; handing a script a correct token and validating it on an opted-in
   route is not speaking for its page.
4. **Account deletion is only as complete as where personal data lives.**
   `delete_user` takes sessions, grants and git credentials; a person's rows in
   a script's own tables stay. An argument for making `personalStorage` and a
   per-person table namespace the _attractive_ place to keep personal data —
   it turns erasure from a per-solution promise into an engine guarantee.
5. **Consent as a primitive.** `/auth/delegate` is a page the engine renders
   and a record it keeps. Generalise it and any script needing approval for
   anything gets an auditable, uniform record instead of a boolean in its own
   table. Design it together with the elevation page — they are the same page
   asked at two lifetimes.
6. **An audit log scripts can append to but not clear.** Scripts have
   `console.log`, and `clear_logs` exists — a log the actor can delete is not
   an audit trail.

---

## 7. Suggested order

1. `UserContext::for_session` (**done** — the four sites now collapse into one,
   which is where elevation lands).
2. A capability refusal that keeps its capabilities (**done**). Useful on its
   own, and the hook the elevation challenge hangs from.
3. `crypto.hmacVerify` and friends (**done**). Small, independent, and stops
   every solution getting webhook verification wrong.
4. Session elevation proper (`docs/SESSION_ELEVATION.md`), `administer` gated
   first.
5. `engine.call` in-process, once refusals carry structure.
6. The agent's two meta-tools and the rendering decision.
