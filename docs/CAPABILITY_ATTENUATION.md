# Running With Less Than You Hold

```javascript
// A planning turn: everything readable, nothing writable. Enforced under the
// JavaScript, not by a filtered tool list and a polite prompt.
const plan = sandbox.run(modelWroteThis, {
  capabilities: ["read_script_data", "read_storage", "read_assets"],
  input: { question },
});

if (plan.error) {
  // Code that did not work is the *result* of the turn, not a failure of the
  // loop that ran it. Feed it back to the model.
  return askAgain(plan.error, plan.console);
}
```

A `UserContext` only ever flowed downward from the caller. A request arrives as
somebody, a delegation caps background work at what was consented to, a script
limit bounds what one script may spend — all of them narrow on the way _in_.
None of them let a script narrow **itself**, so the smallest thing a script
could run something with was everything it had.

`sandbox.run` is the way down: a second execution of the same script, in a
context holding a chosen subset, with JSON the only thing that crosses between
them.

## The two things it is for

**Code as the action.** The interesting shape for an agent hosted here is the
model writing JavaScript rather than emitting a tool call — composition comes
free and the tool list stops growing. Every other project adopting that pattern
has to build a sandbox first. This engine already is one; what was missing was
running model-authored code holding _less_ than the script that asked for it.

**A plan mode that is enforced rather than offered.** Every harness that ships
one implements it as a filtered tool list plus a prompt asking the model
nicely. None can actually enforce read-only, because the filtering lives in the
agent loop and the model writes the transcript. Here the capability checks are
underneath the JavaScript, so a planning turn simply runs in a context holding
the reads and none of the writes.

## The capability vocabulary

Narrowing is only worth as much as the vocabulary can express, and the old one
could not express the things an agent's tools actually do. `fetch` was gated by
nothing at all. `secretStorage` and `personalStorage` were gated only by a
delegation scope. Reads and writes of a script's tables shared one capability,
so **read-only was inexpressible**.

| Name                     | Gates                                                   |
| ------------------------ | ------------------------------------------------------- |
| `read_scripts`           | Reading script source                                   |
| `write_scripts`          | Writing it                                              |
| `delete_scripts`         | Deleting a script you own                               |
| `read_assets`            | `assetStorage.listAssets`, `fetchAsset`                 |
| `write_assets`           | `assetStorage.upsertAsset`                              |
| `delete_assets`          | `assetStorage.deleteAsset`                              |
| `view_logs`              | Writing to the script's log — what `console` does       |
| `delete_logs`            | Clearing it                                             |
| `manage_streams`         | Registering streams and sending stream messages         |
| `manage_graphql`         | Every `graphQLRegistry` call                            |
| `read_script_data`       | `database.query`                                        |
| `write_script_data`      | `insert`, `update`, `delete`, `upsert`, `deleteWhere`   |
| `manage_script_database` | Creating and dropping tables, columns, indexes          |
| `administer_engine`      | Acting on what you do not own                           |
| `use_network`            | `fetch`                                                 |
| `read_secrets`           | `secretStorage.exists`, and `{{secret:…}}` in a `fetch` |
| `write_secrets`          | `setSecret`, `removeSecret`, `clear`                    |
| `read_storage`           | Reading `scriptStorage` and `personalStorage`           |
| `write_storage`          | Writing or clearing either                              |
| `enqueue_tasks`          | `scriptTasks` and `personalTasks` — enqueue and cancel  |
| `send_messages`          | `dispatcher.sendMessage`                                |

The seven script-side names at the bottom of that table are new, and **adding
them took nothing away from anybody**. Every tier that could already do the
thing holds the name for it, including the anonymous tier, which could always
`fetch` and reach `scriptStorage`. They exist to be taken away.

Two things are not capabilities and cannot be narrowed by this. **Ownership**
is a separate check (`user_owns_script`) — a narrowed turn owns exactly what
its caller owned. And **computing** is not a capability: a sub-execution given
nothing at all still runs, which is the base case the rest subtract from.

## What the rules are

**Narrowing only.** `capabilities` must be a subset of what the calling turn
holds. Asking for more throws rather than quietly narrowing, because a script
asking to keep something it never had has a bug, and one that would otherwise
show up later as a puzzling refusal from inside the sub-execution. Underneath
that check the narrowing is an intersection regardless, so there is no order of
calls that widens anything.

**Omitted means none.** `sandbox.run(source)` with no `capabilities` grants
none of them. Defaulting the other way would mean a typo in the option name —
`capability`, `caps` — silently handed model-authored code the whole of the
caller's authority, which is the one mistake this API exists to make
impossible.

**An unknown name is refused**, not ignored. A name nothing gates on would be a
promise to the caller that nothing keeps.

**The identity carries through.** Attenuation says what may be done, not who is
doing it. `personalStorage`, `{{secret:…}}` and ownership all go on resolving
against the same account — a narrower part of that person's data rather than
nobody's. The role flags on `context.request.auth` carry through unchanged too:
a narrowed turn belonging to an administrator still reads `isAdmin` and is
still refused at every gate it no longer holds. The refusal is the enforcement,
not the flag.

**Narrowing composes downward.** A sub-execution can narrow again, and cannot
reach back out to what its own caller dropped.

**Nesting is bounded** at four levels deep. The shared budget bounds how _long_
a chain runs without bounding how deep it goes, and each level is a live
runtime and a stack frame.

## What crosses the boundary

`input` in, a JSON value out, and `console` output either way.

That is the isolation rather than a limitation of it. A restricted region
sharing an object graph with its unrestricted parent could be handed a function
by that parent and call it — a capability leak by reference, which is the
classic way a same-realm sandbox fails. Separate contexts cannot pass anything
but JSON.

What the sub-execution _does_ see is the script itself: its own program is
evaluated first, in the same realm, so the source can call the script's helpers
and whatever its entrypoint imported. `input` arrives as `context.args`.

## Errors: thrown or returned

The line is drawn where an agent needs it.

- The **request** was wrong — a capability that is not one, one the caller does
  not hold, nesting too deep. `sandbox.run` **throws**. That is a bug in the
  script, and it should fail where it was written.
- The **code** was wrong — it threw, looped until the budget ran out, or was
  refused something it had not been given. `sandbox.run` **returns**, with
  `error` set. That is the result of the turn, and what an agent does with it
  is feed it back to the model. Throwing would make every agent wrap every call
  in a try/catch to recover a value it always wanted.

`console` comes back either way, and is usually the most useful part of a turn
that failed.

Note that a refusal reaches the code in whichever way that API already reports
failures. `fetch` and `scriptTasks` throw; `database` answers with
`{ error }` in its envelope; `scriptStorage` and `personalStorage` raise a
`DOMException`. Every one of them names the capability that was missing, and
says _narrowed_ when the reason was this rather than the caller's own tier.

## What it costs

A fresh QuickJS runtime and one re-evaluation of the script's program, per
call. That is the price of the isolation rather than an implementation detail
to optimise away — the program has to be evaluated _in_ the narrowed context to
be narrowed at all.

It is the right price for an agent turn and the wrong one for a loop. Narrow
once around the turn, not once around each tool call.

Two things that would otherwise make nesting wrong are already right: a nested
runtime's budget is clamped to the parent's remaining time, so a chain cannot
outlive the request that started it, and a rollback inside one becomes a
`SAVEPOINT` rather than interfering with the caller's transaction.

## Worked example: a plan step and an act step

```javascript
// The turn the model gets to think in. It can read everything the solution
// has and change none of it, whatever the transcript says.
const READ_ONLY = sandbox.held().filter((c) => c.startsWith("read_"));

function planTurn(question) {
  return sandbox.run(promptTheModelWroteCodeFor(question), {
    capabilities: READ_ONLY,
    input: { question },
    timeoutMs: 20000,
  });
}

// The turn that carries the plan out, once a person has approved it. Still
// narrower than the script itself: no secrets, no queueing, no messaging.
function actTurn(plan) {
  return sandbox.run(plan.code, {
    capabilities: [
      ...READ_ONLY,
      "write_script_data",
      "write_storage",
      "use_network",
    ],
    input: { plan: plan.value },
  });
}
```

## See also

- [Delegation](DELEGATION.md) — what a person authorised a script to do as them
  while they are away. That narrows by consent; this narrows by choice, and an
  execution that is both is subject to each.
- [Script Limits](SCRIPT_LIMITS.md) — what one script may _spend_, as opposed
  to what it may reach.
- [Script Eval](SCRIPT_EVAL.md) — the same evaluation machinery, as an
  administrator's endpoint rather than as something a script calls on itself.
