use std::fs;
use std::path::{Path, PathBuf};

use crate::error::HarnessError;

pub const CLIENT_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReadTextFileRequest {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReadTextFileResponse {
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WriteTextFileRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WriteTextFileResponse {}

pub fn is_inside_root(root: &Path, candidate: &Path) -> bool {
    candidate.strip_prefix(root).is_ok()
}

pub fn resolve_client_file_path(
    allowed_roots: &[PathBuf],
    request_path: &str,
) -> Result<PathBuf, HarnessError> {
    let raw_path = Path::new(request_path);
    let resolved = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else if let Some(first_root) = allowed_roots.first() {
        first_root.join(raw_path)
    } else {
        raw_path.to_path_buf()
    };

    let real = canonicalize_ancestor(&resolved);

    let canonical_roots: Vec<PathBuf> = allowed_roots
        .iter()
        .map(|r| fs::canonicalize(r).unwrap_or_else(|_| r.clone()))
        .collect();

    let inside = canonical_roots.iter().any(|root| is_inside_root(root, &real));
    if !inside {
        return Err(HarnessError::Validation(format!(
            "Path '{}' is outside the session workspace.",
            request_path
        )));
    }

    Ok(real)
}

fn canonicalize_ancestor(path: &Path) -> PathBuf {
    let mut current = path;
    let mut suffix = Vec::new();
    while !current.exists() {
        if let Some(file_name) = current.file_name() {
            suffix.push(file_name);
        }
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }
    let mut resolved = fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    resolved
}

pub fn read_client_text_file(
    allowed_roots: &[PathBuf],
    request: &ReadTextFileRequest,
) -> Result<ReadTextFileResponse, HarnessError> {
    let file_path = resolve_client_file_path(allowed_roots, &request.path)?;
    let meta = fs::metadata(&file_path).map_err(|_| {
        HarnessError::Validation(format!("File '{}' not found.", request.path))
    })?;

    if !meta.is_file() || meta.len() > CLIENT_FILE_MAX_BYTES {
        return Err(HarnessError::Validation(format!(
            "File '{}' is not a readable text file under {} bytes.",
            request.path, CLIENT_FILE_MAX_BYTES
        )));
    }

    let text = fs::read_to_string(&file_path).map_err(|e| {
        HarnessError::Validation(format!("Could not read '{}': {}", request.path, e))
    })?;

    if request.line.is_none() && request.limit.is_none() {
        return Ok(ReadTextFileResponse { content: text });
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let start = request
        .line
        .map(|l| l.saturating_sub(1))
        .unwrap_or(0)
        .min(lines.len());
    let end = match request.limit {
        Some(limit) => (start + limit).min(lines.len()),
        None => lines.len(),
    };

    let content = lines[start..end].join("\n");
    Ok(ReadTextFileResponse { content })
}

pub fn write_client_text_file(
    allowed_roots: &[PathBuf],
    request: &WriteTextFileRequest,
) -> Result<WriteTextFileResponse, HarnessError> {
    if request.content.len() as u64 > CLIENT_FILE_MAX_BYTES {
        return Err(HarnessError::Validation(format!(
            "Payload for file '{}' exceeds maximum allowed size of {} bytes.",
            request.path, CLIENT_FILE_MAX_BYTES
        )));
    }

    let file_path = resolve_client_file_path(allowed_roots, &request.path)?;

    // If destination path already exists as a symlink, verify its target is inside allowed roots
    if let Ok(meta) = fs::symlink_metadata(&file_path)
        && meta.file_type().is_symlink()
        && let Ok(target) = fs::canonicalize(&file_path)
    {
        let canonical_roots: Vec<PathBuf> = allowed_roots
            .iter()
            .map(|r| fs::canonicalize(r).unwrap_or_else(|_| r.clone()))
            .collect();
        if !canonical_roots.iter().any(|r| is_inside_root(r, &target)) {
            return Err(HarnessError::Validation(format!(
                "Symlink target for '{}' escapes session workspace.",
                request.path
            )));
        }
    }

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(HarnessError::Io)?;
    }
    fs::write(&file_path, &request.content).map_err(HarnessError::Io)?;
    Ok(WriteTextFileResponse {})
}
