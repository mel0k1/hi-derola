fn main() {
    if let Err(e) = hiderola_gui::run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
