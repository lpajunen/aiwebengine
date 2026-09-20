-- The verb the scope vocabulary did not have.
--
-- `script_delegations.scopes` held two nouns — personal_storage, secrets —
-- naming what a delegation reaches. Nothing named what it may do with them,
-- so every grant was a grant to change as well as to read. A read-only scope
-- needs a read-only context to enforce it, and a UserContext had no way to
-- hold less than a tier until capability attenuation; that is why the verb
-- arrives now and not with the nouns.
--
-- From here, `write` is what a delegated run needs before it holds any write
-- capability at all: the script's tables, either storage, the queue, the
-- message dispatcher.
--
-- Every existing grant gets it, which is not a widening. The consent page
-- those people saw offered "Read and change the data this app keeps for you",
-- so changing is what they agreed to, and the run they authorised already
-- holds every write a delegation can hold. Leaving the column alone would
-- silently narrow live delegations to something nobody chose, and a
-- background job that quietly stops writing is the worst way to learn about
-- a vocabulary change. The page now asks the question separately, so consent
-- given from here on is explicit.
UPDATE script_delegations
SET scopes = array_append(scopes, 'write')
WHERE NOT ('write' = ANY (scopes));
