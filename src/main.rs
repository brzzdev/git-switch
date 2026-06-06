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

fn dispatch(args: &[String]) -> git_switch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("git-switch {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // `--` ends option/subcommand parsing: everything after is a branch,
        // so a branch literally named `wt`/`worktree` stays reachable.
        Some("--") => git_switch::app::run(args.get(1).map(String::as_str)),
        Some("wt" | "worktree") => dispatch_wt(&args[1..]),
        Some(name) => git_switch::app::run(Some(name)),
        None => git_switch::app::run(None),
    }
}

fn dispatch_wt(args: &[String]) -> git_switch::AppResult<()> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_wt_help();
            Ok(())
        }
        Some("ls" | "list") => git_switch::app::wt::run_ls(),
        Some("rm" | "remove") => git_switch::app::wt::run_rm(args.get(1).map(String::as_str)),
        Some(name) => git_switch::app::wt::run(Some(name)),
        None => git_switch::app::wt::run(None),
    }
}

fn print_help() {
    println!("Usage: git-switch [<branch>]");
    println!("       git-switch -- <branch>     Switch to a branch named wt/worktree");
    println!("       git-switch wt [<branch>]");
    println!("       git-switch wt ls");
    println!("       git-switch wt rm [<branch>]");
}

fn print_wt_help() {
    println!("Usage: git-switch wt [<branch>]    Switch to or create a worktree");
    println!("       git-switch wt ls            List worktrees");
    println!("       git-switch wt rm [<branch>] Remove a worktree (deletes branch if merged)");
}
