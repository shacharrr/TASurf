//! TASurf back end (tasurf/*.py in Rust): timeline parsing and baking, run CSVs and
//! checks, the editor's operations and its local server. Used by the command line
//! (src/main.rs) and the desktop app (app/).

pub mod bake;
pub mod bspmap; // the 3D view
pub mod buttons;
pub mod camera; // auto-strafe's smoothed camera (@view_yaw)
pub mod config;
pub mod editdoc;
pub mod editor;
pub mod gamefiles; // the 3D view
pub mod interp;
pub mod launch; // the desktop app's Setup page: finding the game and SVR, starting the game
pub mod physics;
pub mod py;
pub mod render; // the desktop app: runs to video with SVR
pub mod runcsv;
pub mod timeline;

use std::path::Path;

/// str(OSError) for a failed file operation, as Python prints it.
pub fn io_error(e: &std::io::Error, path: &Path) -> String {
    use std::io::ErrorKind::*;
    let shown = py::repr_str(&path.display().to_string());
    match e.kind() {
        NotFound => format!("[Errno 2] No such file or directory: {}", shown),
        PermissionDenied => format!("[Errno 13] Permission denied: {}", shown),
        AlreadyExists => format!("[Errno 17] File exists: {}", shown),
        _ => match e.raw_os_error() {
            Some(code) => format!("[Errno {}] {}: {}", code, e, shown),
            None => format!("{}: {}", e, shown),
        },
    }
}
