//! `read` (tools.md §3.2): `cat -n` text with paging, image blocks, binary and size refusals.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;

use serde_json::{Map, Value};

use super::args::{check_fields, opt_int, req_str};
use super::{
    Limits, PathCx, Ran, ToolCx, ToolError, commas, io_error, is_directory, resolve, text_result,
};
use crate::model::ContentBlock;

#[derive(Clone, Debug)]
pub struct ReadArgs {
    /// As the model wrote it (messages and hints show this).
    pub path: String,
    pub abs: PathBuf,
    pub offset: u64,
    pub limit: usize,
}

pub fn parse(m: &Map<String, Value>, cx: &PathCx, lim: &Limits) -> Result<ReadArgs, ToolError> {
    check_fields(m, &["limit", "offset", "path"], "")?;
    let path = req_str(m, "path", "")?;
    let offset = opt_int(m, "offset", "")?.unwrap_or(1);
    if offset < 1 {
        return Err(ToolError::invalid("offset: must be at least 1"));
    }
    let limit = opt_int(m, "limit", "")?.unwrap_or(lim.read_lines as i64);
    if limit < 1 {
        return Err(ToolError::invalid("limit: must be at least 1"));
    }
    let abs = resolve(&path, cx)?;
    Ok(ReadArgs {
        path,
        abs,
        offset: offset as u64,
        limit: (limit as u64).min(lim.read_lines as u64) as usize,
    })
}

/// Magic numbers of the four formats every provider accepts.
pub fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.len() >= 6 && &b[..4] == b"GIF8" && (b[4] == b'7' || b[4] == b'9') && b[5] == b'a' {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// RFC 4648 base64 with padding, std only.
pub fn base64(data: &[u8]) -> String {
    crate::context::render::base64(data)
}

fn not_found(a: &ReadArgs) -> ToolError {
    let e = ToolError::new("not_found", format!("{} does not exist", a.path));
    let Some(parent) = a.abs.parent() else {
        return e;
    };
    if !parent.is_dir() {
        return e.hint(format!(
            "the parent directory {} does not exist either",
            shown_parent(&a.path).unwrap_or_else(|| parent.display().to_string())
        ));
    }
    let name = a
        .abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Deviation (tools.md §3.2): the doc's own example (`src/lop.rs` → `src/loop.rs`) does not
    // share three leading characters, so a small edit distance qualifies too.
    let stem: String = name.chars().take(3).collect();
    let mut similar: Vec<(usize, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(parent) {
        for ent in rd.take(1_000).flatten() {
            let n = ent.file_name().to_string_lossy().into_owned();
            let d = edit_distance(&n, &name);
            if (stem.chars().count() == 3 && n.starts_with(&stem)) || d <= 2 {
                similar.push((d, n));
            }
        }
    }
    similar.sort();
    similar.truncate(5);
    if similar.is_empty() {
        e.hint("parent exists; no similar names in it")
    } else {
        let prefix = shown_parent(&a.path)
            .map(|p| format!("{p}/"))
            .unwrap_or_default();
        let list: Vec<String> = similar
            .iter()
            .map(|(_, n)| format!("{prefix}{n}"))
            .collect();
        e.hint(format!("parent exists; similar: {}", list.join(", ")))
    }
}

/// Levenshtein distance over chars (names are short; at most 1,000 are compared).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize)
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn shown_parent(p: &str) -> Option<String> {
    let t = p.trim_end_matches('/');
    t.rfind('/').map(|i| {
        if i == 0 {
            "/".to_string()
        } else {
            t[..i].to_string()
        }
    })
}

/// One line from a `BufRead`, keeping at most `keep` bytes of it (no `\n`).
struct Line {
    kept: Vec<u8>,
    /// Bytes in the line, without `\n` and without a `\r` right before it.
    total: u64,
}

/// Returns `None` at EOF. Memory stays bounded for arbitrarily long lines.
fn next_line<R: BufRead>(r: &mut R, keep: usize) -> io::Result<Option<Line>> {
    let mut kept = Vec::new();
    let mut total: u64 = 0;
    let mut any = false;
    let mut last_cr = false;
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            break;
        }
        any = true;
        let (chunk, done) = match buf.iter().position(|&c| c == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !chunk.is_empty() {
            let room = keep.saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..room.min(chunk.len())]);
            total += chunk.len() as u64;
            last_cr = *chunk.last().unwrap() == b'\r';
        }
        let n = done.unwrap_or(buf.len());
        r.consume(n);
        if done.is_some() {
            if last_cr {
                total -= 1;
                if kept.len() as u64 > total {
                    kept.truncate(total as usize);
                }
            }
            return Ok(Some(Line { kept, total }));
        }
    }
    Ok(if any {
        Some(Line { kept, total })
    } else {
        None
    })
}

/// Counts the lines left in `r`, scanning at most `cap` bytes. `(lines, complete)`.
fn count_rest<R: Read>(r: &mut R, cap: u64) -> io::Result<(u64, bool)> {
    let mut buf = vec![0u8; 64 << 10];
    let mut lines = 0u64;
    let mut scanned = 0u64;
    let mut pending = false; // bytes after the last '\n'
    loop {
        if scanned >= cap {
            return Ok((lines, false));
        }
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        scanned += n as u64;
        let nl = buf[..n].iter().filter(|&&c| c == b'\n').count() as u64;
        lines += nl;
        pending = buf[n - 1] != b'\n';
    }
    Ok((lines + pending as u64, true))
}

pub fn run(a: &ReadArgs, cx: &ToolCx, lim: &Limits) -> Result<Ran, ToolError> {
    #[cfg(test)]
    if a.path.ends_with("__init_test_panic__") {
        panic!("test-injected panic in read");
    }
    let md = match std::fs::metadata(&a.abs) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(not_found(a)),
        Err(e) => return Err(io_error(&a.path, &e)),
    };
    if md.is_dir() {
        return Err(is_directory(&a.path));
    }
    let f = File::open(&a.abs).map_err(|e| io_error(&a.path, &e))?;
    let mut r = BufReader::with_capacity(64 << 10, f);
    // Sniff the first 8,192 bytes without consuming them.
    let mut head = Vec::with_capacity(8_192);
    (&mut r)
        .take(8_192)
        .read_to_end(&mut head)
        .map_err(|e| io_error(&a.path, &e))?;
    if let Some(mt) = sniff_image(&head) {
        return image(a, cx, lim, mt, md.len());
    }
    if head.contains(&0) {
        return Err(ToolError::new(
            "binary_file",
            format!("{} is binary (contains NUL bytes)", a.path),
        )
        .hint(format!(
            "inspect with bash: file {p}; xxd {p} | head -50",
            p = a.path
        )));
    }
    let mut r = io::Cursor::new(head).chain(r);
    let mut r = BufReader::with_capacity(64 << 10, &mut r);
    text(a, lim, &mut r)
}

fn image(a: &ReadArgs, cx: &ToolCx, lim: &Limits, mt: &str, size: u64) -> Result<Ran, ToolError> {
    if !cx.images {
        return Ok(Ran {
            result: text_result(
                cx.call_id,
                format!(
                    "[image file: {mt}, {} bytes; the current model ({}) has no image input]",
                    commas(size),
                    cx.model_id
                ),
            ),
            summary: "image (not shown)".into(),
        });
    }
    if size > lim.image_bytes {
        return Err(ToolError::new(
            "too_large",
            format!(
                "image is {} bytes; read returns images of at most {} bytes",
                commas(size),
                commas(lim.image_bytes)
            ),
        )
        .hint("downscale with bash, e.g. sips -Z 1568 <in> --out <out> (macOS) or magick <in> -resize 1568x1568 <out>"));
    }
    let bytes = std::fs::read(&a.abs).map_err(|e| io_error(&a.path, &e))?;
    let mut res = text_result(
        cx.call_id,
        format!("[image: {mt}, {} bytes]", commas(bytes.len() as u64)),
    );
    res.content.push(ContentBlock::Image {
        media_type: mt.to_string(),
        data_b64: base64(&bytes),
    });
    Ok(Ran {
        result: res,
        summary: "image".into(),
    })
}

/// The largest char boundary ≤ `max` in possibly invalid UTF-8 (never splits a sequence).
fn cut_point(b: &[u8], max: usize) -> usize {
    let mut i = max.min(b.len());
    while i > 0 && i < b.len() && (b[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

fn text<R: BufRead>(a: &ReadArgs, lim: &Limits, r: &mut R) -> Result<Ran, ToolError> {
    let ioe = |e: io::Error| io_error(&a.path, &e);
    // Skip to `offset`.
    let mut n: u64 = 0;
    while n + 1 < a.offset {
        if next_line(r, 0).map_err(ioe)?.is_none() {
            break;
        }
        n += 1;
    }
    let mut out = String::new();
    let mut emitted = 0usize;
    let mut lossy = false;
    let mut leftover = 0u64; // a line read but not emitted (byte cap)
    while emitted < a.limit {
        let Some(line) = next_line(r, lim.read_line_bytes + 4).map_err(ioe)? else {
            break;
        };
        let ln = n + 1;
        let mut s = if line.total as usize > lim.read_line_bytes {
            let cut = cut_point(&line.kept, lim.read_line_bytes);
            let part = &line.kept[..cut];
            lossy |= std::str::from_utf8(part).is_err();
            format!(
                "{}… [line truncated: {} bytes; see it with bash: sed -n '{ln}p' {} | cut -c {}-{}]",
                String::from_utf8_lossy(part),
                commas(line.total),
                a.path,
                lim.read_line_bytes + 1,
                lim.read_line_bytes * 2
            )
        } else {
            lossy |= std::str::from_utf8(&line.kept).is_err();
            String::from_utf8_lossy(&line.kept).into_owned()
        };
        s = format!("{ln:>6}\t{s}\n");
        if emitted > 0 && out.len() + s.len() > lim.read_bytes {
            leftover = 1;
            break;
        }
        out.push_str(&s);
        emitted += 1;
        n += 1;
    }
    if emitted == 0 {
        // Offset 1 and nothing to read.
        if n == 0 {
            return Ok(Ran {
                result: text_result("", "[empty file]".into()),
                summary: "0 lines".into(),
            });
        }
        return Err(ToolError::invalid(format!(
            "offset {} is past the end of the file ({n} lines)",
            a.offset
        )));
    }
    let (rest, complete) = count_rest(r, lim.read_scan).map_err(ioe)?;
    let rest = rest + leftover;
    if lossy {
        out.push_str("[file is not valid UTF-8; invalid bytes shown as U+FFFD]\n");
    }
    if rest > 0 {
        let total = n + rest;
        let t = if complete {
            total.to_string()
        } else {
            format!("more than {total}")
        };
        let s = a.offset;
        out.push_str(&format!(
            "\n[Showing lines {s}-{n} of {t}. Use offset={} to continue.]",
            n + 1
        ));
    }
    Ok(Ran {
        result: text_result("", out),
        summary: format!("{emitted} line{}", if emitted == 1 { "" } else { "s" }),
    })
}
