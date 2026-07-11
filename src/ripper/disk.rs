//! Raw `.ST` disk-image source (PLAN §P7): logical sector 0 (first 512
//! bytes) of a plain sector dump. `.MSA`/`.DIM`/`.STX` are compressed/
//! containered and OUT OF SCOPE — rejected with a clear error, not misread
//! (future work). Path is validated against traversal per the security
//! rules.

use std::path::Path;

use crate::error::{Error, Result};
use crate::ripper::plugins::bootsector::SECTOR_LEN;

/// Read logical sector 0 of a raw `.ST` image. Validates the path first
/// (§7 rules); returns exactly 512 bytes.
pub fn read_boot_sector(path: &Path) -> Result<Vec<u8>> {
    // SECURITY: path traversal / unexpected-file guard.
    let canon = path.canonicalize()?;
    let meta = std::fs::symlink_metadata(&canon)?;
    if !meta.is_file() {
        return Err(Error::Parse {
            context: "disk image".to_string(),
            input: format!("{} is not a regular file", canon.display()),
        });
    }
    validate_extension(&canon)?;
    let bytes = std::fs::read(&canon)?;
    if bytes.len() < SECTOR_LEN {
        return Err(Error::Parse {
            context: "boot sector".to_string(),
            input: "file shorter than 512 bytes".to_string(),
        });
    }
    Ok(bytes[..SECTOR_LEN].to_vec())
}

/// Extension gate (§7.3, case-insensitive): accept `st` or no extension;
/// reject known containers loudly; any other extension fails the same way
/// (never guess a format from bytes alone).
fn validate_extension(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        None | Some("st") => Ok(()),
        Some("msa" | "dim" | "stx" | "zip") => Err(Error::Parse {
            context: "disk image".to_string(),
            input: "compressed/containered image not supported (P7: raw .ST only)".to_string(),
        }),
        Some(_) => Err(Error::Parse {
            context: "disk image".to_string(),
            input: "unsupported extension (P7: raw .ST only)".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("shirazmcp-disk-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn reads_sector_zero() {
        let path = unique_path("reads_sector_zero.st");
        std::fs::write(&path, vec![0xABu8; 600]).unwrap();
        let sector = read_boot_sector(&path).unwrap();
        assert_eq!(sector.len(), SECTOR_LEN);
        assert!(sector.iter().all(|&b| b == 0xAB));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_short_file() {
        let path = unique_path("rejects_short_file.st");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let err = read_boot_sector(&path).unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "boot sector"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_msa_extension() {
        let path = unique_path("rejects_msa_extension.msa");
        std::fs::write(&path, vec![0u8; 600]).unwrap();
        let err = read_boot_sector(&path).unwrap_err();
        match err {
            Error::Parse { input, .. } => assert!(input.contains("raw .ST only")),
            other => panic!("expected Error::Parse, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_directory() {
        let path = unique_path("rejects_directory_dir");
        std::fs::create_dir_all(&path).unwrap();
        let err = read_boot_sector(&path).unwrap_err();
        match err {
            Error::Parse { input, .. } => assert!(input.contains("not a regular file")),
            other => panic!("expected Error::Parse, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn rejects_missing() {
        let path = unique_path("rejects_missing_nonexistent.st");
        assert!(matches!(read_boot_sector(&path), Err(Error::Io(_))));
    }

    #[test]
    fn accepts_st_extension() {
        let path = unique_path("accepts_st_extension.st");
        std::fs::write(&path, vec![0u8; SECTOR_LEN]).unwrap();
        let sector = read_boot_sector(&path).unwrap();
        assert_eq!(sector.len(), SECTOR_LEN);
        let _ = std::fs::remove_file(&path);
    }
}
