//! The ban file: banned IP addresses, one a line, kept up to date as players are banned and
//! unbanned.

use crate::Event;
use crate::config::ConfigError;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// Reads a ban file: one IP address a line; blank lines and `#` comments are skipped.
///
/// # Errors
/// Fails, naming the line, on anything that is not an IP address.
pub fn parse_bans(text: &str) -> Result<Vec<IpAddr>, ConfigError> {
    text.lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let content = raw.split('#').next().unwrap_or_default().trim();
            (!content.is_empty()).then_some((index + 1, content))
        })
        .map(|(line, content)| {
            content.parse().map_err(|_| ConfigError {
                line,
                message: format!("{content} is not an IP address"),
            })
        })
        .collect()
}

/// Writes a ban file that `parse_bans` reads back.
#[must_use]
pub fn format_bans(bans: &[IpAddr]) -> String {
    let header = "# IP addresses banned from this server, one a line.\n";
    let lines = bans.iter().map(|ip| format!("{ip}\n"));
    std::iter::once(header.to_owned()).chain(lines).collect()
}

/// Reads the ban file at `path`. A missing file is an empty one, so a new server can name the
/// file it will keep.
///
/// # Errors
/// Fails, with a message naming the file, if it cannot be read or holds something that is not an
/// IP address.
pub fn read_ban_file(path: &Path) -> Result<Vec<IpAddr>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_bans(&text).map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// The bans in force, kept in the ban file, if there is one, as they change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BanList {
    path: Option<PathBuf>,
    bans: Vec<IpAddr>,
}

impl BanList {
    /// The bans a server starts with, to be kept in the file at `path`.
    #[must_use]
    pub fn new(path: Option<PathBuf>, bans: Vec<IpAddr>) -> Self {
        Self { path, bans }
    }

    /// The bans kept in the file at `path`, joined by `unkept`: those made while no file kept
    /// them, which this one keeps from now on.
    ///
    /// # Errors
    /// Fails, with a message naming the file, if it cannot be read, holds something that is
    /// not an IP address, or cannot take the bans it is to keep.
    pub fn open(path: PathBuf, unkept: &[IpAddr]) -> Result<Self, String> {
        let mut bans = read_ban_file(&path)?;
        let kept = bans.len();
        for ip in unkept {
            if !bans.contains(ip) {
                bans.push(*ip);
            }
        }
        let list = Self::new(Some(path), bans);
        if list.bans.len() > kept {
            list.write()?;
        }
        Ok(list)
    }

    /// The file the bans are kept in, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub fn bans(&self) -> &[IpAddr] {
        &self.bans
    }

    /// Follows a ban or unban, rewriting the file when the list changes.
    ///
    /// # Errors
    /// Fails if the file cannot be written; the list in memory, like the server's, has changed
    /// anyway, and the ban holds until the server stops.
    pub fn update(&mut self, event: &Event) -> Result<(), String> {
        match event {
            Event::Banned(_, ip) | Event::AddressBanned(ip) if !self.bans.contains(ip) => {
                self.bans.push(*ip);
            }
            Event::Unbanned(ip) => self.bans.retain(|banned| banned != ip),
            _ => return Ok(()),
        }
        self.write()
    }

    fn write(&self) -> Result<(), String> {
        match &self.path {
            Some(path) => std::fs::write(path, format_bans(&self.bans)).map_err(|error| {
                format!("could not save the ban list to {}: {error}", path.display())
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BanList, format_bans, parse_bans, read_ban_file};
    use crate::Event;
    use b3313_protocol::PlayerId;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::PathBuf;

    #[test]
    fn a_ban_file_reads_back_what_was_written() {
        let bans: [IpAddr; 2] = [
            Ipv4Addr::new(192, 168, 1, 20).into(),
            Ipv6Addr::LOCALHOST.into(),
        ];
        let written = format_bans(&bans);
        assert_eq!(parse_bans(&written), Ok(bans.to_vec()));
        let error = parse_bans("10.0.0.1\n\n# note\nsomewhere\n").err();
        assert_eq!(error.map(|e| e.line), Some(4));
    }

    /// A file path no other test uses, removed when dropped.
    struct ScratchFile(PathBuf);

    impl ScratchFile {
        fn new(name: &str) -> Self {
            let file = format!("b3313-{name}-{}.txt", std::process::id());
            Self(std::env::temp_dir().join(file))
        }
    }

    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn bans_and_unbans_are_kept_in_the_file() {
        let file = ScratchFile::new("ban-list");
        // A missing file is an empty one.
        assert_eq!(read_ban_file(&file.0), Ok(Vec::new()));
        let (first, second) = (
            IpAddr::from(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::from(Ipv4Addr::new(10, 0, 0, 2)),
        );
        let mut list = BanList::new(Some(file.0.clone()), Vec::new());
        let player = PlayerId::new(2).unwrap_or_else(|| unreachable!());

        for event in [
            Event::Banned(player, first),
            Event::AddressBanned(second),
            Event::AddressBanned(first),
            Event::Unbanned(first),
            Event::Left(player),
        ] {
            assert_eq!(list.update(&event), Ok(()));
        }

        assert_eq!(list.bans(), [second]);
        assert_eq!(read_ban_file(&file.0), Ok(vec![second]));
    }

    #[test]
    fn a_file_named_later_keeps_the_bans_made_before_it() {
        let file = ScratchFile::new("ban-list-named-later");
        let (in_file, made_before) = (
            IpAddr::from(Ipv4Addr::new(10, 0, 0, 4)),
            IpAddr::from(Ipv4Addr::new(10, 0, 0, 5)),
        );
        let written = std::fs::write(&file.0, format_bans(&[in_file]));
        assert!(written.is_ok());

        let list = BanList::open(file.0.clone(), &[made_before, in_file]);

        let both = vec![in_file, made_before];
        assert_eq!(list.map(|list| list.bans().to_vec()), Ok(both.clone()));
        assert_eq!(read_ban_file(&file.0), Ok(both));
    }

    #[test]
    fn a_ban_file_that_cannot_be_written_is_reported() {
        let directory = std::env::temp_dir();
        let mut list = BanList::new(Some(directory), Vec::new());
        let ip = IpAddr::from(Ipv4Addr::new(10, 0, 0, 3));
        assert!(list.update(&Event::AddressBanned(ip)).is_err());
        assert_eq!(list.bans(), [ip], "the ban holds anyway");
    }
}
