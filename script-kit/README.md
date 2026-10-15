# Script kit

How a repository of aiwebengine scripts checks its formatting, its Markdown and
its types: the same way in every repository, and the same way on a developer's
machine as in CI.

This directory lives in the aiwebengine repository as `script-kit/`. Each
script repository holds a verbatim copy as `.aiwebengine/`; CI fails when the
copy differs from the engine's, and `make sync-kit` takes the engine's version.

## Using it

```bash
make types       # types/aiwebengine.d.ts from the engine (see below)
make verify      # format-check + lint + typecheck: what CI runs
make format      # rewrite files in place
```

Needs `make` and `node`. Prettier, markdownlint and TypeScript run through
`npx` at the versions `verify.mk` pins, so nothing is installed in the
repository for them.

`types/aiwebengine.d.ts` is generated, never committed. It is the file an
engine serves at `/engine/types/v0.1.0/aiwebengine.d.ts`: the declarations with
the operation table and the limits filled in, which `aiwebengine --print-types`
writes without a database or keys. `make types` gets it from:

| `TYPES_FROM`          | Source                                        | Needs  |
| --------------------- | --------------------------------------------- | ------ |
| unset                 | the engine image built from `main` (CI)       | docker |
| a directory           | an engine checkout, `cargo run --print-types` | cargo  |
| `http://`, `https://` | a running engine                              | curl   |

An engine checkout is the one to use while changing the engine and scripts
together.

## What a repository adds

```text
Makefile              include .aiwebengine/verify.mk
tsconfig.json         scripts, against the engine's API
tsconfig.public.json  browser code under public/ (optional)
.markdownlint.json    { "extends": ".aiwebengine/markdownlint.json" }
.gitignore            types/
.aiwebengineignore    the files above, so a script does not carry them
.github/workflows/verify.yml
```

`tsconfig.json` extends `.aiwebengine/tsconfig.base.json` and names what to
check. Node tooling lives in `scripts/` or `tools/`, and browser code in
`public/`, so the usual one is:

```json
{
  "extends": "./.aiwebengine/tsconfig.base.json",
  "include": ["**/*.ts", "**/*.tsx", "**/*.js", "**/*.jsx"],
  "exclude": ["node_modules", "scripts", "tools", "**/public/**"]
}
```

The base checks JavaScript as strictly as TypeScript, and treats every file as
a module, so two scripts' `init()` functions do not collide. Browser code is
different: its files share one global scope across `<script>` tags and use the
DOM, so `tsconfig.public.json` extends `tsconfig.browser.json` instead and
includes `**/public/**/*.js`. A repository without it does not check browser
code.

The workflow calls the engine's:

```yaml
name: Verify
on:
  push:
    branches: [main]
  pull_request:
jobs:
  verify:
    uses: lpajunen/aiwebengine/.github/workflows/script-repository.yml@main
```

## Changing it

Change `script-kit/` in the engine, then `make sync-kit` in every script
repository and commit there. CI in a repository that has not synced fails at
`check-kit` until it does.
