//! The head: the stable part of every request (log-context.md §3.8.2). Built at boot and on
//! `initctl reload`, never per turn. Nothing volatile goes in it.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The head, normalized. `tools` are the registry's `ToolSpec` objects `{description, input_schema,
/// name}` (body/tools.md §4.1), sorted byte-wise by `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    /// `GENESIS.md` + `\n# CONSTITUTION.md\n\n` + `CONSTITUTION.md`.
    pub core: String,
    /// `# SELF.md\n\n` + `SELF.md` + `\n# Skills\n\n` + the skills index.
    pub self_: String,
    pub tools: Vec<Value>,
    /// Skills without front matter (a `display` warning each).
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum HeadError {
    /// A head file is not valid UTF-8: the reload is refused (or boot fails).
    NotUtf8(PathBuf),
    Io(PathBuf, std::io::Error),
    BadTool(String),
}

impl std::fmt::Display for HeadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadError::NotUtf8(p) => write!(f, "head: {} is not valid UTF-8", p.display()),
            HeadError::Io(p, e) => write!(f, "head: {}: {e}", p.display()),
            HeadError::BadTool(m) => write!(f, "head: tool spec: {m}"),
        }
    }
}

impl std::error::Error for HeadError {}

/// Strip a UTF-8 BOM, CRLF → LF, strip trailing whitespace at EOF, append exactly one `\n`.
pub fn normalize(text: &str) -> String {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut s = t.replace("\r\n", "\n");
    s.truncate(s.trim_end().len());
    s.push('\n');
    s
}

fn read_text(p: &Path) -> Result<String, HeadError> {
    let bytes = std::fs::read(p).map_err(|e| HeadError::Io(p.to_path_buf(), e))?;
    String::from_utf8(bytes).map_err(|_| HeadError::NotUtf8(p.to_path_buf()))
}

/// `name:` and `description:` single-line values between leading `---` lines.
pub fn front_matter(text: &str) -> Option<(String, String)> {
    let t = normalize(text);
    let mut lines = t.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let (mut name, mut desc) = (None, None);
    for line in lines {
        if line.trim_end() == "---" {
            return Some((name?, desc?));
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string()).filter(|s| !s.is_empty());
        } else if let Some(v) = line.strip_prefix("description:") {
            desc = Some(v.trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// The skills index lines, sorted byte-wise by name, and warnings for skills without front matter.
pub fn skills_index(head_root: &Path) -> Result<(String, Vec<String>), HeadError> {
    let dir = head_root.join("skills");
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), warnings)),
        Err(e) => return Err(HeadError::Io(dir, e)),
    };
    let mut dirs: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    dirs.sort();
    for d in dirs {
        let p = dir.join(&d).join("SKILL.md");
        if !p.exists() {
            continue;
        }
        let text = read_text(&p)?;
        match front_matter(&text) {
            Some((name, desc)) => rows.push((name.clone(), format!("- {name}: {desc} (body/skills/{d}/SKILL.md)"))),
            None => warnings.push(format!("skill body/skills/{d}/SKILL.md has no front matter (name, description); it is not in the skills index")),
        }
    }
    rows.sort_by(|a, b| {
        a.0.as_bytes()
            .cmp(b.0.as_bytes())
            .then_with(|| a.1.cmp(&b.1))
    });
    let mut out = String::new();
    for (_, line) in rows {
        out.push_str(&line);
        out.push('\n');
    }
    Ok((out, warnings))
}

/// Sorts and checks tool specs (`name`, `description`, `input_schema`).
pub fn sort_tools(mut tools: Vec<Value>) -> Result<Vec<Value>, HeadError> {
    for t in &tools {
        let ok = t.get("name").and_then(Value::as_str).is_some()
            && t.get("description").and_then(Value::as_str).is_some()
            && t.get("input_schema").map(Value::is_object).unwrap_or(false);
        if !ok {
            return Err(HeadError::BadTool(t.to_string()));
        }
    }
    tools.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap()
            .as_bytes()
            .cmp(b["name"].as_str().unwrap().as_bytes())
    });
    Ok(tools)
}

/// Builds the head. `head_root` is `$INIT_HOME/body` (or the LKG tree in safe mode); `constitution` is
/// `$INIT_HOME/kernel/CONSTITUTION.md`.
pub fn build_head(
    head_root: &Path,
    constitution: &Path,
    tools: Vec<Value>,
) -> Result<Head, HeadError> {
    let genesis = normalize(&read_text(&head_root.join("GENESIS.md"))?);
    let cons = normalize(&read_text(constitution)?);
    let self_md = normalize(&read_text(&head_root.join("SELF.md"))?);
    let (skills, warnings) = skills_index(head_root)?;
    Ok(Head {
        core: format!("{genesis}\n# CONSTITUTION.md\n\n{cons}"),
        self_: format!("# SELF.md\n\n{self_md}\n# Skills\n\n{skills}"),
        tools: sort_tools(tools)?,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize("\u{feff}a\r\nb  \n\n \t"), "a\nb\n");
        assert_eq!(normalize(""), "\n");
        assert_eq!(normalize("x\n"), "x\n");
    }

    #[test]
    fn front_matter_rules() {
        assert_eq!(
            front_matter("---\nname: pty\ndescription: Run a PTY session.\n---\nbody"),
            Some(("pty".into(), "Run a PTY session.".into()))
        );
        assert_eq!(front_matter("# no front matter"), None);
        assert_eq!(front_matter("---\nname: x\n---\n"), None);
    }
}
