# Several Requests At Once, And One A Piece At A Time

```javascript
// Three tool calls together. One delay, not three.
const [weather, news, mail] = fetchAll([
  "https://api.example.com/weather",
  { url: "https://api.example.com/news", options: { method: "POST", body } },
  "https://api.example.com/mail",
]);

// A model's answer, as it is written rather than when it is finished.
const stream = fetchStream("https://api.example.com/v1/messages", {
  method: "POST",
  headers: { Authorization: "Bearer {{secret:API_TOKEN}}" },
  body: JSON.stringify({ stream: true, messages }),
});

for (const chunk of stream) {
  routeRegistry.sendStreamMessage("answer", chunk);
}
```

Both of these come from one place. `fetch` was a single blocking host call
that performed the whole request and handed back a string, which made two
different things impossible.

**Several at once.** `Promise.all` over three fetches gives the right answers
and runs them one after another: each request has already finished by the
time its `fetch` returns, so each holds an execution slot and a blocking
thread for its whole round trip and the wall clock is the sum. For an agent
running three tool calls that is not an ergonomic complaint — it is the
difference between fitting inside the execution budget and not.

**One as it arrives.** A response that comes over time could not be consumed
at all, which is every model's token stream. An agent's page updated once per
turn, and a turn was as long as the whole model call.

## What `fetchAll` gives you

One thread waits on the batch rather than one per request in series. Each
request is the same `fetch` — the same URL and DNS validation, the same
manually followed and re-validated redirects, the same secret substitution —
run on a worker rather than on the caller's thread.

- **Answers are positional.** The nth answer belongs to the nth request,
  whatever order they arrived in. Matching by position is the obvious thing
  to write, so it has to be the thing that works.
- **A failure is per request.** A refused URL answers `ok: false` and throws
  when you read its body, so the answers that arrived are still usable. The
  caller asked for several answers and has a use for the ones that came.
- **A big batch runs in waves.** Eight are in flight at once; more are run in
  turns rather than refused. Each in-flight request holds a thread the rest
  of the engine shares, and the caller's intent is legible either way — the
  only question is how fast it happens.
- **The budget travels.** Every host call reads its deadline from a
  thread-local, so work handed to another thread would find none and take its
  own full timeout. A script with two seconds left must not be able to start
  a thirty-second fetch, so the remaining budget is read on the calling
  thread and armed again on each worker.

## What `fetchStream` gives you

The status and headers as soon as they arrive, and the body in pieces as they
do. It is iterable, so the ordinary shape is a `for...of`.

Three things are worth knowing before you use one.

**A chunk is not a line and not an SSE event.** Boundaries fall wherever the
network put them. Reassembling whatever you are actually reading — SSE
frames, newline-delimited JSON — is the caller's job, because only the caller
knows which it is.

**No content coding is requested.** `fetch` offers gzip and undoes it after
reading the whole body, which a stream cannot do: undoing a coding
incrementally is a decoder and a buffer of its own. Asking for identity is
the honest way to say that rather than discovering it as `invalid utf8`
halfway through a response. Endpoints that stream are not compressed in
practice.

**Close what you stop reading.** The connection stays open between reads. A
stream read to the end closes itself, and every stream an execution holds
closes when the execution does — but one abandoned in the middle holds a
socket until then, and an execution may hold only a few at once.

Multi-byte characters split across chunks are reassembled for you. A chunk
boundary regularly falls inside one, and decoding each read on its own would
replace the halves with `U+FFFD` — silently, and most often on exactly the
text a model is generating.

## The pieces that were already there

The outbound half of streaming was built before this: `stream_registry` and
`routeRegistry.sendStreamMessage` push to a person's open page today. What
was missing was a `fetch` that hands back a reader, and the bridge between
the two is the `for...of` in the example at the top.

## What this is not

It is not an event loop. There is no `await` that yields, no timer, no
callback that runs later — a host call still blocks the script while it runs.
What changed is how much work one blocked call can be waiting on, and whether
it has to wait for all of it before answering.

So `fetchAll` overlaps requests but not a request with your own computation,
and `fetchStream` blocks until the next piece arrives. For a script that
wants to do something else while a request is in flight, the shape is still a
[task](SCRIPT_TASKS.md).

## Limits

| What                         | Ceiling                                    |
| ---------------------------- | ------------------------------------------ |
| Requests in flight per batch | 8, the rest in waves                       |
| Open streams per execution   | 8                                          |
| Bytes read over one stream   | 10MB, the same ceiling a buffered body has |

A stream answers to the response-size ceiling across its whole life, not per
read. A response that never ends is the case the cap exists for, and a
streaming reader is what makes it reachable.

## See also

- [Script Tasks](SCRIPT_TASKS.md) — for work that should outlive the request.
- [Capability Attenuation](CAPABILITY_ATTENUATION.md) — `use_network` gates
  all three of `fetch`, `fetchAll` and `fetchStream`, and `read_secrets`
  gates a `{{secret:...}}` in any of them.
