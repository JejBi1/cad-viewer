//! Size report: bytes per component, raw and LZMA-compressed (xz preset 9e, one solid stream per component).
use crate::util;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use xz_native::{EncoderOptions, LzmaOptions, PRESET_EXTREME, XzWriter};

/// A group of files reported together.
pub struct Component {
    pub name: String,
    pub note: String,
    pub files: Vec<(PathBuf, String)>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Row {
    pub name: String,
    pub note: String,
    pub files: usize,
    pub raw: u64,
    pub lzma: u64,
}

/// Counts the bytes that pass through; the compressed data itself is not kept.
struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Size of the files as one solid xz stream (preset 9 + extreme, dictionary no larger than the data).
pub fn lzma_size(files: &[(PathBuf, String)]) -> Result<u64> {
    let mut raw = 0u64;
    for (p, _) in files {
        raw += fs::metadata(p).with_context(|| p.display().to_string())?.len();
    }
    if raw == 0 {
        return Ok(0);
    }
    let dict = raw.next_power_of_two().clamp(4096, 64 << 20) as u32;
    let lzma = LzmaOptions::preset(9 | PRESET_EXTREME)
        .map_err(|e| anyhow::anyhow!("xz preset: {e:?}"))?
        .dict_size(dict);
    let opts = EncoderOptions::default().lzma(lzma);
    let mut w = XzWriter::with_options(Counter(0), opts).map_err(|e| anyhow::anyhow!("xz encoder: {e:?}"))?;
    let mut buf = vec![0u8; 1 << 20];
    for (p, _) in files {
        let mut f = fs::File::open(p).with_context(|| p.display().to_string())?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
        }
    }
    let c = w.finish()?;
    Ok(c.0)
}

pub fn measure(components: &[Component]) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for c in components {
        let raw: u64 = c.files.iter().map(|(p, _)| fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum();
        eprintln!("size report: {} ({} files, {}) ...", c.name, c.files.len(), util::mb(raw));
        rows.push(Row { name: c.name.clone(), note: c.note.clone(), files: c.files.len(), raw, lzma: lzma_size(&c.files)? });
    }
    Ok(rows)
}

/// The total as one solid stream over all components (what a solid installer archive does).
pub fn measure_total(components: &[Component]) -> Result<Row> {
    let files: Vec<(PathBuf, String)> = components.iter().flat_map(|c| c.files.clone()).collect();
    let raw: u64 = files.iter().map(|(p, _)| fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum();
    Ok(Row { name: "all components, one solid stream".into(), note: String::new(), files: files.len(), raw, lzma: lzma_size(&files)? })
}

/// Fixed-width text table.
pub fn render(rows: &[Row], total: &Row) -> String {
    let mut s = String::new();
    s.push_str(&format!("{:<44} {:>5} {:>13} {:>13} {:>7}\n", "component", "files", "raw bytes", "xz bytes", "ratio"));
    s.push_str(&format!("{}\n", "-".repeat(87)));
    for r in rows {
        s.push_str(&format!(
            "{:<44} {:>5} {:>13} {:>13} {:>6.1}%\n",
            r.name,
            r.files,
            group(r.raw),
            group(r.lzma),
            if r.raw == 0 { 0.0 } else { 100.0 * r.lzma as f64 / r.raw as f64 }
        ));
    }
    if !rows.is_empty() {
        s.push_str(&format!("{}\n", "-".repeat(87)));
    }
    s.push_str(&format!(
        "{:<44} {:>5} {:>13} {:>13} {:>6.1}%\n",
        total.name,
        total.files,
        group(total.raw),
        group(total.lzma),
        if total.raw == 0 { 0.0 } else { 100.0 * total.lzma as f64 / total.raw as f64 }
    ));
    s
}

/// 12345678 -> "12 345 678"
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1 000");
        assert_eq!(group(12_345_678), "12 345 678");
    }

    #[test]
    fn lzma_of_repeated_data_is_tiny() {
        let dir = std::env::temp_dir().join(format!("xtask-sizes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![7u8; 1 << 20]).unwrap();
        let n = lzma_size(&[(f.clone(), "a.bin".into())]).unwrap();
        assert!(n > 0 && n < 1000, "{n}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
