use std::{
    fs,
    io,
    path,
    process,
};

pub use self::arch::Arch;
mod arch;

pub use self::pkg::Pkg;
mod pkg;

pub fn ensure_dir<P: AsRef<path::Path>>(path: P) -> io::Result<path::PathBuf> {
    if ! path.as_ref().is_dir() {
        fs::create_dir_all(&path)?;
    }
    fs::canonicalize(&path)
}

pub fn ensure_dir_clean<P: AsRef<path::Path>>(path: P) -> io::Result<path::PathBuf> {
    if path.as_ref().is_dir() {
        fs::remove_dir_all(&path)?;
    }
    ensure_dir(&path)
}

pub fn status_err(status: process::ExitStatus) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("exited with status {}", status)
        ))
    }
}
