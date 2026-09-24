-- What an MCP client is told about a long-running tool call.
--
-- Deliberately a second table rather than columns on `script_tasks`, because
-- the two have opposite lifetimes and that is the whole reason this exists.
-- `script_tasks` is a queue: a task that succeeds has its row deleted, since
-- one row per success grows the table for the outcome nobody debugs. An MCP
-- task is the other way round — the client has not seen the answer yet, so a
-- completed task is precisely the row that has to survive, until its TTL says
-- the client has had long enough to come back for it.
--
-- Folding them together would have meant making the queue keep its successes,
-- which is the decision `script_tasks` explicitly made the other way.
CREATE TABLE IF NOT EXISTS mcp_tasks (
    -- What the client holds and polls with. Its own identifier rather than the
    -- queue's, so the handle a client persists does not name an internal row
    -- whose lifetime it does not control.
    task_id UUID PRIMARY KEY,
    -- The queue row doing the work. Nullable because it outlives that row: a
    -- succeeded script task is deleted, and this one stays to be read.
    script_task_id UUID,
    script_uri TEXT NOT NULL,
    -- The MCP method the client called, kept so a `tasks/get` can say what the
    -- task is for without the client having to remember.
    method TEXT NOT NULL,
    -- The tool or prompt name, same reason.
    target TEXT NOT NULL,
    -- The extension's own vocabulary, not the queue's. `input_required` is
    -- representable and currently unreachable: a background run asking a
    -- person a question is a design with no answer yet, and a status the
    -- engine cannot produce is better than one the check constraint forbids
    -- the day it can.
    status TEXT NOT NULL DEFAULT 'working'
        CHECK (status IN ('working', 'input_required', 'completed', 'failed', 'cancelled')),
    -- Free text a script sets to report progress. Shown to the client.
    status_message TEXT,
    -- What the original request would have returned synchronously, present
    -- once `status = 'completed'`.
    result JSONB,
    -- The JSON-RPC error, present once `status = 'failed'`.
    error JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- When the handle stops being answerable. The extension lets a server say
    -- `null` for "no expiry"; the engine always sets one, because a table of
    -- results nobody collected is a table that only grows.
    expires_at TIMESTAMPTZ NOT NULL
);

-- The poll. Every `tasks/get` is a primary-key lookup, so the index that earns
-- its place is the one the pruner uses.
CREATE INDEX IF NOT EXISTS idx_mcp_tasks_expires_at ON mcp_tasks (expires_at);

-- Finding the MCP task a finished queue row belongs to, which is what the
-- worker does on every completion.
CREATE INDEX IF NOT EXISTS idx_mcp_tasks_script_task ON mcp_tasks (script_task_id)
    WHERE script_task_id IS NOT NULL;

-- Deleting a script takes its outstanding task handles with it, the same way
-- `script_tasks` goes. A handle naming a script that no longer exists could
-- never complete.
CREATE INDEX IF NOT EXISTS idx_mcp_tasks_script_uri ON mcp_tasks (script_uri);
