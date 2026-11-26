//! Files that usually hold secrets (`.env`, private keys, credentials), which the agent reads
//! only with the user's permission: `read_file` asks first (like a command; "always" keeps a
//! `read_file:<path>` tool rule), and `search` never looks inside them. What goes to the model
//! leaves the machine, and `.gitignore` hides a file from listings, not from a read by its path.
//!
//! Templates that hold no values (`.env.example`, `.env.sample`, `.env.template`) and public
//! keys (`id_rsa.pub`) are plain files.

use std::path::Path;

/// What a rule names to let `read_file` open the secret file `path` (`read_file:.env`).
pub fn rule(path: &str) -> String {
    format!("read_file:{path}")
}

/// Whether the file at `path` (any form: `config/.env.local`, an absolute path) usually holds
/// secrets, by its name.
pub fn is_secret(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let name = name.to_ascii_lowercase();
    if name.starts_with(".env") {
        return ![".example", ".sample", ".template", ".dist"].iter().any(|t| name.ends_with(t));
    }
    const NAMES: [&str; 10] = [
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        ".netrc",
        ".npmrc",
        ".pypirc",
        "credentials.json",
        "secrets.json",
        "service-account.json",
    ];
    if NAMES.contains(&name.as_str()) {
        return true;
    }
    const EXTENSIONS: [&str; 5] = ["pem", "key", "p12", "pfx", "keystore"];
    Path::new(&name).extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_keys_and_credentials_are_secret_and_templates_are_not() {
        for secret in [
            ".env",
            ".env.local",
            "config/.env.production",
            "id_rsa",
            "certs/server.pem",
            "tls.key",
            "store.p12",
            "credentials.json",
            ".npmrc",
        ] {
            assert!(is_secret(Path::new(secret)), "{secret}");
        }
        // Only Windows reads `\` as a separator.
        #[cfg(windows)]
        assert!(is_secret(Path::new("C:\\code\\app\\.ENV")));
        for plain in
            [".env.example", ".env.sample", "id_rsa.pub", "src/main.rs", "README.md", "keys.rs", "environment.ts"]
        {
            assert!(!is_secret(Path::new(plain)), "{plain}");
        }
    }
}
