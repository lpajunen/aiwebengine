# Editor Ideas

Ideas for the web editor in `aiwebengine-dev` (`editor/`) that are not
commitments. The editor has a file tree, an AI assistant, templates and binary
previews; everything below is what it does not have. `docs/HAIKU_AUTHORING.md`
weighs whether the agent replaces the editor and admin tools altogether, which
decides how much of this is worth building.

## Using what the engine already offers

The engine has operations the editor does not surface yet. Each is a panel over
an existing call rather than new engine work:

- **History.** `list_revisions`, `diff_revisions`, `label_revision` and
  `revert_script`: a timeline per script with a unified diff, labels, and
  revert with a `dryRun` preview first.
- **Deployment.** `deploy_script` and `get_deployment`: show what is serving
  against head, pin a revision, follow head again.
- **Checks and tests.** `check_script` before saving, and `run_tests` with a
  verdict per case, against an unsaved overlay as well as a stored revision.
- **Evaluation.** An `eval_script` console beside the editor.
- **Logs.** A live tail from `/engine/script_logs/stream`, filtered to the
  script and to one request's `invocationId`.
- **Git.** `get_git_status`, `pull_from_git` and `push_to_git` for a bound
  script.

## The assistant

- **Changes as one batch.** The assistant proposes a multi-file change as a
  `write_files` payload, shown as a diff and applied as one revision.
- **Writing tests.** Generate `*.test.ts` for a module and run them in place.
- **Review.** Ask for a review of the current diff against head, with the
  engine's own rules (exposure directories, capability checks) in its prompt.
- **Context budget.** Choosing which other files to include, by import graph
  first, with the token cost shown.

## Editing files that are not code

- **CSS and SVG** with a live preview pane beside the source, rendered from the
  `public/` path the file is served at.
- **Markdown** with a preview, since `.md` files are both documents a script
  serves and string modules it imports (prompts, skills).

## Layout

- Split view of two files, and an indicator of which files the assistant
  currently has in context.
- Hover documentation from `aiwebengine.d.ts` for the globals.
