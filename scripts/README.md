# scripts/

Operator helpers. Each is run through a `make` target; the header comment of
each script says why it exists.

| Script                | Make target                                             | What it does                                                  |
| --------------------- | ------------------------------------------------------- | ------------------------------------------------------------- |
| `pg-backup.sh`        | `docker-backup`, `docker-backup-list`, `docker-restore` | Dumps, lists and restores the database (the `backup` service) |
| `check-dns.sh`        | `check-dns`                                             | Checks the local DNS name resolves                            |
| `acme-dns-cleanup.sh` | `clean-acme-dns`                                        | Removes stale ACME DNS-01 challenge records                   |

For a local database use `make postgres-local`; the engine applies the
migrations in `migrations/` itself when it starts.
