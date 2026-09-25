-- The task kind said whether a row was an ordinary task or a posted message,
-- and only the dispatcher ever wrote the second value. With `dispatcher` gone
-- there is one kind left, so the column says the same thing about every row.
--
-- Rows are not deleted. A queued message names a listener handler that no
-- longer exists, and the worker reports that the same way it reports any
-- handler it cannot find — which is a readable failure with `last_error` set,
-- rather than a row vanishing from under whoever queued it.
ALTER TABLE script_tasks
    DROP CONSTRAINT IF EXISTS script_tasks_kind_check;

ALTER TABLE script_tasks
    DROP COLUMN IF EXISTS kind;
