# Post-switch hooks, and whether we need them

Research for the question: should `switch` gain a hook mechanism so that `switch wt fix/xyz`
opens the new worktree as a herdr Space, and `switch wt rm .` closes it again?

Primary sources only — official docs, man pages, source, first-party CLI output and schemas.
Every claim below carries a URL or a file path. Where no primary source exists, it says so.

Researched 2026-08-13 against herdr 0.8.0 (protocol 19), git 2.55.0, `gh` 2.97.0, cargo 1.97.1.

**This is a point-in-time snapshot, not a specification.** Every version above will drift, and
the sections below are only as true as those versions. What was *decided* on the strength of it
lives in [ADR 0003](../adr/0003-hooks-are-told-never-asked.md) — where this document and the ADR
disagree, the ADR is the decision and this is the evidence it was weighed against.

---

## Summary of what was found

The headline finding is that **the motivating integration does not need a hook mechanism in
`switch` at all** — herdr ships a first-party CLI that does exactly the two things wanted, and
git already fires `post-checkout` on `git worktree add`. There is one real gap: git fires *no*
hook on `git worktree remove`, so the teardown half has no existing seam.

Three findings drive that:

1. **herdr is not a black box.** It is Apache-2.0, open source, and exposes a documented
   newline-delimited JSON socket API with a `worktree.*` and `workspace.*` method family, a
   bundled JSON Schema (`herdr api schema --json`), and a plugin system whose event catalogue
   already includes `worktree.created` / `worktree.opened` / `worktree.removed`. Programmatic
   mutation is fully supported and documented. No private state needs poking.
2. **`git worktree add` already runs `post-checkout`.** A user-global `core.hooksPath` hook, or a
   `hook.<name>.command` config hook, can call `herdr worktree open` today with zero changes to
   `switch`.
3. **`git worktree remove` runs nothing.** Verified against githooks(5), `builtin/worktree.c`,
   and empirically. This is the only place where `switch` owning a hook adds capability that
   git does not already provide.

The prior art is unanimous on one design point: for a tool that operates on repos you cloned
from elsewhere, hook config belongs **user-global or in repo metadata — never in the tracked
tree**. jj says so in those words; gh structurally forecloses it; cargo took the opposite bet
and is spending years trying to claw back a sandbox.

---

## 1. herdr

### 1.1 What it is

Not private, not closed-source.

- Homepage `https://herdr.dev`, source `https://github.com/herdrdev/herdr`, licence Apache-2.0.
  From the installed Homebrew formula, `/opt/homebrew/Cellar/herdr/0.8.0/.brew/herdr.rb`:

  ```ruby
  class Herdr < Formula
    desc "Agent multiplexer that lives in your terminal"
    homepage "https://herdr.dev"
    url "https://github.com/herdrdev/herdr/archive/refs/tags/v0.8.0.tar.gz"
    license "Apache-2.0"
    head "https://github.com/herdrdev/herdr.git", branch: "master"
  ```

- Self-description, `herdr --help` (herdr 0.8.0):
  `herdr — terminal workspace manager for AI coding agents`
- It is a Rust binary with a client/server split. The server is a long-lived daemon
  (`brew services start herdr`, or `herdr server`); clients talk to it over a Unix domain socket.

### 1.2 Yes, it has a CLI — and a documented socket API

`herdr --help` lists these command groups, all of which are described as helpers "over the
socket API": `api`, `config`, `channel`, `workspace`, `worktree`, `tab`, `notification`,
`agent`, `pane`, `session`, `integration`.

Transport, from <https://herdr.dev/docs/socket-api/>:

> "Herdr uses newline-delimited JSON over a local socket. On Unix, that socket is a Unix domain
> socket. On Windows, it is a named pipe."

Request/response envelope, same page:

```json
{"id":"req_1","method":"ping","params":{}}
{"id":"req_1","result":{"type":"pong"}}
```

Socket path resolution, same page, in precedence order: the `--session <name>` flag, then
`HERDR_SOCKET_PATH`, then `HERDR_SESSION=<name>`, then `~/.config/herdr/herdr.sock`. Named
sessions live at `~/.config/herdr/sessions/<name>/herdr.sock`.

Verified live on this machine:

```
$ herdr status server
status: running
version: 0.8.0
protocol: 19
compatible: yes
socket: /Users/lckn/.config/herdr/sessions/personal/herdr.sock
```

The API is versioned and self-describing: `herdr api schema` prints a summary
(`protocol: 19`, `schema_version: 1`), `herdr api schema --json` prints a full JSON Schema
2020-12 document (~250 KB on 0.8.0) covering `request`, `success_response`, `error_response`,
`event` and `subscription_event`. That is the authoritative machine-readable contract.

Stability caveat, from <https://herdr.dev/docs/socket-api/>:

> "Herdr has a protocol version for client/server compatibility." … "Check the server protocol
> with `ping` or `herdr status` before depending on new behavior. Handle unknown fields
> gracefully."

### 1.3 "Spaces" is the UI name for a workspace

The docs' conceptual page uses "workspace" — <https://herdr.dev/docs/concepts/>:

> "A workspace is the top-level project container. Use one workspace per repo, task, or
> investigation."

"Space" is the same object as rendered in the sidebar. From the config reference,
<https://herdr.dev/docs/config-reference/>, key `ui.sidebar.spaces.rows`:

> "Expanded Space sidebar layout. Entries may be token strings or inline
> `{ token, fg, bold, dim }` style tables."

Corroborated by the shipped CHANGELOG, `/opt/homebrew/Cellar/herdr/0.8.0/CHANGELOG.md`:

> "Added configurable row layouts for expanded Space and Agent sidebar entries…"

and by `ui.agent_panel_sort = "spaces"`, which is set in this machine's
`~/.config/herdr/config.toml`.

So: **a herdr Space is a herdr workspace.** The API and CLI say `workspace`; the UI and config
say Space. Anything below that says "workspace" is what you would type.

### 1.4 There is a documented way to add and remove a git worktree from a Space

This is the crux, and the answer is unambiguously yes.

`herdr worktree --help`:

```
Manage Git worktree-backed workspaces

Commands:
  list    List worktree workspaces
  create  Create and open a Git worktree
  open    Open an existing Git worktree
  remove  Remove a worktree checkout
```

Full signatures, from <https://herdr.dev/docs/cli-reference/> (flag lists independently
confirmed against the shipped zsh completions at
`/opt/homebrew/Cellar/herdr/0.8.0/share/zsh/site-functions/_herdr`):

> `herdr worktree list [--workspace ID | --cwd PATH]`
>
> `herdr worktree create [--workspace ID | --cwd PATH] [--branch NAME] [--base REF] [--path PATH] [--label TEXT] [--focus] [--no-focus]`
> — "Creates a Git worktree checkout and opens it as a workspace. If `--branch` references an
> existing local branch, it checks that out; otherwise creates the branch from `--base` or
> `HEAD`. Without `--path`, the checkout appears under `<worktrees.directory>/<repo>/<branch-slug>`."
>
> `herdr worktree open [--workspace ID | --cwd PATH] (--path PATH | --branch NAME) [--label TEXT] [--focus] [--no-focus]`
> — "Opens an existing Git worktree checkout as a workspace."
>
> `herdr worktree remove --workspace ID [--force]`
> — "Explicitly deletes the checkout from disk by running `git worktree remove`. Never deletes
> the branch. Requires `--force` when Git refuses a dirty checkout."
>
> `herdr workspace close <workspace_id>`
> — "Closes Herdr state only (does not delete associated Git worktree checkouts)."

**The two commands that matter for this design are `worktree open` and `workspace close`, not
`worktree create` / `worktree remove`.** `switch` already creates and destroys the checkout
itself (`src/app/wt.rs:333` calls `git::worktree_add`; removal goes through
`src/app/removal.rs`). Handing that job to herdr would mean giving up everything `switch`
knows about staleness, risk markers and licensing. What is wanted is the *adopt* and
*disown* halves:

- after `switch wt fix/xyz` creates the checkout → `herdr worktree open --path <path> --no-focus`
- before `switch wt rm .` destroys it → `herdr workspace close <id>`

`workspace close` is the right teardown because it is explicitly documented as not touching the
checkout — `switch` has already removed it, or is about to.

Corresponding socket methods, from `herdr api schema --json` on 0.8.0 — the request `oneOf`
includes `worktree.list`, `worktree.create`, `worktree.open`, `worktree.remove`,
`workspace.create`, `workspace.close`, `workspace.focus`, `workspace.rename`,
`workspace.report_metadata`. `WorktreeOpenParams`:

```json
{"properties": {
  "branch":       {"type": ["string","null"]},
  "cwd":          {"type": ["string","null"]},
  "focus":        {"default": false, "type": "boolean"},
  "label":        {"type": ["string","null"]},
  "path":         {"type": ["string","null"]},
  "workspace_id": {"type": ["string","null"]}},
 "type": "object"}
```

`WorktreeRemoveParams` requires `workspace_id` and takes `force` (default `false`).

### 1.5 Mapping a path to a Space id

`herdr worktree list` returns the mapping, keyed by checkout path. Run live in this repo:

```
$ herdr worktree list
{"id":"cli:worktree:list","result":{"source":{"repo_key":"/Users/lckn/Developer/Personal/Rust/git-switch/.git",
 "repo_name":"git-switch","repo_root":"/Users/lckn/Developer/Personal/Rust/git-switch",
 "source_checkout_path":"/Users/lckn/Developer/Personal/Rust/git-switch","source_workspace_id":"wH"},
 "type":"worktree_list","worktrees":[{"branch":"main","is_bare":false,"is_detached":false,
 "is_linked_worktree":false,"is_prunable":false,"label":"git-switch","open_workspace_id":"wH",
 "path":"/Users/lckn/Developer/Personal/Rust/git-switch"}]}}
```

`open_workspace_id` is the Space holding a given checkout. It is **absent from the object
entirely** when the worktree is not open as a Space — not `null` — so a consumer has to tolerate
a missing key rather than test for null. Verified 2026-08-13 against herdr 0.8.0 on a throwaway
repo open in no Space: every row came back without the key at all.

`worktree list` is derived from git's own worktree registry — its rows carry `is_prunable` and
`is_linked_worktree`, and a row vanishes the moment `git worktree remove` runs. It therefore
cannot answer "which Space held the checkout that was just removed".

`workspace list` can. That is herdr's own state, and each Space backed by a checkout carries a
`worktree` object with `checkout_path`, `repo_root` and `repo_key`, which outlives the git
worktree. Matching `checkout_path` against the removed path recovers the id after the fact:

```
$ herdr workspace list
… {"label":"git-switch","workspace_id":"wH","worktree":{
    "checkout_path":"/Users/lckn/Developer/Personal/Rust/git-switch",
    "is_linked_worktree":false,"repo_key":"…/.git","repo_root":"…/git-switch"}}
```

This retracts an ordering constraint an earlier draft of this document asserted: teardown does
**not** have to look the id up before removing the checkout.

### 1.5.1 Scoping: `--cwd` is not optional

`worktree list`, `worktree open` and their siblings all take `--workspace <ID>` and `--cwd
<PATH>`. Given neither, herdr resolves the target repo from its own **focused** Space — not from
the calling process's working directory. Verified 2026-08-13: `herdr worktree list` run from
inside a freshly created throwaway repo returned the *git-switch* repo's worktrees, because that
was what herdr had focused at the time.

For anything scripted this is a silent-wrong-repo hazard rather than an error: the command
succeeds, against a repo nobody named. Any hook must pass `--cwd` explicitly.

### 1.6 State: a daemon, plus TOML config, plus a JSON session file

Not a single config file — three distinct things, and only one of them is user-editable.

- **Live state lives in the server**, reachable only over the socket. `herdr api snapshot`
  prints it. This is the part `switch` would mutate, and it is mutated through the documented
  API, not by editing a file.
- **User config** is TOML. `herdr --help` reports the active path; on this machine
  `~/.config/herdr/config-personal.toml`. `herdr --default-config` prints the annotated
  default. Relevant section, from `herdr --default-config`:

  ```toml
  # [worktrees]
  # directory = "~/.herdr/worktrees"
  ```

  documented at <https://herdr.dev/docs/config-reference/> as:

  > "Root directory under which Herdr creates `<repo>/<branch-slug>` checkouts."

- **Session state** is `~/.config/herdr/session.json` plus `~/.config/herdr/sessions/<name>/`.
  Undocumented internally-owned state; nothing should write it.

**Programmatic mutation is a first-class, documented capability.** No private state needs
poking. From <https://herdr.dev/docs/plugins/>:

> "The entire Herdr CLI is the plugin API: every command in the CLI reference is available to a
> plugin, and anything you can run as `herdr ...` yourself a plugin can run too."

And on output conventions, from <https://herdr.dev/docs/cli-reference/>:

> "A timeout or server error is emitted as JSON on stderr with exit status 1. CLI usage errors
> exit with status 2."

confirmed by `herdr --skill`:

> "CLI server errors are JSON on stderr with exit status 1. CLI syntax errors exit with status 2."

### 1.7 A path-layout mismatch worth knowing about

`switch` and herdr disagree about where worktrees go, and about slashes.

- `switch`, `src/app/wt.rs:503-513` (`worktree_path_for`): `<main-parent>/worktrees/<repo>/<branch>`.
  README.md:99 — "Worktrees land at `../worktrees/<repo>/<branch>` relative to the main
  checkout. Branch names with slashes (`feature/foo`) preserve their structure as
  subdirectories."
- herdr: `<worktrees.directory>/<repo>/<branch-slug>`, default root `~/.herdr/worktrees`
  (<https://herdr.dev/docs/config-reference/>). Note **slug**, not path — a slash in
  `fix/xyz` is flattened, not nested.

This machine sets `directory = "/Users/lckn/Developer/Personal/worktrees"` in
`~/.config/herdr/config-personal.toml`, which for this repo still does not coincide with
`switch`'s `/Users/lckn/Developer/Personal/Rust/worktrees/git-switch/<branch>`.

The mismatch does not block anything, because `worktree open --path PATH` takes an explicit
path. It only means `switch` must always pass `--path`, and must never rely on herdr's
default layout or on `--branch` resolution.

### 1.8 herdr's own hook mechanism — plugins

This is the strongest piece of prior art available, because it is prior art in the exact
adjacent system.

herdr plugins are declared by a `herdr-plugin.toml` manifest. From
<https://herdr.dev/docs/plugins/>, the manifest supports `[[build]]`, `[[startup]]`,
`[[actions]]`, `[[events]]`, `[[panes]]` and `[[link_handlers]]` sections. The event form:

```toml
[[events]]
on = "worktree.created"
command = ["herdr", "workspace", "list"]
```

The full event catalogue is in the bundled schema (`herdr api schema --json`, `EventKind`):

```
workspace_created, workspace_updated, workspace_metadata_updated, workspace_closed,
workspace_renamed, workspace_moved, workspace_reordered, workspace_focused,
worktree_created, worktree_opened, worktree_removed,
tab_created, tab_closed, tab_renamed, tab_moved, tab_focused,
pane_created, pane_closed, pane_updated, pane_focused, pane_moved,
pane_output_changed, pane_exited, pane_agent_detected, pane_agent_status_changed,
layout_updated
```

`worktree_created` carries `{workspace: WorkspaceInfo, worktree: WorktreeInfo}`;
`worktree_removed` carries `{workspace_id, worktree, forced, workspace?}`. The
`WorkspaceWorktreeInfo` payload is `{repo_key, repo_name, repo_root, checkout_path,
is_linked_worktree}`.

**Design details worth stealing, all from <https://herdr.dev/docs/plugins/>:**

- **Commands are argv arrays, not shell strings** — "argv arrays, no shell expansion unless
  explicitly invoked". No quoting bugs, no accidental `$(…)`.
- **Data arrives by environment variable, not argv.** herdr injects `HERDR_SOCKET_PATH`,
  `HERDR_BIN_PATH`, `HERDR_ENV=1`, `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`,
  `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR`, `HERDR_PLUGIN_CONTEXT_JSON`,
  `HERDR_PLUGIN_EVENT`, `HERDR_PLUGIN_EVENT_JSON`, and `HERDR_WORKSPACE_ID` /
  `HERDR_TAB_ID` / `HERDR_PANE_ID` where available. The event payload is handed over as
  **JSON in one variable**, which sidesteps ever having to version a positional argv contract.
- **cwd is the plugin directory**, so relative script paths in the manifest work.
- **Config and state get separate directories**, and herdr does not manage their contents:
  "Herdr creates those directories … but it does not validate, sync, or delete their contents.
  The plugin owns the file format and lifecycle."
- **Call back in via `HERDR_BIN_PATH`**, not by re-resolving `herdr` on `PATH`: "That keeps
  plugins portable across Unix sockets and Windows named pipes."

**Trust posture, quoted in full** — this is the most explicit statement any of the tools
surveyed makes:

> "A plugin is ordinary code that runs on your machine. When you install or link one, its build
> and runtime commands run as your user, with your environment, and can call the full Herdr CLI."
>
> "Herdr validates the manifest and keeps each plugin's config and state in its own directory,
> but it does not review or sandbox what a plugin does. Third-party plugins come from their
> authors, not from Herdr, so they are yours to vet and run at your own discretion."

Note what herdr does *not* do: there is no per-repo plugin discovery. Plugins are installed by
explicit user command (`herdr plugin install owner/repo`, `herdr plugin link <path>`) and live
under `~/.config/herdr/plugins/`. Same structural choice as jj and gh (§3).

**There is already a working example on this machine.** `~/.config/herdr/plugins.json` registers
`lckn.worktree-labels` v0.3.0, "Name worktree Spaces `<repo> (<branch>)` and group them under
their repo", whose manifest at
`~/.config/herdr/plugins/worktree-labels/herdr-plugin.toml` binds `worktree.created` and
`worktree.opened` to a Python script, plus a `[[startup]]` sweep. Its script talks to
`HERDR_SOCKET_PATH` directly with `socket.AF_UNIX` and newline-delimited JSON.

**The limitation that matters:** herdr's `worktree.created` fires only when *herdr* creates the
worktree. A worktree created by `switch wt` is invisible to it — the plugin's own comments say
as much for the restore case ("Restored sessions replay no worktree events, so sweep once the
server is up"). So a herdr-side plugin cannot, on its own, adopt a `switch`-created worktree.
The push has to come from the `switch` side. That is the real argument for doing something here.

---

## 2. git worktree mechanics, and the hooks git already fires

### 2.1 What `worktree add` / `remove` do

git-worktree(1), § DETAILS:

> "Each linked worktree has a private sub-directory in the repository's `$GIT_DIR/worktrees`
> directory. … Within a linked worktree, `$GIT_DIR` is set to point to this private directory …
> and `$GIT_COMMON_DIR` is set to point back to the main worktree's `$GIT_DIR` … These settings
> are made in a `.git` file located at the top directory of the linked worktree."

> "**remove** — Remove a worktree. Only clean worktrees (no untracked files and no modification
> in tracked files) can be removed. Unclean worktrees or ones with submodules can be removed
> with `--force`. The main worktree cannot be removed."

In source, `remove_worktree()` → `delete_git_work_tree()` in
<https://github.com/git/git/blob/master/builtin/worktree.c> deletes the working tree directory
and the `$GIT_DIR/worktrees/<id>` admin dir. No hook call anywhere in that path.

### 2.2 `worktree.*` config — there are exactly two keys

Grepped the full git-config(1):

> "**worktree.guessRemote** — If no branch is specified and neither `-b` nor `-B` nor `--detach`
> is used, then `git worktree add` defaults to creating a new branch from HEAD. If
> `worktree.guessRemote` is set to true, `worktree add` tries to find a remote-tracking branch
> whose name uniquely matches the new branch name."

> "**worktree.useRelativePaths** — Link worktrees using relative paths (when "true") or absolute
> paths (when "false"). … Defaults to "false"."

Worktree-relevant keys outside that namespace: `gc.worktreePruneExpire`,
`extensions.worktreeConfig`, `extension.relativeWorktrees`, `checkout.defaultRemote`,
`core.bare`, `core.worktree`, `core.sparseCheckout`.

### 2.3 `git worktree add` DOES fire `post-checkout`

githooks(5), under `post-checkout`, final sentence:

> "It is also run after git-clone(1), unless the `--no-checkout` (`-n`) option is used. The first
> parameter given to the hook is the null-ref, the second the ref of the new HEAD and the flag is
> always 1. **Likewise for `git worktree add` unless `--no-checkout` is used.**"

git-worktree(1) itself never mentions hooks — the word does not appear in the man page or in
upstream `Documentation/git-worktree.adoc`. githooks(5) is the only doc that states this.

The source, <https://github.com/git/git/blob/master/builtin/worktree.c> (~L605-624):

```c
	/*
	 * Hook failure does not warrant worktree deletion, so run hook after
	 * is_junk is cleared, but do return appropriate code when hook fails.
	 */
	if (!ret && opts->checkout && !opts->orphan) {
		struct run_hooks_opt opt = RUN_HOOKS_OPT_INIT_FORCE_SERIAL;

		strvec_pushl(&opt.env, "GIT_DIR", "GIT_WORK_TREE", NULL);
		strvec_pushl(&opt.args,
			     oid_to_hex(null_oid(the_hash_algo)),
			     oid_to_hex(&commit->object.oid),
			     "1",
			     NULL);
		opt.dir = path;

		ret = run_hooks_opt(the_repository, "post-checkout", &opt);
	}
```

Three things fall out of that, all verified empirically on git 2.55.0:

- **cwd is the new worktree** (`opt.dir = path`), and `GIT_DIR` / `GIT_WORK_TREE` are explicitly
  *unset* in the hook's environment. So a hook can just use `$PWD` as the new checkout path.
- **Skipped on `--no-checkout` and on `--orphan`.** The `--orphan` exclusion is in the code only —
  it is not stated in githooks(5) or git-worktree(1).
- **Non-zero exit does not roll back the worktree**, but does become the command's exit status.
  Observed: a `post-checkout` exiting 42 produced `exit=42` from `git worktree add` with the
  worktree still created and registered. This matches githooks(5) on `post-checkout`
  generally: "This hook cannot affect the outcome of `git switch` or `git checkout`, other than
  that the hook's exit status becomes the exit status of these two commands."

`worktree add` also fires `reference-transaction` (several times) and `post-index-change`
(`argv=[1 0]`), incidentally, because it updates refs and writes an index.

### 2.4 `git worktree remove` fires NOTHING. Definitively.

There is no quote to give, because no git documentation mentions any hook for worktree removal.
That absence is the answer, and it is corroborated three ways:

1. **githooks(5)'s hook list is closed** — "The currently supported hooks are described below."
   The complete set is `applypatch-msg`, `pre-applypatch`, `post-applypatch`, `pre-commit`,
   `pre-merge-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `pre-rebase`,
   `post-checkout`, `post-merge`, `pre-push`, `pre-receive`, `update`, `proc-receive`,
   `post-receive`, `post-update`, `reference-transaction`, `push-to-checkout`, `pre-auto-gc`,
   `post-rewrite`, `sendemail-validate`, `fsmonitor-watchman`, `p4-changelist`,
   `p4-prepare-changelist`, `p4-post-changelist`, `p4-pre-submit`, `post-index-change`.
   `git worktree add` appears exactly once (under `post-checkout`); `git worktree remove`,
   `move`, `prune` and `lock` appear zero times.
2. **Source** — `builtin/worktree.c` has exactly one hook call site, the `post-checkout` above.
3. **Empirically** — with `post-checkout`, `post-merge`, `post-index-change`, `pre-auto-gc` and
   `reference-transaction` all installed and logging, `git worktree remove` produced an entirely
   empty hook log. Not even `reference-transaction`, because removal touches no refs.

**This asymmetry is the single most decision-relevant fact in this document.** Worktree creation
already has a seam; worktree removal has none, and never will unless git adds one.

### 2.5 Hooks are shared across worktrees

gitrepository-layout(5), `hooks` entry:

> "This directory is ignored if `$GIT_COMMON_DIR` is set and `$GIT_COMMON_DIR/hooks` will be used
> instead."

Verified from inside a linked worktree on git 2.55.0: `git rev-parse --git-path hooks` returns
the *main* repo's `.git/hooks`, not the worktree's private admin dir. One hooks directory serves
every worktree.

Per-worktree hooks are only reachable via `extensions.worktreeConfig` plus a `core.hooksPath` in
`config.worktree`. git-worktree(1) documents the mechanism but its explicit list of
"configuration that you do not want to share" is only `core.worktree`, `core.bare`,
`core.sparseCheckout`. **No primary source blesses per-worktree `core.hooksPath`.**

---

## 3. Prior art: how comparable tools model user-supplied hooks

### 3.1 git

**Location.** `$GIT_DIR/hooks` by default, overridden by `core.hooksPath`. From git-config(1):

> "By default Git will look for your hooks in the `$GIT_DIR/hooks` directory. Set this to
> different path, e.g. `/etc/git/hooks`, and Git will try to find your hooks in that directory …
> This configuration variable is useful in cases where you'd like to centrally configure your Git
> hooks instead of configuring them on a per-repository basis … You can also disable all hooks
> entirely by setting `core.hooksPath` to `/dev/null`."

`core.hooksPath` is an ordinary config key, so `--global` works. **It replaces rather than
merges** — note "instead of in `$GIT_DIR/hooks/pre-receive`", and the `/dev/null` trick only
works because nothing falls back. Verified on git 2.55.0: with a global `core.hooksPath` set and
a repo-local `.git/hooks/post-checkout` present, only the global one ran; and
`git hook run post-merge` errored with "cannot find a hook named post-merge" rather than falling
back to the repo hook.

**The composing alternative** is config hooks, git-hook(1)
(<https://github.com/git/git/blob/master/Documentation/git-hook.adoc>):

> "`[hook "linter"] event = pre-commit; command = ~/bin/linter --cpp20` — In this example,
> `[hook "linter"]` represents one script … which can be shared by many repos, and even by many
> hook events, if appropriate."

> "Commands are run in the order Git encounters their associated `hook.<friendly-name>.event`
> configs during the configuration parse … Although multiple `hook.linter.event` configs can be
> added, only one `hook.linter.command` event is valid — Git uses "last-one-wins"."

Verified on git 2.55.0 that a global config hook runs **in addition to** the hookdir hook, unlike
`core.hooksPath`, which stomps it. For a user-global "open this in herdr" hook that must not
break anyone's existing repo hooks, this is the correct mechanism.

**Trust model.** The definitive statement is git(1) § SECURITY — note that git-clone(1) itself is
silent on hooks; the word does not appear in it:

> "Some configuration options and hook files may cause Git to run arbitrary shell commands.
> **Because configuration and hooks are not copied using `git clone`, it is generally safe to
> clone remote repositories with untrusted content**, inspect them with `git log`, and so on.
>
> However, **it is not safe to run Git commands in a `.git` directory (or the working tree that
> surrounds it) when that `.git` directory itself comes from an untrusted source. The commands in
> its config and hooks are executed in the usual way.**
>
> By default, Git will refuse to run when the repository is owned by someone other than the user
> running the command. See the entry for `safe.directory` in git-config(1)…
>
> **If you have an untrusted `.git` directory, you should first clone it with
> `git clone --no-local` to obtain a clean copy.**"

Two supporting facts: hooks that arrive in a fresh clone come from the *template* directory, and
git-init(1) says "The sample hooks are all disabled by default. To enable one of the sample hooks
rename it by removing its `.sample` suffix." And githooks(5): "Hooks that don't have the
executable bit set are ignored." Verified empirically — cloning a repo with an executable
`post-checkout` produced a clone containing only `*.sample` files, and the hook did not run.

**So git's whole security story is: the dangerous thing is never checked in.** Hooks live in
`.git/`, which does not travel.

**Data passing.** githooks(5): "Hooks can get their arguments via the environment, command-line
arguments, and stdin." Concretely — `post-checkout` gets three argv (prev HEAD, new HEAD, branch
flag); `pre-push` gets two argv plus N lines on stdin; `reference-transaction` gets one argv
(`prepared`/`committed`/`aborted`) plus lines on stdin; `pre-receive` gets push options via
`GIT_PUSH_OPTION_0…N` env vars.

**Failure semantics** are per-hook and explicitly documented each time. Advisory: `post-commit`,
`post-merge`, `post-receive`, `post-update`, `post-applypatch` ("cannot affect the outcome of…").
Fatal: `pre-commit`, `commit-msg`, `pre-push`, `pre-receive`, `update`, `sendemail-validate`
("Exiting with a non-zero status causes … to abort"). `reference-transaction` is a hybrid: "The
exit status of the hook is ignored for any state except for the "prepared" state." `post-checkout`
is the odd one, and the relevant one here: the operation is never undone, but the hook's exit
status becomes the command's.

### 3.2 jj (jujutsu) — the closest analogue, and the strongest precedent

**jj has no hooks.** From `docs/git-compatibility.md`
(<https://github.com/jj-vcs/jj/blob/main/docs/git-compatibility.md>):

> "**Hooks: No.** There's [#405](https://github.com/jj-vcs/jj/issues/405) specifically for
> providing the checks from <https://pre-commit.com>."

The canonical discussion is <https://github.com/jj-vcs/jj/discussions/403>, where maintainer
@martinvonz wrote "There's no support for git hooks yet" and "I've planned to eventually add
native (i.e. not Git-specific) hooks." Still unimplemented as of this research. The `jj run`
design doc is explicit about avoiding git's model
(<https://docs.jj-vcs.dev/latest/design/run/>):

> "In a discussion on discord about the git-hook model, there was consensus about not repeating
> their mistakes."

**What jj offers instead.** `[aliases]` (user config only); `jj util exec` as the arbitrary-code
escape hatch, carrying a `!!! warning` block in `docs/config.md`:

> "The following technique just provides a convenient syntax for running arbitrary code on your
> system. Using it irresponsibly may cause damage ranging from breaking the behavior of `jj undo`
> to wiping your file system. Exercise the same amount of caution while writing these aliases as
> you would when typing commands into the terminal!"

And `[fix.tools]`, the closest thing to a structured hook: `command` is an argv array with
`$root` / `$path` interpolation, file content arrives **on stdin**, output comes back on stdout,
and a non-zero exit means the output is discarded and the file left alone.

**The config-location finding, quoted verbatim** from
<https://github.com/jj-vcs/jj/blob/main/docs/config.md>:

> "- The repo settings. These can be edited with `jj config edit --repo`, or found with
> `jj config path --repo`. **For security reasons, they are not located inside the repo.**
> - The workspace settings. These can be edited with `jj config edit --workspace`, or found with
> `jj config path --workspace`. **For security reasons, they are not located inside the
> workspace.**"

This is the single most transferable piece of prior art. jj *does* offer per-repo settings — it
just puts the file in `.jj/` metadata, addressable via `jj config path --repo`, so it never
travels with a clone. Per-repo configurability without inheriting a stranger's shell script.

### 3.3 gh CLI

**Aliases are user-global only, and can run shell.** From `gh alias set --help` /
<https://cli.github.com/manual/gh_alias_set>:

> "If the expansion starts with `!` or if `--shell` was given, the expansion is a shell expression
> that will be evaluated through the `sh` interpreter when the alias is invoked."

Stored as an `aliases:` map in the single `config.yml` under `ConfigDir()`. Resolution, from
<https://github.com/cli/go-gh/blob/trunk/pkg/config/config.go>: `GH_CONFIG_DIR`, then
`XDG_CONFIG_HOME/gh`, then `AppData/GitHub CLI` on Windows, then `~/.config/gh`. **There is no
directory walk and no repo-local lookup anywhere in that path.**

**Extensions carry an explicit, docs-only trust warning.** From `gh extension --help` /
<https://cli.github.com/manual/gh_extension>:

> "**Extensions are not verified, signed, or endorsed by GitHub. When you install or upgrade an
> extension, you are trusting its publisher. It is your responsibility to review the source and
> provenance of any extension before use.**"

No interactive prompt; `--pin` (tag or commit) is the only mitigation offered. Data passing is
pure argv: "All arguments passed to the `gh <extname>` invocation will be forwarded to the
`gh-<extname>` executable."

**Does gh read repo-checked-in config that can execute code? No.** Its only sources are
`config.yml` / `hosts.yml` in `ConfigDir()`, environment variables, and the keyring. It reads the
local git repo for remote and branch context only.

### 3.4 cargo — the counterexample

Cargo executes repo-checked-in code, twice over, with **no prompt, no allowlist, no sandbox**.

`build.rs`, from <https://doc.rust-lang.org/cargo/reference/build-scripts.html>:

> "Placing a file named `build.rs` in the root of a package will cause Cargo to compile that
> script and execute it just before building the package."
>
> "When the build script is run, there are a number of inputs to the build script, all passed in
> the form of environment variables."
>
> "In addition to environment variables, the build script's current directory is the root
> directory of the build script's package."
>
> "The script may communicate with Cargo by printing specially formatted commands prefixed with
> `cargo::` to stdout."

That page contains **no** statement about security, sandboxing, trust or review — searched and
absent. No argv at all: everything is env vars in, `cargo::`-prefixed directives out. Failure is
fatal.

`.cargo/config.toml`, from <https://doc.rust-lang.org/cargo/reference/config.html>:

> "It looks for configuration files in the current directory and all parent directories. If, for
> example, Cargo were invoked in `/projects/foo/bar/baz`, then the following configuration files
> would be probed for and unified in this order: `/projects/foo/bar/baz/.cargo/config.toml` …
> `$CARGO_HOME/config.toml`"
>
> "With this structure, you can specify configuration per-package, **and even possibly check it
> into version control.**"

The repo-local file *beats* your home config. Keys there that execute code: `[alias]`,
`build.rustc`, `build.rustc-wrapper` ("Sets a wrapper to execute instead of `rustc`"),
`target.<triple>.runner` ("executables for the target `<triple>` will be executed by invoking the
specified runner … This applies to `cargo run`, `cargo test` and `cargo bench`"),
`target.<triple>.linker`, `build.rustdoc`. That page carries **no security warning at all**.

The official acknowledgement lives elsewhere, in the Rust Project Goal
<https://rust-lang.github.io/rust-project-goals/2024h2/sandboxed-build-script.html>:

> "Build scripts in Cargo can do literally anything from network requests to executing arbitrary
> binaries."
>
> "This isn't deemed a security issue as it is 'by design'. Unfortunately, this 'by design' virtue
> relies on trust among developers within the community."

Sandboxing remains open and RFC-blocked: <https://github.com/rust-lang/cargo/issues/5720>
("Sandbox/jail build scripts", `S-needs-rfc`) and
<https://github.com/rust-lang/cargo/issues/13681> ("Build script allowlist mode", a proposal).

Cargo is not a model to copy here. Its build model genuinely needs per-package code (`-sys` crates
probing system libraries); `switch` has no such requirement.

### 3.5 direnv — the trust-on-first-use model

**Not verified empirically: direnv is not installed on this machine.** Everything below comes
from upstream man-page sources and Go source on `direnv/direnv` master.

The rationale, direnv(1) § USAGE
(<https://github.com/direnv/direnv/blob/master/man/direnv.1.md>):

> "On the next prompt you will notice that direnv complains about the `.envrc` being blocked.
> **This is the security mechanism to avoid loading new files automatically. Otherwise any git
> repo that you pull, or tar archive that you unpack, would be able to wipe your hard drive once
> you `cd` into it.**"

**What is trusted is content, not path.** From
<https://github.com/direnv/direnv/blob/master/internal/cmd/rc.go>, `fileHash()` computes
`sha256(absolute_path + "\n" + full_file_contents)` while `pathHash()` (used for *deny*) computes
`sha256(absolute_path + "\n")`. So an allow is bound to both the exact location and the exact
bytes; a deny is bound to location only. **Edit one byte of an allowed `.envrc` and it is blocked
again** — the recomputed hash simply stops matching, and `Load()` refuses with "%s is blocked. Run
`direnv allow` to approve its content". Fail-closed, no trust-on-first-use drift.

**Where the allow-list lives.** direnv(1) § FILES: "`$XDG_DATA_HOME/direnv/allow` — Records which
`.envrc` files have been `direnv allow`ed." Default `~/.local/share/direnv/allow/`. Each entry is
a file *named* for the hash whose contents are the path, written `0644` (with an explicit
`#nosec` waiver — these are deliberately not private). `direnv prune` clears stale entries.

**`whitelist` in direnv.toml** — the escape hatch, carrying its own warning
(<https://github.com/direnv/direnv/blob/master/man/direnv.toml.1.md>):

> "Specifying whitelist directives marks specific directory hierarchies or specific directories as
> "trusted" -- direnv will evaluate any matching .envrc files regardless of whether they have been
> specifically allowed. **This feature should be used with great care**, as anyone with the ability
> to write files to that directory (**including collaborators on VCS repositories**) will be able
> to execute arbitrary code on your computer."

`prefix` and `exact` both allow "regardless of contents or past usage of `direnv allow` or
`direnv deny`".

Two implementation notes found in source that the docs get wrong or overstate, worth knowing if
this model is copied: `Allowed()` checks the deny path *first*, so in the current implementation
an explicit `direnv deny` **does** override a whitelist entry, contradicting the doc quoted above;
and `WhitelistPrefix` uses a raw `strings.HasPrefix` with no path-boundary check, so a prefix of
`/home/user/code/project-a` also matches `/home/user/code/project-a-evil/.envrc`.

---

## 4. Synthesis for the design question

### 4.1 Where hook config should live, if we add one

The vote is 3–1 against repo-checked-in config, and the one dissenter regrets it.

| Tool | Repo-checked-in code execution? | Where the dangerous file lives |
|---|---|---|
| git | No — hooks are not cloned | `.git/hooks`, or `core.hooksPath`, or `hook.*.command` config |
| jj | **No, explicitly "for security reasons"** | `.jj/` metadata, via `jj config path --repo` |
| gh | No — `ConfigDir()` only, no directory walk | `~/.config/gh/config.yml` |
| herdr | No — plugins are installed by explicit command | `~/.config/herdr/plugins/` |
| cargo | **Yes** — `build.rs` and `.cargo/config.toml` | the tracked tree; sandbox still unshipped |

`switch` operates on repos cloned from elsewhere. It belongs firmly in the first group. If it
ever grows hooks, the config should be either user-global or in git config
(`git-switch.hook.*`, matching the existing `git-switch.keep` at `src/git.rs:482`) — and if
per-repo, then in `.git/config`, which does not travel with a clone, never in a tracked file.

Note that even git config is not entirely free of this: `git clone` does not copy `.git/config`
either, which is exactly why git(1) § SECURITY says cloning is safe. So `git-switch.hook.command`
in `.git/config` inherits git's own trust model for free, and is the cheapest correct answer.

### 4.2 Interface shape, if we add one

Converging on the same answer from three directions — herdr plugins, jj `[fix.tools]`, git config
hooks:

- **argv array, not a shell string.** herdr: "argv arrays, no shell expansion unless explicitly
  invoked". jj `[fix.tools]`: `command` is an array. Avoids the entire class of quoting bugs.
- **Data in environment variables, not positional argv.** git's `post-checkout` argv contract
  (`$1 $2 $3`) is frozen forever and unextendable; herdr's `HERDR_PLUGIN_EVENT_JSON` can grow
  fields without breaking anyone. If `switch` passes anything, a `GIT_SWITCH_*` env set — or one
  JSON blob — beats positional arguments.
- **cwd should be defined and documented.** git sets it to the new worktree for `post-checkout`
  on `worktree add`; herdr sets it to the plugin directory; cargo sets it to the package root.
- **Advisory failure, with a loud report.** `post-checkout` on `worktree add` never rolls back the
  worktree (the source comment: "Hook failure does not warrant worktree deletion"), and jj `fix`
  scopes failure to discarding one tool's output. Both argue the same thing for
  post-worktree-create: the checkout succeeded, so do not undo it — report the non-zero exit
  loudly and carry on. This matches how `src/app/wt.rs` already handles non-fatal trouble, e.g.
  the `update_in` failure at `wt.rs:48-54` and the stale-branch check at `wt.rs:78-86`, both of
  which print a `!` warning and continue.

### 4.3 The three options this research surfaces

**Option A — no change to `switch`; use a git `post-checkout` hook.**
`git worktree add` already fires `post-checkout` with cwd set to the new worktree (§2.3). A
user-global `hook.herdr.event = post-checkout` / `hook.herdr.command = …` (§3.1, git-hook(1),
composes rather than stomping) could run
`herdr worktree open --path "$PWD" --no-focus` today. Zero code in `switch`.
Costs: it fires on *every* `worktree add` in every repo, not just `switch`'s; it cannot
distinguish `switch wt` from a bare `git worktree add`; the hook's non-zero exit becomes
`git worktree add`'s exit status, which `switch` would surface as a worktree-add failure; and
**it does nothing for removal, because `git worktree remove` fires no hook at all** (§2.4).

**Option B — a narrow, purpose-built herdr integration in `switch`.**
Two calls at two known points: `herdr worktree open --path <path> --no-focus` after
`create_worktree` succeeds (`src/app/wt.rs:316-355`), and `herdr workspace close <id>` in
`run_rm` before the checkout goes (`src/app/wt.rs:138-206`), with the id looked up beforehand
via `herdr worktree list`'s `open_workspace_id` (§1.5). Guarded by
`HERDR_ENV=1` (herdr's own documented probe, per `herdr --skill`: "verify that this agent is
running inside a Herdr-managed pane: `test "${HERDR_ENV:-}" = 1`") plus `herdr` on `PATH`, and
advisory on failure.
Costs: `switch` grows a hard dependency on one third-party tool, which sits awkwardly against
CLAUDE.md's "Keep dependencies minimal" and against CONTEXT.md's framing of the domain as
"the small set of judgements it makes on the user's behalf". Also, herdr's protocol is
versioned (protocol 19 today) and its docs advise checking compatibility before depending on
behaviour.

**Option C — a general hook mechanism in `switch`.**
Covers the removal gap that git cannot, and keeps herdr out of the codebase. Shape per §4.2,
config per §4.1.
Costs: it is a whole feature — config surface, argv/env contract, failure policy, docs, tests —
in service of one user's one integration. Nothing in the research says a general mechanism is
needed to reach the stated goal; §1.4 shows two CLI calls reach it.

The honest reading is that Option A gets the create half for free but cannot do teardown, and
Options B and C differ mainly in whether the herdr knowledge lives in `switch`'s source or in
the user's config. The removal gap (§2.4) is what makes *some* `switch`-side change necessary
for the full round trip; it does not by itself argue for generality.

---

## 5. Where no primary source was found

Stated plainly rather than guessed:

- **herdr's third-level `--help` is broken on 0.8.0.** `herdr worktree create --help` prints the
  *top-level* help, not the subcommand's. The flag lists in §1.4 therefore come from the shipped
  zsh completions (`/opt/homebrew/Cellar/herdr/0.8.0/share/zsh/site-functions/_herdr`), the
  bundled JSON Schema, and <https://herdr.dev/docs/cli-reference/> — which agree with each other,
  but the binary's own nested help could not be used to confirm them.
- **herdr's full plugin event name list is not on the docs page.** <https://herdr.dev/docs/plugins/>
  shows `on = "worktree.created"` as an example but does not enumerate the valid values. The
  catalogue in §1.8 is taken from the `EventKind` enum in `herdr api schema --json`, which uses
  snake_case (`worktree_created`) while the manifest uses dotted form (`worktree.created`). The
  mapping between the two is inferred from the working plugin on this machine, not documented.
- **herdr's socket API has no documented authentication or trust model.** <https://herdr.dev/docs/socket-api/>
  says nothing about it; access control appears to rest on filesystem permissions of the Unix
  socket, but that is an inference, not a quote.
- **git-clone(1) says nothing about hooks.** The "hooks are not copied on clone" guarantee exists
  only in git(1) § SECURITY. Cite git(1), not git-clone(1).
- **git-worktree(1) says nothing about hooks either.** The `post-checkout`-on-`worktree add` fact
  is documented *only* in githooks(5).
- **The `--orphan` exclusion** for `post-checkout` on `worktree add` is in `builtin/worktree.c`
  but is stated in neither githooks(5) nor git-worktree(1).
- **Per-worktree `core.hooksPath`** via `extensions.worktreeConfig` is mechanically possible but
  endorsed by no git doc found.
- **`pre-rebase` exit-status semantics** are not spelled out in githooks(5).
- **direnv was not installed on this machine**, so §3.5 is docs-and-source only, with no runtime
  confirmation. Its deny-beats-whitelist ordering contradicts direnv.toml(1); the source was
  trusted over the prose, but the docs are unresolved on it.
- **jj was not installed either**; §3.2 is from the docs and source repo only.
- **jj's native hooks remain unimplemented.** Planned since 2022
  (<https://github.com/jj-vcs/jj/discussions/403>); no shipped design to copy.
