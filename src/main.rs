use std::process;

use perch::app::complete::{self, Position};
use perch::app::{
    Verb,
    wt::{ShellHandoff, Subverb},
};

fn main() {
    let _ = ctrlc::set_handler(|| {
        let _ = console::Term::stderr().show_cursor();
        process::exit(130);
    });

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = dispatch(&args);

    if let Err(e) = result {
        if e.is_interrupt() {
            let _ = console::Term::stderr().show_cursor();
            process::exit(130);
        }
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn dispatch(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("perch {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // Read before the branch name, and prints the branches a bare `perch`
        // will accept rather than going to one. See `dispatch_wt_rm` for why
        // every position answers this as a flag and not a subcommand.
        Some("--complete") => complete::run(Position::Bare),
        // `--` ends option/subcommand parsing: everything after is a branch,
        // so a branch literally named `br`/`wt` stays reachable.
        Some("--") => escaped(args.get(1).map(String::as_str), perch::app::run),
        // Parsed rather than matched word by word, so the verbs exist in one
        // place: this match is exhaustive over them, and the completions
        // subtract what it reads. See `perch::app::complete`.
        Some(name) => match Verb::parse(name) {
            Some(Verb::Here) => dispatch_br(&args[1..]),
            Some(Verb::Worktree) => dispatch_wt(&args[1..]),
            // `parse` never answers `Go`, which has no spelling — so a word it
            // rejects is a branch name, which is the whole of what `Go` means.
            Some(Verb::Go) | None => perch::app::run(Some(name)),
        },
        None => perch::app::run(None),
    }
}

fn dispatch_br(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_br_help();
            Ok(())
        }
        Some("--complete") => complete::run(Position::Br),
        Some("--") => escaped(args.get(1).map(String::as_str), perch::app::run_br),
        name => perch::app::run_br(name),
    }
}

fn dispatch_wt(args: &[String]) -> perch::AppResult<()> {
    let mut shell_handoff = ShellHandoff::Emit;
    let mut reads_options = true;
    let mut remaining_args = Vec::with_capacity(args.len());
    for arg in args {
        if reads_options && arg == "--" {
            reads_options = false;
            remaining_args.push(arg.as_str());
        } else if reads_options && arg == "--no-switch" {
            shell_handoff = ShellHandoff::Suppress;
        } else {
            remaining_args.push(arg.as_str());
        }
    }

    match remaining_args.first().copied() {
        Some("--help" | "-h") => {
            print_wt_help();
            Ok(())
        }
        Some("--complete") => complete::run(Position::Wt),
        // As at the top level, `--` ends subverb parsing, which is what keeps a
        // branch named `ls`, `rm`, or one of the retired words below reachable.
        Some("--") => escaped(remaining_args.get(1).copied(), |target| {
            perch::app::wt::run(target, shell_handoff)
        }),
        // Parsed rather than matched word by word, for the same reason as the
        // verbs at the top level: this is where the subverbs are defined, and
        // the completions read what it reads rather than restating it.
        Some(name) => {
            let subverb = Subverb::parse(name);
            if shell_handoff == ShellHandoff::Suppress && subverb.is_some() {
                return Err(perch::Error::NoSwitchWithSubverb {
                    subverb: name.to_string(),
                });
            }

            match subverb {
                Some(Subverb::Ls) => perch::app::wt::run_ls(),
                // `wt <name>` creates a worktree for any word it doesn't recognise,
                // so the retired subverbs have to be turned away by name: left to
                // fall through, old muscle memory would build a branch called
                // `list`.
                Some(Subverb::List) => Err(perch::Error::retired("list", "ls")),
                Some(Subverb::Remove) => Err(perch::Error::retired("remove", "rm")),
                Some(Subverb::Rm) => dispatch_wt_rm(&remaining_args[1..]),
                None => perch::app::wt::run(Some(name), shell_handoff),
            }
        }
        None => perch::app::wt::run(None, shell_handoff),
    }
}

/// What follows a `--` at any of the three levels: a branch name, handed to that
/// level's `run`, or the completion request for the position `--` opens up.
///
/// Nothing is lost to reading `--complete` past the escape that exists to stop
/// words being read: git refuses a branch whose name begins with `-`, so
/// `perch -- --complete` could never have reached one. What it buys is that a
/// shell completing after a `--` asks about the position it is actually in,
/// rather than borrowing the answer from a verb that happens to eat nothing.
fn escaped(
    word: Option<&str>,
    run: impl FnOnce(Option<&str>) -> perch::AppResult<()>,
) -> perch::AppResult<()> {
    match word {
        Some("--complete") => complete::run(Position::Escaped),
        name => run(name),
    }
}

fn dispatch_wt_rm(args: &[&str]) -> perch::AppResult<()> {
    // Read before the target and the force flag: this prints what `rm` accepts
    // and removes nothing. A flag rather than a subverb so it can never eat a
    // branch name, and so ADR 0007's three verbs stand — which is why every
    // position answers `--complete` as a flag. Scanned across all of `args`
    // rather than read as the first word, since `rm` takes its `--force` in
    // either order. Deliberately absent from `print_wt_help`: the shell
    // completions are its only caller, and a line of usage teaching a human to
    // type it would be advertising a surface nothing asks them to use.
    if args.contains(&"--complete") {
        return perch::app::wt::run_rm_complete();
    }
    let force = args.iter().any(|a| *a == "--force" || *a == "-f");
    let target = args.iter().copied().find(|a| !a.starts_with('-'));
    perch::app::wt::run_rm(target, force)
}

fn print_help() {
    println!("Usage: perch [<branch>]       Go to the branch, wherever it lives");
    println!("       perch br [<branch>]    Check the branch out here");
    println!("       perch wt [<branch>]    Give the branch its own worktree");
    println!();
    println!("       perch .                Refresh the current branch from its remote");
    println!("       perch -- <branch>      Go to a branch named br/wt");
    println!("       perch wt ls            List worktrees");
    println!("       perch wt rm [<branch>|.]");
    println!();
    println!("With the shell integration sourced, `br` and `wt` stand in for `perch br`");
    println!("and `perch wt`. Set PERCH_NO_SHORTCUTS to leave both names alone.");
}

fn print_br_help() {
    println!("Usage: perch br [<branch>]    Check the branch out here");
}

fn print_wt_help() {
    println!("Usage: perch wt [<branch>] [--no-switch]");
    println!("                                  Give the branch its own worktree");
    println!("       perch wt ls            List worktrees");
    println!("       perch wt rm [<branch>] Remove a worktree (deletes branch if merged)");
    println!("       perch wt rm .          Remove the worktree you're in");
    println!("       perch wt -- <branch>   Worktree a branch named ls/rm/list/remove");
    println!();
    println!("Options:");
    println!("      --no-switch  Create or find the worktree without switching to it");
    println!("  -f, --force      Skip the confirmation for uncommitted or unmerged work");
}
