use std::process;

fn main() {
    let target = std::env::args().nth(1);

    if let Err(e) = git_switch::app::run(target.as_deref()) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
