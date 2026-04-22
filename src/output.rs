use std::path::Path;

pub fn headline(mode: &str, command: &str, root: &Path) {
    println!("{mode} {command} on {}", root.display());
}

pub fn note(message: impl AsRef<str>) {
    println!("{}", message.as_ref());
}
