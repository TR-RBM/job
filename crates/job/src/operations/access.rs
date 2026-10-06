use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::store::Store;

const NAME: &str = "terminal.sock";
const DIRECTORY: &str = "terminals";

static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

fn private(store: &Store, id: u64) -> PathBuf {
    store.job_dir(id).join(NAME)
}

fn shared_directory(store: &Store) -> Option<PathBuf> {
    let info = crate::service::published()?;
    info.socket_group.as_ref()?;
    let runtime = info.socket.parent()?;
    (runtime != store.root).then(|| runtime.join(DIRECTORY))
}

pub fn terminal_socket(store: &Store, id: u64) -> std::io::Result<PathBuf> {
    let Some(directory) = shared_directory(store) else {
        return Ok(private(store, id));
    };
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => crate::service::access::shared().share(&directory, 0o750)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(directory.join(format!("{id}.sock")))
}

pub fn listening_terminal(store: &Store, id: u64) -> Option<PathBuf> {
    shared_directory(store)
        .map(|directory| directory.join(format!("{id}.sock")))
        .into_iter()
        .chain([private(store, id)])
        .find(|path| path.exists())
}

pub fn supervisor_argument(store: &Store, id: u64) -> std::io::Result<Option<PathBuf>> {
    let path = terminal_socket(store, id)?;
    Ok((path != private(store, id)).then_some(path))
}

pub fn supervised(path: Option<String>) {
    if let Some(path) = path {
        let _ = OVERRIDE.set(PathBuf::from(path));
    }
}

fn stale(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
        && std::os::unix::net::UnixStream::connect(path).is_err()
}

pub fn supervisor_terminal(store: &Store, id: u64) -> PathBuf {
    match OVERRIDE.get() {
        Some(path) => {
            if stale(path) {
                let _ = std::fs::remove_file(path);
            }
            path.clone()
        }
        None => private(store, id),
    }
}
