//! Saving a buffer to disk atomically, refusing when the file moved under it.

use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a save did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The file on disk no longer has the blob the buffer was loaded from.
    Moved { disk_blob: String },
    /// The file no longer exists. A save never recreates it.
    Gone,
    /// Reading, writing or renaming failed; the text is the io error.
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Moved { .. } => f.write_str("changed on disk since it was opened"),
            Self::Gone => f.write_str("no longer exists"),
            Self::Io(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for SaveError {}

/// Git's blob id of `bytes`: the SHA-1 of `blob <len>\0` followed by the bytes, in hex.
pub fn blob_id(bytes: &[u8]) -> String {
    let mut object = format!("blob {}\0", bytes.len()).into_bytes();
    object.extend_from_slice(bytes);
    to_hex(&sha1(&object))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Write `text` over `path` and return the new blob id.
///
/// Unless `force`, the write is refused with [`SaveError::Moved`] when the file's
/// current blob differs from `loaded_blob`. A missing file is [`SaveError::Gone`]
/// even with `force`. The write goes through a temp file beside the target and a
/// rename, keeping the target's permissions; a symlink is followed, so the
/// link stays and its target is replaced.
pub fn save(path: &Path, text: &str, loaded_blob: &str, force: bool) -> Result<String, SaveError> {
    let current = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(SaveError::Gone),
        Err(error) => return Err(io_error(&error)),
    };
    let disk_blob = blob_id(&current);
    if !force && disk_blob != loaded_blob {
        return Err(SaveError::Moved { disk_blob });
    }

    let target = fs::canonicalize(path).map_err(|error| io_error(&error))?;
    let permissions = fs::metadata(&target)
        .map_err(|error| io_error(&error))?
        .permissions();
    let dir = target.parent().unwrap_or(Path::new("."));
    let temp = dir.join(temp_name(&target));

    let written =
        write_temp(&temp, text.as_bytes(), permissions).and_then(|()| fs::rename(&temp, &target));
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(io_error(&error));
    }
    Ok(blob_id(text.as_bytes()))
}

fn io_error(error: &io::Error) -> SaveError {
    SaveError::Io(error.to_string())
}

/// A name beside `target` that no other save picks: hidden, with the pid and a nanosecond stamp.
fn temp_name(target: &Path) -> String {
    let name = target
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(".{name}.rvw-save-{}-{stamp}", std::process::id())
}

fn write_temp(temp: &Path, bytes: &[u8], permissions: fs::Permissions) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    file.write_all(bytes)?;
    file.set_permissions(permissions)?;
    file.sync_all()
}

/// SHA-1 (FIPS 180-4) of `data`. Used only for git blob ids, never for security.
// The working variables keep the names a..e that FIPS 180-4 gives them.
#[allow(clippy::many_single_char_names)]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0u8; 20];
    for (slot, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *slot = word.to_be_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
    const HELLO: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

    fn file_with(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("notes.txt");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn blob_id_matches_git_for_known_content() {
        assert_eq!(blob_id(b""), EMPTY);
        assert_eq!(blob_id(b"hello\n"), HELLO);
    }

    #[test]
    fn sha1_matches_a_multi_block_message() {
        // 56 bytes: the padding does not fit the first block, so it spills into a second.
        let hex = to_hex(&sha1(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
        ));
        assert_eq!(hex, "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }

    #[test]
    fn save_writes_text_and_returns_its_blob_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "old\n");
        let loaded = blob_id(b"old\n");

        let id = save(&path, "new text\n", &loaded, false).unwrap();

        assert_eq!(id, blob_id(b"new text\n"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "new text\n");
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "no temp file is left beside the target");
    }

    #[test]
    fn save_refuses_when_the_file_changed_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "theirs\n");
        let loaded = blob_id(b"ours\n");

        let error = save(&path, "mine\n", &loaded, false).unwrap_err();

        assert_eq!(
            error,
            SaveError::Moved {
                disk_blob: blob_id(b"theirs\n")
            }
        );
        assert_eq!(error.to_string(), "changed on disk since it was opened");
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");
    }

    #[test]
    fn force_overwrites_a_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "theirs\n");

        let id = save(&path, "mine\n", &blob_id(b"ours\n"), true).unwrap();

        assert_eq!(id, blob_id(b"mine\n"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine\n");
    }

    #[test]
    fn a_missing_file_is_gone_even_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleted.txt");
        let loaded = blob_id(b"x");

        assert_eq!(save(&path, "x", &loaded, false), Err(SaveError::Gone));
        assert_eq!(save(&path, "x", &loaded, true), Err(SaveError::Gone));
        assert!(!path.exists(), "a save never recreates a deleted file");
        assert_eq!(SaveError::Gone.to_string(), "no longer exists");
    }

    #[cfg(unix)]
    #[test]
    fn read_only_directory_reports_the_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "old\n");
        let loaded = blob_id(b"old\n");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let result = save(&path, "new\n", &loaded, false);

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.unwrap_err();
        assert!(matches!(error, SaveError::Io(_)), "got {error:?}");
        assert!(
            error.to_string().contains("Permission denied"),
            "got {error}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_the_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "old\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        save(&path, "new\n", &blob_id(b"old\n"), false).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn save_through_a_symlink_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = file_with(dir.path(), "old\n");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let id = save(&link, "new\n", &blob_id(b"old\n"), false).unwrap();

        assert_eq!(id, blob_id(b"new\n"));
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_link(&link).unwrap(), real);
        assert_eq!(fs::read_to_string(&real).unwrap(), "new\n");
    }
}
