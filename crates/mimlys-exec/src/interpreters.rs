//! Pinned interpreter builds (external design §12; REQ-SBX-001, REQ-SBX-007,
//! REQ-ARCH-007). An interpreter is a wasip1 module identified by its
//! SHA-256; the binary knows, per language, the artifact mimlys ships and
//! its hash; a module whose hash differs from the configured one is never
//! loaded. Modules live under `<data_dir>/interpreters/<sha256>.wasm`.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One pinned interpreter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    /// The `language` value of `run_script`.
    pub language: &'static str,
    /// The script file name inside `/tmp`.
    pub script_name: &'static str,
    /// Arguments placed before the script path (interpreter switches).
    pub args: &'static [&'static str],
    /// The upstream project and version, for the status and the recipe.
    pub build: &'static str,
    /// Where the artifact is published.
    pub url: &'static str,
    /// Its SHA-256, lowercase hex.
    pub sha256: &'static str,
}

/// The interpreters mimlys ships (`tools/interpreters/` holds the recipes).
pub const PINNED: [Pinned; 1] = [Pinned {
    language: "javascript",
    script_name: "main.js",
    // The wasip1 build has no `std`/`os` modules: scripts read stdin and args and print; files are the file tools' job.
    args: &[],
    build: "quickjs-ng v0.17.0 (qjs-wasi.wasm, wasip1)",
    url: "https://github.com/quickjs-ng/quickjs/releases/download/v0.17.0/qjs-wasi.wasm",
    sha256: "42a732a676ec2d93488c19411e0fad283bf72658fdad746f089914b523c783b1",
}];

/// The pinned entry for a language.
pub fn pinned(language: &str) -> Option<&'static Pinned> {
    PINNED.iter().find(|p| p.language == language)
}

/// Lowercase hex SHA-256 of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A configured interpreter: where its module is and which hash it must have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configured {
    /// Language.
    pub language: String,
    /// Module path.
    pub module: PathBuf,
    /// Expected SHA-256.
    pub sha256: String,
    /// Script file name inside `/tmp`.
    pub script_name: String,
}

/// Why an interpreter cannot be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InterpreterError {
    /// The module file is missing.
    #[error(
        "interpreter module for {language} is not installed at {path}; run `mimlys interpreters install {language}`"
    )]
    Missing {
        /// Language.
        language: String,
        /// Path looked at.
        path: String,
    },
    /// The file's hash is not the configured one.
    #[error(
        "interpreter module for {language} has hash {actual}, configured {expected}; it is not loaded"
    )]
    HashMismatch {
        /// Language.
        language: String,
        /// Expected.
        expected: String,
        /// Found.
        actual: String,
    },
    /// Larger than LIM-S5.
    #[error("interpreter module for {language} is {bytes} bytes, above LIM-S5")]
    TooLarge {
        /// Language.
        language: String,
        /// Size.
        bytes: u64,
    },
    /// Cannot be read.
    #[error("interpreter module for {language} cannot be read: {detail}")]
    Unreadable {
        /// Language.
        language: String,
        /// Detail.
        detail: String,
    },
}

/// The default module path of a pinned interpreter under the data directory.
pub fn default_module_path(data_dir: &Path, sha256: &str) -> PathBuf {
    data_dir.join("interpreters").join(format!("{sha256}.wasm"))
}

/// Reads and verifies a configured module (REQ-SBX-007, REQ-SBX-012).
pub fn load_verified(configured: &Configured) -> Result<Vec<u8>, InterpreterError> {
    let language = configured.language.clone();
    let meta = std::fs::metadata(&configured.module).map_err(|_| InterpreterError::Missing {
        language: language.clone(),
        path: configured.module.to_string_lossy().into_owned(),
    })?;
    if meta.len() > mimlys_core::limits::LIM_S5_MODULE_BYTES {
        return Err(InterpreterError::TooLarge {
            language,
            bytes: meta.len(),
        });
    }
    let bytes = std::fs::read(&configured.module).map_err(|e| InterpreterError::Unreadable {
        language: language.clone(),
        detail: e.kind().to_string(),
    })?;
    let actual = sha256_hex(&bytes);
    if actual != configured.sha256.to_ascii_lowercase() {
        return Err(InterpreterError::HashMismatch {
            language,
            expected: configured.sha256.clone(),
            actual,
        });
    }
    Ok(bytes)
}

/// Verifies bytes against a hash and stores them at the default path, owner-only.
pub fn install(
    data_dir: &Path,
    language: &str,
    expected_sha256: &str,
    bytes: &[u8],
) -> Result<PathBuf, InterpreterError> {
    let actual = sha256_hex(bytes);
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(InterpreterError::HashMismatch {
            language: language.to_owned(),
            expected: expected_sha256.to_owned(),
            actual,
        });
    }
    if bytes.len() as u64 > mimlys_core::limits::LIM_S5_MODULE_BYTES {
        return Err(InterpreterError::TooLarge {
            language: language.to_owned(),
            bytes: bytes.len() as u64,
        });
    }
    let dir = data_dir.join("interpreters");
    std::fs::create_dir_all(&dir).map_err(|e| InterpreterError::Unreadable {
        language: language.to_owned(),
        detail: e.kind().to_string(),
    })?;
    let path = default_module_path(data_dir, &actual);
    let tmp = dir.join(format!("{actual}.wasm.part"));
    std::fs::write(&tmp, bytes).map_err(|e| InterpreterError::Unreadable {
        language: language.to_owned(),
        detail: e.kind().to_string(),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).map_err(|e| InterpreterError::Unreadable {
        language: language.to_owned(),
        detail: e.kind().to_string(),
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests;
