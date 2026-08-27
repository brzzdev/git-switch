//! Branch-specific commands. Navigation stays in the parent module because all
//! three verbs share it; branch removal lives here because its local and upstream
//! targets have a safety contract of their own.

use std::collections::{HashMap, HashSet};

use console::style;

use super::picker::{align_labels, interactive_keys, multi_select};
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
                "-f" | "--force" if !options.force => options.force = true,
                "--upstream" if !options.upstream => options.upstream = true,
                "-f" | "--force" | "--upstream" => {
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
    let upstreams: HashMap<usize, git::RemoteBranch> = select_upstreams(&rows, &selected, options)?
        .into_iter()
        .map(|choice| (choice.row, choice.upstream))
        .collect();

    let mut failed = false;
    let mut steps = removal::GitSteps::at_main(Some(&main.path));
    for &row_index in &selected {
        let row = &rows[row_index];
        let license = if options.force {
            removal::License::forced()
        } else {
            removal::License::shown(row.risk)
        };
        let report = removal::remove(
            removal::Target::Branch { name: &row.name },
            &license,
            &mut steps,
        )?;
        for line in reporting::removal_outcome(&report) {
            eprintln!("{line}");
        }

        let local_deleted = matches!(
            report.branch,
            Some(
                git::BranchDeleteOutcome::Deleted
                    | git::BranchDeleteOutcome::DeletedLeavingConfig(_)
                    | git::BranchDeleteOutcome::DeletedConfigUnverified(_)
            )
        );
        if !local_deleted {
            failed = true;
        }

        let Some(upstream) = upstreams.get(&row_index) else {
            continue;
        };
        let local_ref = format!("refs/heads/{}", row.name);
        if !local_deleted || git::resolve(None, &local_ref).is_some() {
            eprintln!("{}", reporting::upstream_kept_local(upstream));
            failed = true;
            continue;
        }
        let outcome = git::delete_remote_branch(upstream)?;
        eprintln!("{}", reporting::upstream_outcome(upstream, &outcome));
        if !matches!(
            outcome,
            git::RemoteBranchDeleteOutcome::Deleted | git::RemoteBranchDeleteOutcome::AlreadyAbsent
        ) {
            failed = true;
        }
    }

    if failed {
        Err(Error::RemovalFailed)
    } else {
        Ok(())
    }
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
        let defaults = vec![false; rows.len()];
        let visible_risks: Vec<Risk> = rows
            .iter()
            .filter(|row| !row.disabled())
            .map(|row| row.risk)
            .collect();
        let legend = risk_legend(&visible_risks);
        return multi_select(
            "Remove local branches (space to toggle, →/← all/none)",
            legend.as_deref(),
            &labels,
            &defaults,
            &disabled,
            keys,
        );
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
) -> AppResult<Vec<UpstreamChoice>> {
    let interactive = interactive_term().is_some();
    if !options.upstream && !interactive {
        return Ok(Vec::new());
    }

    let named = options.target.is_some();
    let mut choices = Vec::new();
    for &row in selected {
        let branch = &rows[row].name;
        if let Some(upstream) = upstream_choice(row, branch, named, options.upstream)? {
            choices.push(upstream);
        }
    }
    if choices.is_empty() {
        return Ok(Vec::new());
    }

    if options.force && options.upstream {
        return Ok(choices);
    }
    if named {
        let choice = &choices[0];
        eprintln!("{}", reporting::upstream_warning(&choice.upstream));
        if !interactive {
            return Err(Error::Unconfirmed(
                "upstream deletion was requested without a terminal; pass --force to confirm it"
                    .into(),
            ));
        }
        return if confirm(
            &format!(
                "Delete upstream {}/{} too?",
                choice.upstream.remote, choice.upstream.branch
            ),
            options.upstream,
        )? {
            Ok(choices)
        } else {
            Ok(Vec::new())
        };
    }

    let Some(keys) = interactive_keys() else {
        return if options.upstream {
            Err(Error::Unconfirmed(
                "upstream deletion was requested without a terminal; pass --force to confirm it"
                    .into(),
            ))
        } else {
            Ok(Vec::new())
        };
    };
    let labels: Vec<String> = choices
        .iter()
        .map(|choice| format!("{}/{}", choice.upstream.remote, choice.upstream.branch))
        .collect();
    let defaults = vec![options.upstream; choices.len()];
    let disabled = vec![false; choices.len()];
    let picked = multi_select(
        "Also delete upstream branches? (space to toggle, →/← all/none)",
        Some("Each selected row removes a shared upstream ref"),
        &labels,
        &defaults,
        &disabled,
        keys,
    )?;
    let picked: HashSet<usize> = picked.into_iter().collect();
    Ok(choices
        .into_iter()
        .enumerate()
        .filter_map(|(index, choice)| picked.contains(&index).then_some(choice))
        .collect())
}

fn upstream_choice(
    row: usize,
    branch: &str,
    target_was_named: bool,
    upstream_requested: bool,
) -> AppResult<Option<UpstreamChoice>> {
    let Some(upstream) = git::same_named_upstream(branch)? else {
        if target_was_named && upstream_requested {
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
            if target_was_named {
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
            if target_was_named && upstream_requested {
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
            if upstream_requested {
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
        Err(error) if upstream_requested => Err(error),
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
        let parsed = RmOptions::parse(&strings(&["--upstream", "topic", "-f"]))
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
