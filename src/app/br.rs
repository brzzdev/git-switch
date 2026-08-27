//! Branch-specific commands. Navigation stays in the parent module because all
//! three verbs share it; branch removal lives here because its local and upstream
//! targets have a safety contract of their own.

use std::collections::{HashMap, HashSet};

use console::style;

use super::picker::{MultiItem, align_labels, interactive_keys, multi_select};
use super::{
    Risk, confirm, display_path, interactive_term, marker, removal, reporting, risk_legend,
};
use crate::{AppResult, Error, git};

spelled! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Subverb {
        Rm = "rm",
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct RmOptions {
    target: Option<String>,
    force: bool,
    upstream: bool,
}

impl RmOptions {
    pub fn parse(args: &[String]) -> AppResult<Self> {
        let mut options = Self::default();
        for arg in args {
            match arg.as_str() {
                "--force" if !options.force => options.force = true,
                "--upstream" if !options.upstream => options.upstream = true,
                "--force" | "--upstream" => {
                    return Err(Error::BrRmUsage(format!("duplicate option '{arg}'")));
                }
                _ if arg.starts_with('-') => {
                    return Err(Error::BrRmUsage(format!("unknown option '{arg}'")));
                }
                _ if options.target.is_some() => {
                    return Err(Error::BrRmUsage(format!("unexpected extra target '{arg}'")));
                }
                _ => options.target = Some(arg.clone()),
            }
        }
        Ok(options)
    }
}

#[derive(Clone)]
struct BranchRow {
    name: String,
    holder: Option<git::Worktree>,
    kept: bool,
    risk: Risk,
}

impl BranchRow {
    fn disabled(&self) -> bool {
        self.holder.is_some() || self.kept
    }
}

struct UpstreamChoice {
    row: usize,
    upstream: git::RemoteBranch,
}

#[derive(Clone, Copy)]
struct UpstreamSelection {
    target_was_named: bool,
    requested: bool,
    interactive: bool,
}

#[derive(Default)]
struct UpstreamPlan {
    selected: HashMap<usize, git::RemoteBranch>,
    failures: HashMap<usize, String>,
}

pub fn run_rm(options: &RmOptions) -> AppResult<()> {
    let worktrees = git::worktree_list()?;
    let main = worktrees
        .iter()
        .find(|worktree| worktree.is_main)
        .ok_or_else(|| Error::Git {
            command: "worktree list".into(),
            message: "main worktree not found".into(),
        })?;
    let current = git::current_branch()?;
    let remote = git::current_remote(current.as_deref());
    let local = git::local_branches()?;
    if local.is_empty() {
        eprintln!("No local branches to remove.");
        return Ok(());
    }

    let kept: HashSet<String> = git::pinned_branches(&remote).into_iter().collect();
    let unmerged = git::unmerged_branches(Some(&main.path)).unwrap_or_default();
    let rows: Vec<BranchRow> = local
        .into_iter()
        .map(|name| BranchRow {
            holder: git::worktree_for_branch(&worktrees, &name),
            kept: kept.contains(&name),
            risk: Risk {
                dirty: false,
                unmerged: unmerged.get(&name).copied(),
            },
            name,
        })
        .collect();

    let selected = select_local(&rows, current.as_deref(), main, options)?;
    if selected.is_empty() {
        return Ok(());
    }
    let Some(upstreams) = select_upstreams(&rows, &selected, options)? else {
        return Ok(());
    };

    let mut failed = false;
    let mut steps = removal::GitSteps::at_main(Some(&main.path));
    for &row_index in &selected {
        failed |= !remove_pair(
            row_index,
            &rows[row_index],
            &upstreams,
            options.force,
            &mut steps,
        );
    }

    if failed {
        Err(Error::RemovalFailed)
    } else {
        Ok(())
    }
}

fn remove_pair(
    row_index: usize,
    row: &BranchRow,
    upstreams: &UpstreamPlan,
    force: bool,
    steps: &mut removal::GitSteps,
) -> bool {
    if let Some(error) = upstreams.failures.get(&row_index) {
        eprintln!(
            "{} could not prepare upstream removal for {}: {error}; kept the local branch",
            style("!").yellow().bold(),
            row.name,
        );
        return false;
    }
    let license = if force {
        removal::License::forced()
    } else {
        removal::License::shown(row.risk)
    };
    let report = match removal::remove(removal::Target::Branch { name: &row.name }, &license, steps)
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "{} could not remove {}: {error}",
                style("!").yellow().bold(),
                row.name,
            );
            return false;
        }
    };
    for line in reporting::removal_outcome(&report) {
        eprintln!("{line}");
    }

    let local_deleted = report.branch_removed();
    let Some(upstream) = upstreams.selected.get(&row_index) else {
        return local_deleted;
    };
    let local_ref = format!("refs/heads/{}", row.name);
    if !local_deleted || git::resolve(None, &local_ref).is_some() {
        eprintln!("{}", reporting::upstream_kept_local(upstream));
        return false;
    }
    let outcome = match git::delete_remote_branch(upstream) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!(
                "{} could not delete upstream {}/{}: {error}",
                style("!").yellow().bold(),
                upstream.remote,
                upstream.branch,
            );
            return false;
        }
    };
    eprintln!("{}", reporting::upstream_outcome(upstream, &outcome));
    matches!(
        outcome,
        git::RemoteBranchDeleteOutcome::Deleted | git::RemoteBranchDeleteOutcome::AlreadyAbsent
    )
}

pub fn run_rm_complete() -> AppResult<()> {
    let mut out = String::new();
    for branch in git::local_branches()? {
        out.push_str(&branch);
        out.push('\n');
    }
    print!("{out}");
    Ok(())
}

fn select_local(
    rows: &[BranchRow],
    current: Option<&str>,
    main: &git::Worktree,
    options: &RmOptions,
) -> AppResult<Vec<usize>> {
    let Some(target) = options.target.as_deref() else {
        let Some(keys) = interactive_keys() else {
            // Non-interactive (piped/CI): we can't prompt, so remove nothing rather
            // than blocking on key input.
            return Ok(Vec::new());
        };
        let disabled: Vec<bool> = rows.iter().map(BranchRow::disabled).collect();
        let labels = align_labels(
            &rows
                .iter()
                .map(|row| {
                    let annotation = row_annotation(row, current);
                    let marker = if row.disabled() {
                        String::new()
                    } else {
                        marker::markers(row.risk)
                    };
                    let detail = [annotation, marker]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    (row.name.clone(), detail)
                })
                .collect::<Vec<_>>(),
        );
        let items: Vec<MultiItem> = labels
            .into_iter()
            .zip(disabled)
            .map(|(label, disabled)| MultiItem {
                label,
                selected: false,
                disabled,
            })
            .collect();
        let visible_risks: Vec<Risk> = rows
            .iter()
            .filter(|row| !row.disabled())
            .map(|row| row.risk)
            .collect();
        let legend = risk_legend(&visible_risks);
        return Ok(multi_select(
            "Remove local branches (space to toggle, →/← all/none)",
            legend.as_deref(),
            &items,
            keys,
        )?
        .unwrap_or_default());
    };

    let index = rows
        .iter()
        .position(|row| row.name == target)
        .ok_or_else(|| Error::LocalBranchNotFound {
            branch: target.to_string(),
        })?;
    if let Some(holder) = &rows[index].holder {
        return Err(held_error(target, holder));
    }
    if confirm_local(&rows[index], main, options.force)? {
        Ok(vec![index])
    } else {
        Ok(Vec::new())
    }
}

fn row_annotation(row: &BranchRow, current: Option<&str>) -> String {
    if current == Some(&row.name) {
        return "current".into();
    }
    if let Some(holder) = &row.holder {
        let path = display_path(&holder.path);
        return if holder.is_main {
            format!("main worktree at {path}")
        } else if holder.prunable {
            format!("missing worktree at {path}; use wt rm")
        } else {
            format!("{path}; use wt rm")
        };
    }
    if row.kept {
        return "kept".into();
    }
    String::new()
}

fn held_error(branch: &str, holder: &git::Worktree) -> Error {
    let hint = if holder.is_main {
        "check out another branch in the main worktree first".into()
    } else {
        format!(
            "remove that worktree with `perch wt rm {}`",
            super::shell_quote(branch)
        )
    };
    Error::HeldForRemoval {
        branch: branch.to_string(),
        path: display_path(&holder.path),
        hint,
    }
}

fn confirm_local(row: &BranchRow, main: &git::Worktree, force: bool) -> AppResult<bool> {
    if force || !row.risk.any() {
        return Ok(true);
    }
    let warnings = reporting::warnings(row.risk, &row.name, &main.path);
    if interactive_term().is_none() {
        let reason = reporting::describe(row.risk, &row.name, &main.path).join("; ");
        return Err(Error::Unconfirmed(format!(
            "{reason}; not removing. Rerun in a terminal to confirm, or pass --force."
        )));
    }
    for warning in warnings {
        eprintln!("{warning}");
    }
    confirm(&format!("Delete {} anyway?", row.name), false)
}

fn select_upstreams(
    rows: &[BranchRow],
    selected: &[usize],
    options: &RmOptions,
) -> AppResult<Option<UpstreamPlan>> {
    let selection = UpstreamSelection {
        target_was_named: options.target.is_some(),
        requested: options.upstream,
        interactive: interactive_term().is_some(),
    };
    if !selection.requested && !selection.interactive {
        return Ok(Some(UpstreamPlan::default()));
    }

    let mut choices = Vec::new();
    let mut failures = HashMap::new();
    for &row in selected {
        let branch = &rows[row].name;
        match upstream_choice(row, branch, selection) {
            Ok(Some(upstream)) => choices.push(upstream),
            Ok(None) => {}
            Err(error) if selection.target_was_named => return Err(error),
            Err(error) => {
                failures.insert(row, error.to_string());
            }
        }
    }
    if choices.is_empty() {
        return Ok(Some(UpstreamPlan {
            selected: HashMap::new(),
            failures,
        }));
    }

    if options.force && selection.requested {
        return Ok(Some(plan_all(choices, failures)));
    }
    if selection.target_was_named {
        return select_named_upstream(choices, failures, selection).map(Some);
    }

    select_upstream_rows(choices, failures, selection.requested)
}

fn plan_all(choices: Vec<UpstreamChoice>, failures: HashMap<usize, String>) -> UpstreamPlan {
    UpstreamPlan {
        selected: choices
            .into_iter()
            .map(|choice| (choice.row, choice.upstream))
            .collect(),
        failures,
    }
}

fn select_named_upstream(
    choices: Vec<UpstreamChoice>,
    failures: HashMap<usize, String>,
    selection: UpstreamSelection,
) -> AppResult<UpstreamPlan> {
    let choice = &choices[0];
    eprintln!("{}", reporting::upstream_warning(&choice.upstream));
    if !selection.interactive {
        return Err(Error::Unconfirmed(
            "upstream deletion was requested without a terminal; pass --force to confirm it".into(),
        ));
    }
    if confirm(
        &format!(
            "Delete upstream {}/{} too?",
            choice.upstream.remote, choice.upstream.branch
        ),
        selection.requested,
    )? {
        Ok(plan_all(choices, failures))
    } else {
        Ok(UpstreamPlan {
            selected: HashMap::new(),
            failures,
        })
    }
}

fn select_upstream_rows(
    choices: Vec<UpstreamChoice>,
    failures: HashMap<usize, String>,
    requested: bool,
) -> AppResult<Option<UpstreamPlan>> {
    let Some(keys) = interactive_keys() else {
        // Non-interactive (piped/CI): we can't prompt, so refuse an explicit
        // request or keep every upstream rather than blocking on key input.
        return if requested {
            Err(Error::Unconfirmed(
                "upstream deletion was requested without a terminal; pass --force to confirm it"
                    .into(),
            ))
        } else {
            Ok(Some(UpstreamPlan {
                selected: HashMap::new(),
                failures,
            }))
        };
    };
    let items: Vec<MultiItem> = choices
        .iter()
        .map(|choice| MultiItem {
            label: format!("{}/{}", choice.upstream.remote, choice.upstream.branch),
            selected: requested,
            disabled: false,
        })
        .collect();
    let Some(picked) = multi_select(
        "Also delete upstream branches? (space to toggle, →/← all/none)",
        Some("Each selected row removes a shared upstream ref"),
        &items,
        keys,
    )?
    else {
        return Ok(None);
    };
    let picked: HashSet<usize> = picked.into_iter().collect();
    Ok(Some(UpstreamPlan {
        selected: choices
            .into_iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                picked
                    .contains(&index)
                    .then_some((choice.row, choice.upstream))
            })
            .collect(),
        failures,
    }))
}

fn upstream_choice(
    row: usize,
    branch: &str,
    selection: UpstreamSelection,
) -> AppResult<Option<UpstreamChoice>> {
    let upstream = match git::same_named_upstream(branch) {
        Ok(upstream) => upstream,
        Err(error) if selection.requested => return Err(error),
        Err(error) => {
            eprintln!(
                "{} could not read the upstream of {branch}: {error}; offering local removal only",
                style("!").yellow().bold(),
            );
            return Ok(None);
        }
    };
    let Some(upstream) = upstream else {
        if selection.target_was_named && selection.requested {
            return Err(Error::NoRemovableUpstream {
                branch: branch.to_string(),
                reason: "it has no explicit same-named upstream".into(),
            });
        }
        return Ok(None);
    };
    match git::inspect_upstream(&upstream) {
        Ok(git::UpstreamInspection::Removable(upstream)) => {
            Ok(Some(UpstreamChoice { row, upstream }))
        }
        Ok(git::UpstreamInspection::Absent(upstream)) => {
            if selection.target_was_named {
                eprintln!(
                    "{} upstream {}/{} is already absent",
                    style("!").yellow().bold(),
                    upstream.remote,
                    upstream.branch,
                );
            }
            Ok(None)
        }
        Ok(git::UpstreamInspection::Default(upstream)) => {
            if selection.target_was_named && selection.requested {
                return Err(Error::NoRemovableUpstream {
                    branch: branch.to_string(),
                    reason: format!(
                        "{}/{} is the remote's default branch",
                        upstream.remote, upstream.branch
                    ),
                });
            }
            Ok(None)
        }
        Ok(git::UpstreamInspection::DefaultUnknown(upstream)) => {
            let reason = format!(
                "could not establish the default branch of {}",
                upstream.remote
            );
            if selection.requested {
                return Err(Error::NoRemovableUpstream {
                    branch: branch.to_string(),
                    reason,
                });
            }
            eprintln!(
                "{} {reason}; offering local removal only",
                style("!").yellow().bold()
            );
            Ok(None)
        }
        Err(error) if selection.requested => Err(error),
        Err(error) => {
            eprintln!(
                "{} could not inspect the upstream of {branch}: {error}; offering local removal only",
                style("!").yellow().bold(),
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn rm_options_accept_target_and_flags_in_any_order() {
        let parsed = RmOptions::parse(&strings(&["--upstream", "topic", "--force"]))
            .expect("valid options should parse");
        assert_eq!(
            parsed,
            RmOptions {
                target: Some("topic".into()),
                force: true,
                upstream: true,
            }
        );
    }

    #[test]
    fn rm_options_reject_unknown_flags() {
        let error = RmOptions::parse(&strings(&["--remote", "topic"]))
            .expect_err("unknown option should fail");
        assert!(error.to_string().contains("unknown option '--remote'"));
    }

    #[test]
    fn rm_options_reject_extra_targets() {
        let error =
            RmOptions::parse(&strings(&["one", "two"])).expect_err("extra target should fail");
        assert!(error.to_string().contains("unexpected extra target 'two'"));
    }
}
