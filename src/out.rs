use std::io::{ErrorKind, Write};

pub fn say(text: impl AsRef<str>) {
    let Err(error) = writeln!(std::io::stdout(), "{}", text.as_ref()) else {
        return;
    };
    if error.kind() != ErrorKind::BrokenPipe {
        return;
    }
    std::process::exit(0);
}

pub fn warn(text: impl AsRef<str>) {
    let _ = writeln!(std::io::stderr(), "{}", text.as_ref());
}
