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
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // `--` ends option/subcommand parsing: everything after is a branch,
        // so a branch literally named `wt`/`worktree` stays reachable.
        Some("--") => perch::app::run(args.get(1).map(String::as_str)),
        Some("wt" | "worktree") => dispatch_wt(&args[1..]),
        Some(name) => perch::app::run(Some(name)),
        None => perch::app::run(None),
    }
}

fn dispatch_wt(args: &[String]) -> perch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_wt_help();
            Ok(())
        }
        Some("ls" | "list") => perch::app::wt::run_ls(),
        Some("rm" | "remove") => {
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
    println!("Usage: perch [<branch>]");
    println!("       perch .               Refresh the current branch from its remote");
    println!("       perch -- <branch>     Switch to a branch named wt/worktree");
    println!("       perch wt [<branch>]");
    println!("       perch wt ls");
    println!("       perch wt rm [<branch>|.]");
}

fn print_wt_help() {
    println!("Usage: perch wt [<branch>]    Switch to or create a worktree");
    println!("       perch wt ls            List worktrees");
    println!("       perch wt rm [<branch>] Remove a worktree (deletes branch if merged)");
    println!("       perch wt rm .          Remove the worktree you're in");
    println!();
    println!("Options:");
    println!("  -f, --force   Skip the confirmation for uncommitted or unmerged work");
}
