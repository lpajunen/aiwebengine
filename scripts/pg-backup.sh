#!/bin/sh
# Dump, list and restore the engine's database.
#
# Runs inside a postgres:16-alpine container so that pg_dump's version matches
# the server's — a dump taken by an older client against a newer server is
# refused outright, and this is the one place that pairing is guaranteed.
#
# Everything it needs is one variable, DATABASE_URL, and it is the *same*
# expression the engine services are given. That is deliberate: a backup
# configured separately from the engine is a backup of a database nobody is
# using, and the failure is silent until the restore.
#
# What this does NOT hold: the four keys in the environment file. A dump taken
# without security.secret_encryption_key is one you cannot fully restore —
# every script and user secret in it stays ciphertext. Back the env file up
# with the dumps, and not in the same place.
set -eu

DIR="${BACKUP_DIR:-/backups}"
KEEP="${BACKUP_KEEP:-14}"
INTERVAL="${BACKUP_INTERVAL_SECONDS:-86400}"

die() {
	echo "pg-backup: $*" >&2
	exit 1
}

[ -n "${DATABASE_URL:-}" ] || die "DATABASE_URL is not set"

# Wait for the server to answer before the first dump. A backup container
# restarting alongside its database would otherwise record one failure per
# restart, and the interval is long enough that the noise outlives the cause.
wait_for_server() {
	i=0
	while ! pg_isready -d "$DATABASE_URL" >/dev/null 2>&1; do
		i=$((i + 1))
		[ "$i" -lt 60 ] || die "database did not answer within 60s"
		sleep 1
	done
}

# One dump, written under a temporary name and renamed only once pg_dump has
# succeeded. A half-written file that carries the final name is worse than no
# file at all: it is the one the restore reaches for.
backup_once() {
	mkdir -p "$DIR"
	ts=$(date -u +%Y%m%dT%H%M%SZ)
	out="$DIR/aiwebengine-$ts.dump"

	# -Fc, the custom format: compressed, and restorable selectively by
	# pg_restore, which a plain SQL file is not.
	if pg_dump -d "$DATABASE_URL" -Fc -f "$out.partial"; then
		mv "$out.partial" "$out"
		echo "pg-backup: wrote $out ($(du -h "$out" | cut -f1))"
	else
		rm -f "$out.partial"
		die "pg_dump failed; nothing was written"
	fi

	prune
}

# Keep the newest BACKUP_KEEP dumps. By count rather than by age, because an
# age window on a volume nobody watches deletes the last copy of a database
# that stopped being dumped a month ago.
prune() {
	# shellcheck disable=SC2012 -- names are generated above and contain no
	# whitespace, and `ls -t` is the only ordering busybox offers here.
	ls -1t "$DIR"/aiwebengine-*.dump 2>/dev/null | tail -n +"$((KEEP + 1))" | while read -r old; do
		echo "pg-backup: pruning $old"
		rm -f "$old"
	done
}

case "${1:-loop}" in
once)
	wait_for_server
	backup_once
	;;
loop)
	wait_for_server
	while true; do
		backup_once || echo "pg-backup: dump failed, retrying next interval" >&2
		sleep "$INTERVAL"
	done
	;;
list)
	ls -1t "$DIR"/aiwebengine-*.dump 2>/dev/null || echo "no dumps in $DIR"
	;;
restore)
	file="${2:-}"
	[ -n "$file" ] || die "usage: pg-backup.sh restore <file>"
	# Accept a bare name as well as a path, since `list` prints paths and an
	# operator types names.
	[ -f "$file" ] || file="$DIR/$file"
	[ -f "$file" ] || die "no such dump: $2"
	wait_for_server
	# --clean --if-exists drops what it is about to replace, so a restore over
	# a populated database is a replacement rather than a merge that fails on
	# every unique constraint. --exit-on-error because a restore that reports
	# success having skipped half the objects is the worst outcome available.
	pg_restore --clean --if-exists --no-owner --no-acl --exit-on-error \
		-d "$DATABASE_URL" "$file"
	echo "pg-backup: restored $file"
	;;
*)
	die "unknown command: $1 (once | loop | list | restore <file>)"
	;;
esac
