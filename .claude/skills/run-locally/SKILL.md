---
name: run-locally
description: Start the engine on this machine (Postgres in Docker, the engine with cargo run on http://localhost:3000) and confirm it answers. Use when asked to run, start or try the engine locally, or to check a change in the running engine rather than only in tests.
---

# Run the engine locally

1. Postgres, in Docker. Skip if `docker ps` already lists
   `aiwebengine-local-postgres`:

   ```bash
   make postgres-local
   ```

2. The engine, in the background so the session stays usable. `caffeinate -i`
   keeps the Mac awake — a sleeping Mac looks like a hang:

   ```bash
   source .env-local && caffeinate -i cargo run
   ```

   Run it with `run_in_background`. The first build takes a few minutes.

3. Wait until it answers, rather than sleeping a fixed time:

   ```bash
   until curl -sf http://localhost:3000/health; do sleep 2; done
   ```

4. Report the URL. Sign-in is a local account: registration is open, and the
   first account named `admin` is an administrator
   (`APP_AUTH__INTERNAL__BOOTSTRAP_ADMIN_USERNAMES` in `.env-local`).

## When it fails

- **Cannot connect to the database:** Postgres is not up, or another Postgres
  holds port 5432. `make postgres-local-logs` shows which.
- **Configuration error at startup:** the validator names the setting:

  ```bash
  source .env-local && cargo run -- --validate-config
  ```

  `${VAR}` in `config.toml` is not expanded; overrides are
  `APP_<SECTION>__<KEY>` environment variables.

- **Port 3000 in use:** an earlier run is still going. Stop it rather than
  starting a second one on another port.

Stop with the background task's stop, and `make postgres-local-stop` when done.
