//! Read-only filesystem policy matching Electron's filesystem handlers.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_SIZE: u64 = 1_048_576;

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}

pub fn directory(path: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            let directory = item.file_type().ok()?.is_dir();
            if name.starts_with('.')
                || (directory && matches!(name.as_str(), "node_modules" | "__pycache__"))
            {
                return None;
            }
            // Electron stats the target, skipping broken/inaccessible links.
            std::fs::metadata(item.path()).ok()?;
            Some(Entry {
                path: item.path(),
                name,
                directory,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| (!e.directory, e.name.to_lowercase(), e.name.clone()));
    Ok(entries)
}

pub struct Preview {
    pub content: String,
    pub language: &'static str,
    pub size: usize,
}

pub fn preview(path: &Path) -> Result<Preview, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "webp"
            | "svg"
            | "mp3"
            | "mp4"
            | "wav"
            | "ogg"
            | "webm"
            | "avi"
            | "mov"
            | "zip"
            | "tar"
            | "gz"
            | "7z"
            | "rar"
            | "bz2"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
            | "node"
            | "wasm"
            | "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "eot"
            | "sqlite"
            | "db"
            | "lock"
    ) {
        return Err("Binary file — cannot preview".into());
    }
    // Refuse named pipes/devices before open, which can itself block.
    if !std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Cannot preview a non-regular file".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Cannot preview a non-regular file".into());
    }
    let mut data = Vec::new();
    // Bound the read too: metadata alone races a growing file.
    file.take(MAX_SIZE + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if metadata.len() > MAX_SIZE || data.len() as u64 > MAX_SIZE {
        return Err(format!(
            "File too large ({:.1} MB). Max: 1 MB.",
            metadata.len().max(data.len() as u64) as f64 / MAX_SIZE as f64
        ));
    }
    Ok(Preview {
        size: data.len(),
        content: String::from_utf8_lossy(&data).into_owned(),
        language: language(path),
    })
}

pub fn language(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    match name.as_str() {
        "dockerfile" | ".dockerfile" => return "dockerfile",
        "makefile" => return "makefile",
        ".gitignore" | ".gitattributes" => return "gitignore",
        ".env" => return "shell",
        _ => {}
    }
    match path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
        .as_str()
    {
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "json" => "json",
        "md" => "markdown",
        "css" => "css",
        "scss" => "scss",
        "html" => "html",
        "xml" => "xml",
        "yml" | "yaml" => "yaml",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "rs" => "rust",
        "cs" => "csharp",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        "sh" | "bash" | "env" => "shell",
        "ps1" | "psm1" => "powershell",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "toml" => "toml",
        "ini" => "ini",
        "dockerfile" => "dockerfile",
        "tf" | "hcl" => "hcl",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        _ => "plaintext",
    }
}

pub fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < MAX_SIZE as usize {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / MAX_SIZE as f64)
    }
}
