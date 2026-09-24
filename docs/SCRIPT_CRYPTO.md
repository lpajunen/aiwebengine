# Verifying a Webhook Without Holding the Key

```ts
// Slack signs the body. The signing secret never enters JavaScript.
const base = `v0:${timestamp}:${rawBody}`;
if (
  !crypto.hmacVerify({
    secretName: "SLACK_SIGNING_SECRET",
    message: base,
    signature: signatureHeader.replace("v0=", ""),
  })
) {
  return { status: 401, body: "no" };
}
```

The engine asks scripts to verify their own webhook signatures, and it is right
to: Telegram echoes a shared secret in a header, Slack and GitHub sign the
body, and only the sender's documentation says which. What was wrong is that it
asked and then handed them nothing to do it with. There was no HMAC in the
JavaScript API, no constant-time comparison, and no source of randomness — so
every solution that followed the documentation wrote the same three mistakes.

## The three mistakes, and what replaces each

**The secret was fetched into JavaScript.** `secretStorage` has no read, on
purpose — `exists()` only says whether a key is there — so a script that needed
to _compare_ a secret had nowhere to keep it but `scriptStorage`, in the clear,
readable by anything holding `read_storage`. The agent's own Telegram webhook
says so in its setup instructions: the shared secret lives in `scriptStorage`
"because checking it means _comparing_ it, and the engine never returns a
secret's value to JavaScript".

Now the secret is **named, not passed**. `secretEquals` and `hmacVerify` take
the name of a secret and resolve it host-side, exactly as `fetch` resolves
`{{secret:NAME}}` in a header — `user_secrets` first, then the script's own. A
script cannot leak a value it is never given, and the webhook secret goes back
to being a secret.

The option is `secretName` rather than `secret` for that reason. A field called
`secret` invites somebody to pass one, which would work, and would silently
give up the property they came here for.

**The comparison was `===`.** Which leaks the secret a byte at a time to
anyone who can time the endpoint, and looks like working code forever.
`constantTimeEqual` is the primitive; `secretEquals` is it applied to a value
the script never sees. Length is not secret — it is visible in the encoding of
anything that carries one — so a length mismatch answers immediately; what
stays constant is the time taken over two strings of equal length.

**The webhook secret was typed by a person.** `randomToken` mints one. It is
refused below 16 bytes and above 64 rather than clamped, because a caller who
asked for 8 and silently got 16 would go on believing it had asked for
something it did not get, and the thing it did not get is the entropy.
(`script_limits.rs` clamps, and is right to: a stored limit takes effect
without a restart and a mistyped one would hold a slot until somebody noticed.
Here the mistake is invisible instead, which is the case for refusing.)

## The surface

| Call                                    | What it answers                          | Capability     |
| --------------------------------------- | ---------------------------------------- | -------------- |
| `crypto.randomUUID()`                   | a v4 UUID, exactly as the web platform's | none           |
| `crypto.randomToken(bytes?, encoding?)` | an unguessable token, 16–64 bytes        | none           |
| `crypto.constantTimeEqual(a, b)`        | equal, without saying where they differ  | none           |
| `crypto.secretEquals(name, candidate)`  | does this match the stored secret        | `read_secrets` |
| `crypto.hmacVerify({...})`              | did this key sign these bytes            | `read_secrets` |

The split is the point. Randomness is not authority, and comparing two strings
the caller already holds reveals nothing it did not have — so model-authored
code inside `sandbox.run` may use the first three, and should, because the
alternative is that it writes the comparison itself. The two that resolve a
secret take the gate `fetch` puts on `{{secret:...}}`, and for the same reason:
a narrowed execution that may not reach the account's credentials must not
reach them through a comparison either. `run_js` withholding `read_secrets` is
what stops this becoming an oracle.

## What a refusal means

**A missing secret throws.** Answering `false` would make an unfinished
deployment look exactly like an endpoint under permanent attack — every
delivery refused, the log agreeing, and nothing anywhere naming the key nobody
stored. This is the `{{secret:...}}` rule in the place it matters most: an
unresolvable secret is an error, because behaving as though it resolved is
worse than stopping.

**An unknown algorithm or encoding throws**, naming both what was asked for and
what there is. A verifier silently defaulted from `md5` to `sha256` answers
`false` for every delivery, which reads as an attack rather than as a typo.

**Everything else is `false`.** A signature over different bytes, one of the
wrong length, one that is not valid hex at all — from the script's side these
are one event, _something arrived that this key did not sign_, and
distinguishing them in the return value would put a decoding oracle where a
yes-or-no belongs.

## Algorithms

`sha256` by default, `sha512`, and `sha1`. SHA-1 is here because webhooks still
send it — GitHub's original `X-Hub-Signature` is HMAC-SHA1 — and a verifier
that cannot speak it simply cannot check those deliveries. It is not a choice
to make for something new.

The implementation is pinned against RFC 4231 and RFC 2202 rather than against
its own output, because an HMAC that agrees with itself verifies everything and
protects nothing.

## What is deliberately absent

**Signing.** Verification answers a question about something that arrived;
signing produces a credential. The outbound cases the engine has — a bearer
token, a key in a path — are already served by `{{secret:...}}` without the
script holding anything at all. When an API that wants a signed request turns
up, that is the moment to design it, and the shape it wants will be clearer
then than it is now.

**Stripping the scheme prefix.** GitHub sends `sha256=…` and Slack sends
`v0=…`; the caller strips it. Only the sender's documentation says what the
prefix is, so an engine that guessed would be wrong for the next sender.

**A general hashing API.** `crypto.hash(...)` would be the obvious neighbour
and is not here, because nothing has needed it: digests inside the engine are
computed by Postgres (`asset_blobs`) or by `git_sync` for blob ids, and neither
is a script's business.

## The rule this follows

The engine owns the key, so the script cannot get it wrong. `personalStorage`
is keyed `(script_uri, user_id, key)` by the engine, so a script does not
_fail_ to read another person's data — it cannot name it.
`personalTasks.enqueueFrom` takes a sender rather than an account, so a buggy
webhook can claim the wrong sender and not a different account. This is the
same move: the comparison a solution would have written is one the engine
writes once.

The test for whether something belongs on that list: **can a script get this
wrong in a way that harms someone other than its own author's solution?** A
leaked webhook secret does. A timing side channel does. A guessable token does.

## What the engine still cannot do

Verify that a message came from the sender it names. Telegram's shared secret
proves the _call_ came from Telegram; nothing proves the body was not edited by
whoever holds that secret. Checking the signature is the script's job because
only the script knows the scheme — this gives it the tools, not the judgement.
See `docs/DELEGATION.md` for why that is bounded anyway: a forged delivery
still has to name a sender somebody has linked.
