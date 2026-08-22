use std::process;

use perch::app::complete::{Position, TopWord, WtWord};

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
        // will accept rather than going to one. See `dispatch_wt`'s `rm` arm for
        // why every one of these is a flag and not a subcommand.
        Some("--complete") => perch::app::complete::run(Position::Top),
        // `--` ends option/subcommand parsing: everything after is a branch,
        // so a branch literally named `br`/`wt` stays reachable.
        Some("--") => perch::app::run(args.get(1).map(String::as_str)),
        // Parsed rather than matched word by word, so the verbs exist in one
        // place: this match is exhaustive over them, and the completions
        // subtract the same table. See `perch::app::complete`.
        Some(name) => match TopWord::parse(name) {
            Some(TopWord::Br) => dispatch_br(&args[1..]),
            Some(TopWord::Wt) => dispatch_wt(&args[1..]),
            None => perch::app::run(Some(name)),
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
        // `br` eats no word of its own, so this is every branch there is — which
        // is also what the completions want after a `--` at any level.
        Some("--complete") => perch::app::complete::run(Position::Br),
        Some("--") => perch::app::run_br(args.get(1).map(String::as_str)),
        name => perch::app::run_br(name),
    }
}

fn dispatch_wt(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_wt_help();
            Ok(())
        }
        Some("--complete") => perch::app::complete::run(Position::Wt),
        // As at the top level, `--` ends subverb parsing, which is what keeps a
        // branch named `ls`, `rm`, or one of the retired words below reachable.
        Some("--") => perch::app::wt::run(args.get(1).map(String::as_str)),
        // Parsed rather than matched word by word, for the same reason as the
        // verbs at the top level: this is where the subverbs are defined, and
        // the completions read the table rather than restating it.
        Some(name) => match WtWord::parse(name) {
            Some(WtWord::Ls) => perch::app::wt::run_ls(),
            // `wt <name>` creates a worktree for any word it doesn't recognise,
            // so the retired subverbs have to be turned away by name: left to
            // fall through, old muscle memory would build a branch called
            // `list`.
            Some(WtWord::List) => Err(perch::Error::retired("list", "ls")),
            Some(WtWord::Remove) => Err(perch::Error::retired("remove", "rm")),
            Some(WtWord::Rm) => dispatch_wt_rm(&args[1..]),
            None => perch::app::wt::run(Some(name)),
        },
        None => perch::app::wt::run(None),
    }
}

fn dispatch_wt_rm(args: &[String]) -> perch::AppResult<()> {
    // Read before the target and the force flag: this prints what `rm` accepts
    // and removes nothing. A flag rather than a subverb so it can never eat a
    // branch name, and so ADR 0007's three verbs stand — which is why the three
    // completion flags above are flags too. Deliberately absent from
    // `print_wt_help`: the shell completions are its only caller, and a line of
    // usage teaching a human to type it would be advertising a surface nothing
    // asks them to use.
    if args.iter().any(|a| a == "--complete") {
        return perch::app::wt::run_rm_complete();
    }
    let force = args.iter().any(|a| a == "--force" || a == "-f");
    let target = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
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
    println!("Usage: perch wt [<branch>]    Give the branch its own worktree");
    println!("       perch wt ls            List worktrees");
    println!("       perch wt rm [<branch>] Remove a worktree (deletes branch if merged)");
    println!("       perch wt rm .          Remove the worktree you're in");
    println!("       perch wt -- <branch>   Worktree a branch named ls/rm/list/remove");
    println!();
    println!("Options:");
    println!("  -f, --force   Skip the confirmation for uncommitted or unmerged work");
}
