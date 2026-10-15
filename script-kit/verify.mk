# Format, lint and typecheck for a repository of aiwebengine scripts.
#
# Everything in .aiwebengine/ is a verbatim copy of script-kit/ in the
# aiwebengine repository. CI runs `make check-kit` and then `make verify`
# against the engine's copy (.github/workflows/script-repository.yml there), so
# `make verify` on a developer's machine is the same gate. Include it from the
# repository Makefile:
#
#   include .aiwebengine/verify.mk
#
# The tools run through npx at the versions pinned below, so a repository needs
# node and nothing installed. The type definitions come from the engine:
#
#   make types                             the engine image's, with docker (CI)
#   make types TYPES_FROM=../aiwebengine   an engine checkout's, with cargo
#   make types TYPES_FROM=https://host     what a running engine serves

.PHONY: types format format-check lint typecheck verify sync-kit check-kit

KIT := .aiwebengine
KIT_SOURCE ?= ../aiwebengine/script-kit
ENGINE_IMAGE ?= ghcr.io/lpajunen/aiwebengine:main
TYPES_FROM ?=
TYPES := types/aiwebengine.d.ts

PRETTIER := npx --yes --loglevel=error prettier@3.9.6
MARKDOWNLINT := npx --yes --loglevel=error markdownlint-cli@0.49.1
TSC := npx --yes --loglevel=error -p typescript@6.0.3 tsc

# Prettier leaves alone what .gitignore names; markdownlint is told to.
FORMATTED := "**/*.{js,mjs,jsx,ts,tsx,json,md}"

types:
	@mkdir -p types
	@case "$(TYPES_FROM)" in \
	  "") docker run --rm --pull always --platform linux/amd64 "$(ENGINE_IMAGE)" aiwebengine --print-types ;; \
	  http://*|https://*) curl -fsS "$(TYPES_FROM)/engine/types/v0.1.0/aiwebengine.d.ts" ;; \
	  *) (cd "$(TYPES_FROM)" && cargo run --quiet -- --print-types) ;; \
	esac > $(TYPES).tmp \
	  && mv $(TYPES).tmp $(TYPES) && echo "✓ $(TYPES) from $(or $(TYPES_FROM),$(ENGINE_IMAGE))" \
	  || { rm -f $(TYPES).tmp; exit 1; }

$(TYPES):
	@$(MAKE) --no-print-directory types

format:
	@$(PRETTIER) --write --log-level warn --no-error-on-unmatched-pattern $(FORMATTED)
	@echo '✓ Formatted'

format-check:
	@$(PRETTIER) --check --log-level warn --no-error-on-unmatched-pattern $(FORMATTED)
	@echo '✓ Formatting'

lint:
	@$(MARKDOWNLINT) --ignore-path .gitignore --ignore node_modules "**/*.md"
	@echo '✓ Markdown'

# Scripts against the engine's API; then, where the repository has one,
# browser code under public/ against the DOM.
typecheck: $(TYPES)
	@$(TSC) -p tsconfig.json
	@if [ -f tsconfig.public.json ]; then $(TSC) -p tsconfig.public.json; fi
	@echo '✓ Types'

verify: format-check lint typecheck

# Take the kit from the engine repository. Mirrored, not merged, so a file
# removed there goes here too.
sync-kit:
	@test -d "$(KIT_SOURCE)" || { echo "no kit at $(KIT_SOURCE); set KIT_SOURCE"; exit 1; }
	@rsync -a --delete "$(KIT_SOURCE)/" $(KIT)/
	@echo "✓ $(KIT) synced from $(KIT_SOURCE)"

check-kit:
	@test -d "$(KIT_SOURCE)" || { echo "no kit at $(KIT_SOURCE); set KIT_SOURCE"; exit 1; }
	@diff -r -u "$(KIT_SOURCE)" $(KIT) || { echo; echo "run 'make sync-kit'"; exit 1; }
	@echo "✓ $(KIT) matches $(KIT_SOURCE)"
