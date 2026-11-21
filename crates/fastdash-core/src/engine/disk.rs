//! Desktop filesystem helpers.
//!
//! The browser target has no `std::fs`. These functions miss there, and the
//! callers already treat a miss as "nothing stored". Config and UI preferences
//! use `browser_store` instead, so a reload in the browser keeps them.

use std::io;
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(target_arch = "wasm32")]
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let _ = path;
    Err(unsupported())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(target_arch = "wasm32")]
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let _ = path;
    Err(unsupported())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(target_arch = "wasm32")]
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let _ = (path, contents);
    Err(unsupported())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(target_arch = "wasm32")]
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let _ = (from, to);
    Err(unsupported())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(target_arch = "wasm32")]
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let _ = path;
    Err(unsupported())
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the browser build has no filesystem",
    )
}
