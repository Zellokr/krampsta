//! The images messages carry, one PNG file each in `images/` beside the sessions, named
//! `<session>-<message>.png`. A message's image never changes, so each is written once: saving
//! a session again rewrites only its JSON, which names the file instead of holding the image.

use std::fs;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use krampsta_core::{MessageId, Session, SessionId};

use crate::StoreError;

/// The file `message`'s image is saved in, as the session's JSON names it.
pub fn name(session: SessionId, message: MessageId) -> String {
    format!("{session}-{message}.png")
}

fn folder(dir: &Path) -> PathBuf {
    dir.join("images")
}

/// Writes the images of `session` that aren't on disk yet, before its JSON names them.
pub fn write_new(dir: &Path, session: &Session) -> Result<(), StoreError> {
    for m in &session.messages {
        let Some(png) = &m.image_png else { continue };
        let path = folder(dir).join(name(session.id, m.id));
        if path.exists() {
            continue;
        }
        let write = |path: &Path| -> std::io::Result<()> {
            let bytes = STANDARD.decode(png.as_bytes()).map_err(std::io::Error::other)?;
            fs::create_dir_all(folder(dir))?;
            // Beside its name, then renamed over it: a crash never leaves half an image that
            // `exists` would take for written.
            let temp = path.with_extension("png.tmp");
            fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, path)).inspect_err(|_| {
                // Best effort: a leftover temporary file is harmless (nothing reads it).
                let _ = fs::remove_file(&temp);
            })
        };
        write(&path).map_err(|source| StoreError::ImageWrite { path, source })?;
    }
    Ok(())
}

/// The image a session's JSON names, as base64 (how messages carry it).
pub fn read(dir: &Path, file: &str) -> Result<String, StoreError> {
    let path = folder(dir).join(file);
    // Only a plain file name: the JSON never points outside the images folder.
    if Path::new(file).file_name().is_none_or(|n| n != file) {
        let source = std::io::Error::new(std::io::ErrorKind::InvalidData, "not a plain file name");
        return Err(StoreError::ImageRead { path, source });
    }
    fs::read(&path).map(|bytes| STANDARD.encode(bytes)).map_err(|source| StoreError::ImageRead { path, source })
}

/// Removes every image of `session`.
pub fn delete(dir: &Path, session: SessionId) -> Result<(), StoreError> {
    let folder = folder(dir);
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        // No image was ever saved.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(StoreError::Delete { path: folder, source }),
    };
    let prefix = format!("{session}-");
    for entry in entries {
        let path = entry.map_err(|source| StoreError::Delete { path: folder.clone(), source })?.path();
        let ours = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix));
        if ours {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(StoreError::Delete { path, source }),
            }
        }
    }
    Ok(())
}
