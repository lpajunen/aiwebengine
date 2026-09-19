-- What a task is, so the worker knows what to hand its handler.
--
-- 'task' is the ordinary one: the handler is called with the payload under
-- `context.meta.task`. 'message' is one `dispatcher.post` enqueued, and its
-- handler is a listener registered with `dispatcher.registerListener` — so it
-- is called with `messageType` and `messageData` the way an inline
-- `sendMessage` calls it. Without that distinction a listener would behave
-- differently depending on how the message reached it, and reusing the
-- dispatcher's registrations is the whole point of posting one.
ALTER TABLE script_tasks
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'task';

ALTER TABLE script_tasks
    DROP CONSTRAINT IF EXISTS script_tasks_kind_check;

ALTER TABLE script_tasks
    ADD CONSTRAINT script_tasks_kind_check CHECK (kind IN ('task', 'message'));
