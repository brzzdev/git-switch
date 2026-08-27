use std::process;

fn main() {
    let _ = ctrlc::set_handler(|| {
        let _ = console::Term::stderr().show_cursor();
        process::exit(130);
    });

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = perch::run(&args);

    if let Err(e) = result {
        if e.is_interrupt() {
            let _ = console::Term::stderr().show_cursor();
            process::exit(130);
        }
        eprintln!("error: {e}");
        process::exit(1);
    }
}
