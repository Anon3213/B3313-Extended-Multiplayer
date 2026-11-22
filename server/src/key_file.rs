//! The key file: a server's long-term key, made the first time the server runs and read back
//! every time after, so that it stays the same server to the players who know it.
//!
//! It is apart from the settings file because settings are shown and sent to other people, and
//! whoever has this file can pose as the server.

use b3313_protocol::secure::ServerKey;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The name of the key file kept beside a settings file that names no other.
const DEFAULT_NAME: &str = "server.key";

const HEADER: &str = "# This server's key. Keep it to stay the same server to your players, \
copy it to move the\n# server to another PC, and show it to nobody: whoever has it can pose \
as this server.\n# Delete it to become a new server; players then need a new join code.\n";

/// Where the key of a server whose settings are in `config` is kept, if they name no place.
/// An empty path, for a server with no settings file, gives the folder it is run from.
#[must_use]
pub fn beside(config: &Path) -> PathBuf {
    config.with_file_name(DEFAULT_NAME)
}

/// Reads a key file: the key on a line of its own; blank lines and `#` comments are skipped.
fn parse(text: &str) -> Result<ServerKey, String> {
    let mut lines = crate::config::without_mark(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    match (lines.next(), lines.next()) {
        (Some(key), None) => ServerKey::from_private_text(key).map_err(|error| error.to_string()),
        _ => Err("it should hold one key on one line".to_owned()),
    }
}

/// Makes the file at `path` with a new key in it, failing if something is already there:
/// a key is never written over.
fn create(path: &Path) -> io::Result<ServerKey> {
    let key = ServerKey::generate().map_err(io::Error::other)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Its owner's alone to read, from the moment it exists.
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(format!("{HEADER}{}\n", key.private_text()).as_bytes())?;
    Ok(key)
}

/// The key kept in the file at `path`, which is made, with a new key, if it is not there.
///
/// # Errors
/// Fails, with a message naming the file, if it cannot be read or made, or holds something
/// that is not a key. A file that is there and unreadable is never replaced: that would make
/// this another server without its owner having asked.
pub fn load_or_create(path: &Path) -> Result<ServerKey, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text)
            .map_err(|error| format!("{} is not a server's key file: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create(path).map_err(|error| {
            format!(
                "could not keep this server's key in {}: {error}",
                path.display()
            )
        }),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{beside, load_or_create};
    use std::path::{Path, PathBuf};

    /// A file in the temporary folder, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let file = format!("b3313-key-{name}-{}.key", std::process::id());
            let scratch = Self(std::env::temp_dir().join(file));
            let _ = std::fs::remove_file(&scratch.0);
            scratch
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_server_keeps_the_key_it_made_the_first_time() -> Result<(), String> {
        let file = Scratch::new("kept");
        let first = load_or_create(&file.0)?;
        let again = load_or_create(&file.0)?;
        assert_eq!(again.public(), first.public());

        // Another file is another server.
        let other = Scratch::new("other");
        assert_ne!(load_or_create(&other.0)?.public(), first.public());
        Ok(())
    }

    #[test]
    fn a_damaged_key_file_stops_the_server_and_is_left_as_it_was() -> Result<(), String> {
        let file = Scratch::new("damaged");
        let damaged = "# a key was here\nnot a key\n";
        std::fs::write(&file.0, damaged).map_err(|error| error.to_string())?;
        let refused = load_or_create(&file.0);
        assert!(refused.is_err_and(|message| message.contains("b3313-key-damaged")));
        let kept = std::fs::read_to_string(&file.0).map_err(|error| error.to_string())?;
        assert_eq!(kept, damaged, "written over");
        Ok(())
    }

    #[test]
    fn the_key_is_kept_beside_the_settings_unless_they_say_where() {
        let config = Path::new("servers").join("friday.conf");
        assert_eq!(beside(&config), Path::new("servers").join("server.key"));
        assert_eq!(beside(Path::new("friday.conf")), Path::new("server.key"));
        assert_eq!(beside(Path::new("")), Path::new("server.key"));
    }
}
