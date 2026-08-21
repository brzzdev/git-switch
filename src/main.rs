use std::process;

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
        // `--` ends option/subcommand parsing: everything after is a branch,
        // so a branch literally named `br`/`wt` stays reachable.
        Some("--") => perch::app::run(args.get(1).map(String::as_str)),
        Some("br") => dispatch_br(&args[1..]),
        Some("wt") => dispatch_wt(&args[1..]),
        Some(name) => perch::app::run(Some(name)),
        None => perch::app::run(None),
    }
}

fn dispatch_br(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_br_help();
            Ok(())
        }
        name => perch::app::run_br(name),
    }
}

fn dispatch_wt(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_wt_help();
            Ok(())
        }
        Some("ls") => perch::app::wt::run_ls(),
        // `wt <name>` creates a worktree for any word it doesn't recognise, so
        // the retired subverbs have to be turned away by name: left to fall
        // through, old muscle memory would build a branch called `list`.
        Some("list") => Err(perch::Error::retired("wt list", "wt ls")),
        Some("remove") => Err(perch::Error::retired("wt remove", "wt rm")),
        Some("rm") => {
            let rest = &args[1..];
            let force = rest.iter().any(|a| a == "--force" || a == "-f");
            let target = rest
                .iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str);
            perch::app::wt::run_rm(target, force)
        }
        Some(name) => perch::app::wt::run(Some(name)),
        None => perch::app::wt::run(None),
    }
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
}

fn print_br_help() {
    println!("Usage: perch br [<branch>]    Check the branch out here");
}

fn print_wt_help() {
    println!("Usage: perch wt [<branch>]    Give the branch its own worktree");
    println!("       perch wt ls            List worktrees");
    println!("       perch wt rm [<branch>] Remove a worktree (deletes branch if merged)");
    println!("       perch wt rm .          Remove the worktree you're in");
    println!();
    println!("Options:");
    println!("  -f, --force   Skip the confirmation for uncommitted or unmerged work");
}
