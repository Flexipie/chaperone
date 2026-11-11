//! Evidence files (R10, R13): text a check produced, beside the ledger at
//! `evidence/sha256/<first two hex>/<hash>`, named by the sha256 of the
//! bytes stored. Text is redacted, then cut to its last 256 KiB, then
//! hashed, so the raw bytes are never stored (a cut cannot leave the end of
//! a secret whose start it removed) and a retry stores the same file. An
//! event carries only the hash, and a read hashes again.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::redact;

/// What is kept of one text: its end, where a command's result is.
pub const TEXT_BYTES: usize = 256 * 1024;

/// A stored file: its name, how many secrets were redacted out of the text,
/// and whether its start was cut off.
#[derive(Debug, PartialEq)]
pub struct Stored {
    pub hash: String,
    pub redactions: u64,
    pub truncated: bool,
}

/// A text as the store keeps it, before it is written: redacted, then its
/// end, and the name it will have. A caller that must not write yet (a
/// write that may still be refused) names it first.
pub struct Kept {
    pub stored: Stored,
    pub text: String,
}

impl Kept {
    pub fn of(text: &str) -> Kept {
        let (text, redactions) = redact::text(text);
        let mut from = text.len().saturating_sub(TEXT_BYTES);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        let kept = text[from..].to_string();
        Kept {
            stored: Stored {
                hash: format!("{:x}", Sha256::digest(&kept)),
                redactions,
                truncated: from > 0,
            },
            text: kept,
        }
    }
}

/// Tells this process's unfinished files apart.
static PENDING: AtomicU64 = AtomicU64::new(0);

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

    /// Stores `text` and returns its hash. Written and synced under a name
    /// of its own, then renamed, so a file with a hash's name is whole;
    /// call it before the event that names the hash commits. The
    /// directories are the owner's alone (0700), the file 0600.
    pub fn put(&self, text: &str) -> io::Result<Stored> {
        let kept = Kept::of(text);
        self.write(&kept)?;
        Ok(kept.stored)
    }

    /// Writes `kept` as [`Evidence::put`] does.
    pub fn write(&self, kept: &Kept) -> io::Result<()> {
        let hash = &kept.stored.hash;
        let dir = self.dir.join(&hash[..2]);
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        // One made before this rule, or by `mkdir`.
        fs::set_permissions(&self.dir, Permissions::from_mode(0o700))?;
        let n = PENDING.fetch_add(1, Ordering::Relaxed);
        let pending = dir.join(format!("{hash}.{}-{n}.incomplete", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending)?;
        file.write_all(kept.text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&pending, dir.join(hash))?;
        File::open(&dir)?.sync_all()
    }

    /// The hash a unique prefix of 12 to 64 lowercase hex digits names.
    pub fn resolve(&self, prefix: &str) -> io::Result<String> {
        if !(12..=64).contains(&prefix.len()) || !is_hex(prefix) {
            let why = "an evidence ref is 12 to 64 lowercase hex digits";
            return Err(io::Error::new(ErrorKind::InvalidInput, why));
        }
        let mut found = Vec::new();
        for entry in fs::read_dir(self.dir.join(&prefix[..2]))? {
            let name = entry?.file_name();
            // Not an unfinished file.
            if let Some(name) = name.to_str().filter(|n| n.len() == 64)
                && name.starts_with(prefix)
            {
                found.push(name.to_string());
            }
        }
        match found.len() {
            0 => Err(io::Error::new(ErrorKind::NotFound, "no evidence file")),
            1 => Ok(found.remove(0)),
            n => {
                let why = format!("evidence ref {prefix} matches {n} files; give more digits");
                Err(io::Error::new(ErrorKind::InvalidInput, why))
            }
        }
    }

    /// The text stored as `hash`, hashed again: a file that was edited or
    /// cut is an error, never an answer.
    pub fn get(&self, hash: &str) -> io::Result<String> {
        if hash.len() != 64 || !is_hex(hash) {
            let why = "an evidence hash is 64 lowercase hex digits";
            return Err(io::Error::new(ErrorKind::InvalidInput, why));
        }
        let bytes = fs::read(self.dir.join(&hash[..2]).join(hash))?;
        if format!("{:x}", Sha256::digest(&bytes)) != hash {
            let why = format!("evidence {hash} does not match its name");
            return Err(io::Error::new(ErrorKind::InvalidData, why));
        }
        String::from_utf8(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
    }
}

fn is_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn store() -> (tempfile::TempDir, Evidence, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let store = Evidence::beside(&dir.path().join("ledger.db"));
        let files = dir.path().join("evidence").join("sha256");
        (dir, store, files)
    }

    /// A text is stored redacted under the hash of what is stored, owner
    /// only, and the same text again is the same file.
    #[test]
    fn text_is_redacted_then_hashed() {
        let (_dir, store, files) = store();
        // As `migrate` and older binaries left it.
        fs::create_dir_all(&files).unwrap();
        fs::set_permissions(&files, Permissions::from_mode(0o755)).unwrap();
        let token = format!("ghp_{}", "a".repeat(20));
        let stored = store.put(&format!("ran with {token}\nok")).unwrap();
        assert_eq!((stored.redactions, stored.truncated), (1, false));
        let text = store.get(&stored.hash).unwrap();
        assert_eq!(text, "ran with [redacted:github_token]\nok");
        assert_eq!(stored.hash, format!("{:x}", Sha256::digest(&text)));
        let (sub, file) = (files.join(&stored.hash[..2]), &stored.hash);
        assert_eq!(fs::read_to_string(sub.join(file)).unwrap(), text);
        let modes = (mode(&files), mode(&sub), mode(&sub.join(file)));
        assert_eq!(modes, (0o700, 0o700, 0o600));
        // A retry, and the stored text itself, name the same file.
        assert_eq!(store.put(&format!("ran with {token}\nok")).unwrap(), stored);
        assert_eq!(store.put(&text).unwrap().hash, stored.hash);
        assert_eq!(fs::read_dir(&sub).unwrap().count(), 1);
    }

    /// Only the end of a long text is kept, cut between characters and
    /// after redaction: a secret the cut falls in is not stored in part,
    /// and what is stored is at most the cap and stores as itself.
    #[test]
    fn long_text_keeps_its_end() {
        let (_dir, store, _) = store();
        // Two bytes each, so the cut falls inside one.
        let long = format!("{}x", "é".repeat(TEXT_BYTES));
        let stored = store.put(&long).unwrap();
        assert!(stored.truncated);
        let kept = store.get(&stored.hash).unwrap();
        assert_eq!(kept, format!("{}x", "é".repeat(TEXT_BYTES / 2 - 1)));

        // The cut of the raw text would fall inside the token.
        let tail = "b".repeat(TEXT_BYTES - 10);
        let stored = store.put(&format!("ghp_{}{tail}", "a".repeat(30))).unwrap();
        let kept = store.get(&stored.hash).unwrap();
        assert!(
            !kept.contains("aaaa") && kept.len() <= TEXT_BYTES,
            "{}",
            &kept[..60]
        );

        // Redaction grows this text; the cap still holds.
        let stored = store.put(&"TOKEN=v ".repeat(TEXT_BYTES / 8)).unwrap();
        let kept = store.get(&stored.hash).unwrap();
        assert_eq!(kept.len(), TEXT_BYTES);
        assert_eq!(store.put(&kept).unwrap().hash, stored.hash);
    }

    /// Threads storing one text never see or leave a partial file.
    #[test]
    fn one_text_from_many_threads_is_whole() {
        let (_dir, store, _) = store();
        let text = "line\n".repeat(40_000);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..20 {
                        let hash = store.put(&text).unwrap().hash;
                        assert_eq!(store.get(&hash).unwrap(), text);
                    }
                });
            }
        });
    }

    /// A prefix names the one file it starts; a short, upper case,
    /// unknown or shared one does not.
    #[test]
    fn a_prefix_names_one_file() {
        let (_dir, store, files) = store();
        let hash = store.put("seen").unwrap().hash;
        assert_eq!(store.resolve(&hash[..12]).unwrap(), hash);
        assert_eq!(store.resolve(&hash).unwrap(), hash);
        let kind = |r: &str| store.resolve(r).unwrap_err().kind();
        assert_eq!(kind(&hash[..11]), ErrorKind::InvalidInput);
        assert_eq!(kind(&hash[..12].to_uppercase()), ErrorKind::InvalidInput);
        assert_eq!(kind(&"0".repeat(12)), ErrorKind::NotFound);
        let mut twin = hash[..12].to_string();
        twin.push_str(&"0".repeat(52));
        fs::write(files.join(&hash[..2]).join(&twin), "x").unwrap();
        assert_eq!(kind(&hash[..12]), ErrorKind::InvalidInput);
        assert_eq!(store.resolve(&hash[..13]).unwrap(), hash);
    }

    /// A read refuses a name that is no hash, a file that is not there and
    /// one whose bytes changed.
    #[test]
    fn a_read_checks_the_name_and_the_bytes() {
        let (_dir, store, files) = store();
        let hash = store.put("seen").unwrap().hash;
        let file = files.join(&hash[..2]).join(&hash);
        let kind = |hash: &str| store.get(hash).unwrap_err().kind();
        assert_eq!(kind("../../ledger.db"), ErrorKind::InvalidInput);
        assert_eq!(kind(&hash.to_uppercase()), ErrorKind::InvalidInput);
        assert_eq!(kind(&"0".repeat(64)), ErrorKind::NotFound);
        fs::write(&file, "edited").unwrap();
        assert_eq!(kind(&hash), ErrorKind::InvalidData);
        // Bytes that match their name and are no text.
        let name = format!("{:x}", Sha256::digest([0xff]));
        fs::create_dir_all(files.join(&name[..2])).unwrap();
        fs::write(files.join(&name[..2]).join(&name), [0xff]).unwrap();
        assert_eq!(kind(&name), ErrorKind::InvalidData);
    }
}
