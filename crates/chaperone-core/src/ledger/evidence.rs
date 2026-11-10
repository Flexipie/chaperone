//! Evidence files (R10, R13): text a check produced, beside the ledger at
//! `evidence/sha256/<hash>`, named by the sha256 of the bytes stored. Text
//! is cut to its last 256 KiB, then redacted, then hashed, so the raw bytes
//! are never stored and a retry stores the same file. An event carries only
//! the hash, and a read hashes again.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::redact;

/// What is kept of one text: its end, where a command's result is.
pub const TEXT_BYTES: usize = 256 * 1024;

/// A stored file: its name, and how many secrets were redacted out of it.
#[derive(Debug, PartialEq)]
pub struct Stored {
    pub hash: String,
    pub redactions: u64,
}

pub struct Evidence {
    dir: PathBuf,
}

impl Evidence {
    /// The store of the ledger at `ledger` (`chaperone migrate` makes the
    /// directory).
    pub fn beside(ledger: &Path) -> Evidence {
        let dir = ledger.parent().unwrap_or(Path::new(""));
        Evidence {
            dir: dir.join("evidence").join("sha256"),
        }
    }

    /// Stores `text` and returns its hash. Written and synced under another
    /// name, then renamed, so a file with a hash's name is whole; call it
    /// before the event that names the hash commits. The directory is the
    /// owner's alone (0700), the file 0600.
    pub fn put(&self, text: &str) -> io::Result<Stored> {
        let mut from = text.len().saturating_sub(TEXT_BYTES);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        let (text, redactions) = redact::text(&text[from..]);
        let hash = format!("{:x}", Sha256::digest(&text));
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        // One made before this rule, or by `mkdir`.
        fs::set_permissions(&self.dir, Permissions::from_mode(0o700))?;
        let pending = self
            .dir
            .join(format!("{hash}.{}.incomplete", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&pending)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&pending, self.dir.join(&hash))?;
        File::open(&self.dir)?.sync_all()?;
        Ok(Stored { hash, redactions })
    }

    /// The text stored as `hash`, hashed again: a file that was edited or
    /// cut is an error, never an answer.
    pub fn get(&self, hash: &str) -> io::Result<String> {
        let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if hash.len() != 64 || !hash.bytes().all(hex) {
            let why = "an evidence hash is 64 lowercase hex digits";
            return Err(io::Error::new(ErrorKind::InvalidInput, why));
        }
        let bytes = fs::read(self.dir.join(hash))?;
        if format!("{:x}", Sha256::digest(&bytes)) != hash {
            let why = format!("evidence {hash} does not match its name");
            return Err(io::Error::new(ErrorKind::InvalidData, why));
        }
        String::from_utf8(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A text is stored redacted under the hash of what is stored, owner
    /// only, and the same text again is the same file.
    #[test]
    fn text_is_redacted_then_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        let files = dir.path().join("evidence").join("sha256");
        // As `migrate` and older binaries left it.
        fs::create_dir_all(&files).unwrap();
        fs::set_permissions(&files, Permissions::from_mode(0o755)).unwrap();
        let token = format!("ghp_{}", "a".repeat(20));
        let stored = store.put(&format!("ran with {token}\nok")).unwrap();
        assert_eq!(stored.redactions, 1);
        let text = store.get(&stored.hash).unwrap();
        assert_eq!(text, "ran with [redacted:github_token]\nok");
        assert_eq!(stored.hash, format!("{:x}", Sha256::digest(&text)));
        assert_eq!(fs::read_to_string(files.join(&stored.hash)).unwrap(), text);
        assert_eq!(
            (mode(&files), mode(&files.join(&stored.hash))),
            (0o700, 0o600)
        );
        // A retry, and the stored text itself, name the same file.
        assert_eq!(store.put(&format!("ran with {token}\nok")).unwrap(), stored);
        assert_eq!(store.put(&text).unwrap().hash, stored.hash);
        assert_eq!(fs::read_dir(&files).unwrap().count(), 1);
    }

    /// Only the end of a long text is kept, cut between characters.
    #[test]
    fn long_text_keeps_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        // Two bytes each, so the cut would fall inside one.
        let long = format!("x{}", "é".repeat(TEXT_BYTES));
        let kept = store.get(&store.put(&long).unwrap().hash).unwrap();
        assert_eq!(kept, "é".repeat(TEXT_BYTES / 2));
    }

    /// A read refuses a name that is no hash, a file that is not there and
    /// one whose bytes changed.
    #[test]
    fn a_read_checks_the_name_and_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        let hash = store.put("seen").unwrap().hash;
        let file = dir.path().join("evidence").join("sha256").join(&hash);
        let kind = |hash: &str| store.get(hash).unwrap_err().kind();
        assert_eq!(kind("../../ledger.db"), ErrorKind::InvalidInput);
        assert_eq!(kind(&hash.to_uppercase()), ErrorKind::InvalidInput);
        assert_eq!(kind(&"0".repeat(64)), ErrorKind::NotFound);
        fs::write(&file, "edited").unwrap();
        assert_eq!(kind(&hash), ErrorKind::InvalidData);
        fs::write(&file, [0xff]).unwrap();
        let name = format!("{:x}", Sha256::digest([0xff]));
        fs::rename(&file, file.with_file_name(&name)).unwrap();
        assert_eq!(kind(&name), ErrorKind::InvalidData);
    }
}
