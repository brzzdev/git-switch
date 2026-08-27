//! Branch-specific commands. Navigation stays in the parent module because all
//! three verbs share it; branch removal lives here because its local and upstream
//! targets have a safety contract of their own.

use super::picker::{MultiItem, interactive_keys, multi_select};
use super::{Confirmation, confirm, interactive_term, removal};
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

pub fn run_rm(options: &RmOptions) -> AppResult<()> {
    let worktrees = git::worktree_list()?;
    let current = git::current_branch()?;
    let remote = git::current_remote(current.as_deref());
    let local = git::local_branches()?;
    if local.is_empty() {
        eprintln!("No local branches to remove.");
        return Ok(());
    }
    let upstream = if options.upstream {
        removal::UpstreamInterest::Requested
    } else if interactive_term().is_some() {
        removal::UpstreamInterest::Offer
    } else {
        removal::UpstreamInterest::None
    };
    let assessment = removal::assess(removal::Request::Branches(removal::BranchRequest::new(
        local,
        worktrees,
        current.as_deref(),
        &remote,
        upstream,
    )))?;
    let Some(choice) = select_local(&assessment, options)? else {
        return Ok(());
    };
    let pending = assessment.choose(choice)?;
    for notice in pending.notices() {
        eprintln!("{notice}");
    }
    let Some(upstream) = select_upstream(&pending, options)? else {
        return Ok(());
    };
    finish(pending, upstream)
}

fn select_local(
    assessment: &removal::Assessment,
    options: &RmOptions,
) -> AppResult<Option<removal::LocalChoice>> {
    if let Some(target) = options.target.as_deref() {
        let named = assessment.named(target)?;
        if options.force {
            return Ok(Some(removal::LocalChoice::forced(named.id())));
        }
        if named.warnings().is_empty() {
            return Ok(Some(removal::LocalChoice::named(named.id())));
        }
        if interactive_term().is_none() {
            return Err(Error::Unconfirmed(named.refusal().to_string()));
        }
        for warning in named.warnings() {
            eprintln!("{warning}");
        }
        return Ok(
            (confirm(named.question(), false)? == Confirmation::Accepted)
                .then(|| removal::LocalChoice::named(named.id())),
        );
    }

    let Some(keys) = interactive_keys() else {
        return Ok(None);
    };
    let items: Vec<MultiItem> = assessment
        .offers()
        .iter()
        .map(|offer| MultiItem {
            label: offer.label().to_string(),
            selected: offer.selected(),
            disabled: offer.disabled(),
        })
        .collect();
    let Some(selected) = multi_select(
        "Remove local branches (space to toggle, →/← all/none)",
        assessment.legend(),
        &items,
        keys,
    )?
    else {
        return Ok(None);
    };
    if selected.is_empty() {
        return Ok(None);
    }
    let ids = selected
        .into_iter()
        .map(|index| assessment.offers()[index].id())
        .collect();
    Ok(Some(if options.force {
        removal::LocalChoice::forced_picked(ids)
    } else {
        removal::LocalChoice::picked(ids)
    }))
}

fn select_upstream(
    pending: &removal::Pending,
    options: &RmOptions,
) -> AppResult<Option<removal::UpstreamChoice>> {
    if pending.upstream_offers().is_empty() {
        return Ok(Some(removal::UpstreamChoice::keep()));
    }
    if options.force && options.upstream {
        return Ok(Some(removal::UpstreamChoice::selected(
            pending
                .upstream_offers()
                .iter()
                .map(removal::UpstreamOffer::id)
                .collect(),
        )));
    }
    if options.target.is_some() {
        let offer = &pending.upstream_offers()[0];
        eprintln!("{}", offer.warning());
        if interactive_term().is_none() {
            return Err(Error::Unconfirmed(
                "upstream deletion was requested without a terminal; pass --force to confirm it"
                    .into(),
            ));
        }
        return Ok(match confirm(offer.question(), options.upstream)? {
            Confirmation::Accepted => Some(removal::UpstreamChoice::selected(vec![offer.id()])),
            Confirmation::Cancelled => None,
            Confirmation::Declined => Some(removal::UpstreamChoice::keep()),
        });
    }

    let Some(keys) = interactive_keys() else {
        if options.upstream {
            return Err(Error::Unconfirmed(
                "upstream deletion was requested without a terminal; pass --force to confirm it"
                    .into(),
            ));
        }
        return Ok(Some(removal::UpstreamChoice::keep()));
    };
    let items: Vec<MultiItem> = pending
        .upstream_offers()
        .iter()
        .map(|offer| MultiItem {
            label: offer.label().to_string(),
            selected: options.upstream,
            disabled: false,
        })
        .collect();
    let Some(selected) = multi_select(
        "Also delete upstream branches? (space to toggle, →/← all/none)",
        Some("Each selected row removes a shared upstream ref"),
        &items,
        keys,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(removal::UpstreamChoice::selected(
        selected
            .into_iter()
            .map(|index| pending.upstream_offers()[index].id())
            .collect(),
    )))
}

fn finish(pending: removal::Pending, upstream: removal::UpstreamChoice) -> AppResult<()> {
    let outcome = pending.finish(upstream)?;
    for line in outcome.lines() {
        eprintln!("{line}");
    }
    if outcome.failed() {
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
