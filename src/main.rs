use std::process;

fn main() {
    let _ = ctrlc::set_handler(|| {
        let _ = console::Term::stderr().show_cursor();
        process::exit(130);
    });

    let target = match std::env::args().nth(1).as_deref() {
        Some("--help" | "-h") => {
            println!("Usage: git-switch [<branch>]");
            return;
        }
        Some("--version" | "-V") => {
            println!("git-switch {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some(name) => Some(name.to_string()),
        None => None,
    };

    if let Err(e) = git_switch::app::run(target.as_deref()) {
        if let git_switch::Error::Io(io) = &e
            && io.kind() == std::io::ErrorKind::Interrupted
        {
            let _ = console::Term::stderr().show_cursor();
            process::exit(130);
        }
        eprintln!("error: {e}");
        process::exit(1);
    }
}
