# Asking the Person a Question, Mid-Tool

```javascript
// Registered as an MCP tool. Called by whatever agent is driving /mcp.
function openIssue(context) {
  const { repo } = context.request.arguments;

  // First call: nobody has answered, so this does not return. The engine
  // answers the client with `input_required` and the tool call ends there.
  // The client asks the person, then calls the tool again — and this time
  // `ask` returns what they said.
  const answer = mcp.ask({
    message: `Title for the new issue in ${repo}?`,
    schema: {
      type: "object",
      properties: { title: { type: "string" } },
      required: ["title"],
    },
  });

  if (answer.action !== "accept") {
    return { content: [{ type: "text", text: "Nothing opened." }] };
  }
  return createIssue(repo, answer.content.title);
}
```

A tool could not do this. Every MCP tool had to answer from its arguments
alone, so anything needing a decision from the person had to be split into two
tools with the agent taught to call them in order — which makes the model
responsible for a protocol, and models forget protocols.

## Why this is re-execution and not suspension

The obvious implementation is to stop the handler where it asks and resume it
when the answer arrives. That is not what happens, and not what the
specification wants.

Multi Round-Trip Requests are _stateless by construction_. The retry is a new
request with a new JSON-RPC id, and the specification is explicit that the
server processing it "does not need any information beyond what is directly
present in the retry request" — the pattern exists so that a server needs no
shared storage layer and no sticky load balancing. Whatever the server needs
carried across the gap, it puts in `requestState`, which the client echoes back
without being able to read it.

So the handler **runs again from the top**. `mcp.ask` is not a pause; it is a
lookup that either finds an answer already given or ends the execution. The
second time through, the code before the `ask` runs again and the `ask` itself
returns immediately.

This is the right shape for this engine rather than merely an acceptable one. A
host call blocks the script and there is no event loop to yield to, so
suspending a handler mid-call would have meant an interpreter change. Re-running
one needs nothing the engine does not already do. And because each round trip is
an ordinary tool call with an ordinary budget, no execution slot is held while a
person is thinking — `script_limits.rs` never sees a difference.

## What it costs, and the two ways out

Everything before the first `ask` happens on every round trip. A handler that
reads storage and calls `ask` reads storage twice; one that charges a card and
then asks "are you sure?" charges twice.

`tasks.rs` also re-runs handlers, but only after a failure. This re-runs a
prologue that _succeeded_, which is a different and sharper thing, so the engine
offers two answers rather than only documenting the hazard.

**The rule.** A handler that asks must be idempotent up to its last `ask`. Put
the reads, the shaping and the validation before; put the writes after.

**`mcp.once`.** For the prologue that cannot be made idempotent:

```javascript
const draft = mcp.once("draft", () => expensivelyBuildDraft(repo));
```

The function runs on the first pass. Its result is carried in `requestState` and
handed back on every later pass, so it runs exactly once across the whole
exchange. The result has to be JSON and small — it travels to the client and
back on every round trip — so this is for a decision or an identifier, not for a
document.

## `requestState`

An opaque string the client stores and echoes. The specification requires a
server to treat it as attacker-controlled, and to protect its integrity if it
influences authorization, resource access or business logic.

The engine encrypts it (AES-GCM, `security::encryption` — the same primitive
behind sessions and at-rest secrets) rather than merely signing it. Signing
would be enough for integrity, but the blob carries the answers a person has
typed, and those should not sit in plaintext in a client's logs or in whatever
sits between. What goes inside:

- **the principal**, so state minted for one account is refused for another;
- **the host**, which the specification does not ask for and this engine needs:
  a deployment serves several, and state minted on one must not be replayed on
  the next — the same argument realms and token audiences already make;
- **the tool and a digest of its arguments**, so state cannot be moved onto a
  different call;
- **a short expiry**, because this is the span of a person answering a dialog;
- **the answers so far**, and anything `mcp.once` memoized.

It is deliberately **not single-use**. The specification is clear that expiry,
principal and request-binding bound the replay window without guaranteeing
once-only, and that a server needing once-only must enforce it server-side —
which means a table of spent state, which is exactly the shared storage MRTR
exists to avoid. A tool that must act at most once should say so itself; the
engine has that pattern already in `spend_invite` and in refresh-token families.

## When the client cannot ask

Elicitation is a client capability, and a server **MUST NOT** send an input
request the client has not declared. Many clients declare none.

So `mcp.canAsk()` answers whether this caller can be asked, and `mcp.ask` throws
when it cannot, rather than returning something a script might mistake for a
refusal. A tool that can work without the answer should check first and fall
back; a tool that cannot should let the throw stand.

```javascript
const title = mcp.canAsk()
  ? mcp.ask({ message: "Title?", schema: titleSchema }).content.title
  : defaultTitle(repo);
```

`canAsk` is also false wherever there is no client at all — a scheduled job, a
delegated task, a message listener. Background work runs for somebody who is not
there, which is the same reason `delegation.rs` caps it at `authenticated`.

## Three answers, not two

A person can **accept**, **decline** or **cancel**, and the difference matters:
declining is a decision and cancelling is a dismissal. `answer.action` carries
which, and `answer.content` is present only on `accept`. A handler that treats
anything other than `accept` as a failure will report an error for somebody who
simply closed a dialog.

## What is not here

**Form mode only.** `mode: "url"` — the out-of-band flow for credentials and
third-party OAuth, where the answer must not pass through the client at all — is
a separate piece of work, and a larger one than it looks.

It was worth expecting `script_channel_link_tokens` to be the answer to its
anti-phishing requirement, since both are "a URL only one particular person may
complete". They are not the same problem. A link token exists because a Telegram
sender _is not an account_: there is no identity to compare against, so holding
the token — which arrived in a chat only that sender can read — is the only
evidence of ownership the engine can have. A URL-mode elicitation is minted for
somebody who authenticated to `/mcp`, so there is a real identity on both ends,
and the check is a comparison rather than a token: the browser opening the URL
must carry a session for the same account. That is stronger, because a token can
be forwarded to the victim of exactly the attack the requirement is about and a
session cannot.

What URL mode does need is the thing the rest of this file is arranged to avoid:
**server-side state**. The specification says so outright — the server holds the
third-party tokens, so it is stateful. The out-of-band interaction completes
against the engine rather than the client, so its result has nowhere to live but
a table, and `requestState` can only carry a reference to it. That is a
departure worth deciding on deliberately rather than arriving at.

Until it exists, form mode **must not** be used for passwords, API keys, tokens
or payment details; the specification forbids it and the engine does not police
it.

**`resources/read`.** The specification permits `input_required` on
`tools/call`, `prompts/get` and `resources/read`. The first two work — a prompt
that needs a parameter has the same problem a tool does, and asks the same way.
The engine serves no resources, so the third is not a gap.

**Sampling and roots.** Both are deprecated as of `2026-07-28` and are not
implemented. Asking the caller's model to generate something is not available;
a script wanting a model uses its own key, which is what `{{secret:...}}` and
`delegation.rs` are for.
