//! TrueType outlines for text: font lookup, glyph paths, text metrics (ezdxf `fonts` + `TTFontRenderer`).
//!
//! SHX fonts and fonts that are not installed fall back to the default font, as in ezdxf: Arial (or the first
//! available face of the ezdxf default list), finally the DejaVu Sans that is embedded in the crate.
use crate::geom::{Elems, V2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static EMBEDDED: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

const DEFAULT_FONTS: [&str; 7] =
    ["arialuni.ttf", "arial unicode.ttf", "arial.ttf", "dejavusanscondensed.ttf", "dejavusans.ttf", "liberationsans-regular.ttf", "opensans-regular.ttf"];

/// Where fonts are searched.
#[derive(Clone, Debug)]
pub struct FontOptions {
    /// Search the system font folders (Windows Fonts, /usr/share/fonts ...).
    pub system_fonts: bool,
    /// Additional folders searched first.
    pub dirs: Vec<PathBuf>,
}

impl Default for FontOptions {
    fn default() -> Self {
        FontOptions { system_fonts: true, dirs: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Measurements {
    pub baseline: f64,
    pub cap_height: f64,
    pub x_height: f64,
    pub descender: f64,
}

#[derive(Debug)]
pub struct Glyph {
    pub elems: Elems,
    pub advance: f64,
}

pub struct Font {
    pub name: String,
    data: Arc<Vec<u8>>,
    index: u32,
    pub units: f64,
    /// Measurements in font units.
    pub m: Measurements,
    glyphs: Mutex<HashMap<char, Arc<Glyph>>>,
    pub embedded: bool,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Font({})", self.name)
    }
}

struct PathPen {
    e: Elems,
    start: V2,
    cur: V2,
    open: bool,
}

impl ttf_parser::OutlineBuilder for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close_contour();
        let p = [x as f64, y as f64];
        self.e.move_to(p);
        self.start = p;
        self.cur = p;
        self.open = true;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = [x as f64, y as f64];
        self.e.line_to(p);
        self.cur = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p = [x as f64, y as f64];
        self.e.quad_to([x1 as f64, y1 as f64], p);
        self.cur = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p = [x as f64, y as f64];
        self.e.curve_to([x1 as f64, y1 as f64], [x2 as f64, y2 as f64], p);
        self.cur = p;
    }
    fn close(&mut self) {
        self.close_contour();
    }
}

impl PathPen {
    fn close_contour(&mut self) {
        if self.open && (self.cur[0] != self.start[0] || self.cur[1] != self.start[1]) {
            self.e.line_to(self.start);
        }
        self.open = false;
    }
}

impl Font {
    fn from_data(name: &str, data: Arc<Vec<u8>>, index: u32, embedded: bool) -> Option<Font> {
        let face = ttf_parser::Face::parse(&data, index).ok()?;
        let units = face.units_per_em() as f64;
        let mut f = Font {
            name: name.to_string(),
            data: data.clone(),
            index,
            units,
            m: Measurements { baseline: 0.0, cap_height: units * 0.7, x_height: units * 0.5, descender: units * 0.2 },
            glyphs: Mutex::new(HashMap::new()),
            embedded,
        };
        let pts = |c: char| -> Option<[f64; 4]> {
            let g = f.glyph(c);
            g.elems.bounds()
        };
        let bx = pts('x');
        let ba = pts('A');
        let bp = pts('p');
        if let (Some(bx), Some(ba), Some(bp)) = (bx, ba, bp) {
            let baseline = bx[1];
            let m = Measurements {
                baseline,
                x_height: bx[3] - baseline,
                cap_height: ba[3] - baseline,
                descender: baseline - bp[1],
            };
            if m.cap_height > 0.0 {
                f.m = m;
            }
        }
        Some(f)
    }

    /// Glyph outline in font units (the `.notdef` glyph for characters the font does not have).
    pub fn glyph(&self, c: char) -> Arc<Glyph> {
        if let Some(g) = self.glyphs.lock().ok().and_then(|m| m.get(&c).cloned()) {
            return g;
        }
        let g = Arc::new(self.build_glyph(c));
        if let Ok(mut m) = self.glyphs.lock() {
            m.insert(c, g.clone());
        }
        g
    }

    fn build_glyph(&self, c: char) -> Glyph {
        let Ok(face) = ttf_parser::Face::parse(&self.data, self.index) else {
            return Glyph { elems: Elems::new(), advance: 0.0 };
        };
        let gid = face.glyph_index(c).unwrap_or(ttf_parser::GlyphId(0));
        let mut pen = PathPen { e: Elems::new(), start: [0.0; 2], cur: [0.0; 2], open: false };
        let _ = face.outline_glyph(gid, &mut pen);
        pen.close_contour();
        let advance = face.glyph_hor_advance(gid).map(|a| a as f64).unwrap_or(0.0);
        Glyph { elems: pen.e, advance }
    }

    pub fn scaling(&self, cap_height: f64) -> f64 {
        cap_height / self.m.cap_height
    }

    /// Measurements for a cap height in drawing units.
    pub fn measurements(&self, cap_height: f64) -> Measurements {
        let s = self.scaling(cap_height);
        Measurements {
            baseline: self.m.baseline * s,
            cap_height: self.m.cap_height * s,
            x_height: self.m.x_height * s,
            descender: self.m.descender * s,
        }
    }

    /// Width of the text in drawing units (0 for blank text), no kerning.
    pub fn text_width(&self, text: &str, cap_height: f64, width_factor: f64) -> f64 {
        if text.trim().is_empty() {
            return 0.0;
        }
        self.advance_width(text, cap_height, width_factor)
    }

    /// Sum of the glyph advances, blank text included (additive: the width of a concatenation is the sum of the widths).
    pub fn advance_width(&self, text: &str, cap_height: f64, width_factor: f64) -> f64 {
        let sum: f64 = text.chars().map(|c| self.glyph(c).advance).sum();
        sum * self.scaling(cap_height) * width_factor
    }

    pub fn space_width(&self, cap_height: f64, width_factor: f64) -> f64 {
        self.glyph(' ').advance * self.scaling(cap_height) * width_factor
    }

    /// Path elements the outlines of `text` have, counted up to `cap` (a longer text gives `cap + 1`; it is not counted to the
    /// end): what `glyph_paths` would make, found out without making it.
    pub fn glyph_elems_up_to(&self, text: &str, cap: usize) -> usize {
        let mut n = 0usize;
        for ch in text.chars() {
            n = n.saturating_add(self.glyph(ch).elems.len());
            if n > cap {
                return cap.saturating_add(1);
            }
        }
        n
    }

    /// One path per glyph that has an outline, scaled to `cap_height`, baseline at y = 0, text starts at x = 0.
    pub fn glyph_paths(&self, text: &str, cap_height: f64, width_factor: f64) -> Vec<Elems> {
        let s = self.scaling(cap_height);
        let (xf, yf) = (s * width_factor, s);
        let mut out = Vec::new();
        let mut x_off = 0.0;
        for ch in text.chars() {
            let g = self.glyph(ch);
            if !g.elems.is_empty() {
                let mut e = g.elems.clone();
                for p in &mut e.xy {
                    *p = [p[0] * xf + x_off, (p[1] - self.m.baseline) * yf];
                }
                out.push(e);
            }
            x_off += g.advance * xf;
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ lookup
fn font_dirs(opts: &FontOptions) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = opts.dirs.clone();
    if opts.system_fonts {
        if cfg!(windows) {
            if let Some(w) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
                dirs.push(PathBuf::from(w).join("Fonts"));
            }
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Microsoft").join("Windows").join("Fonts"));
            }
        } else if cfg!(target_os = "macos") {
            dirs.push("/Library/Fonts".into());
            dirs.push("/System/Library/Fonts".into());
        } else {
            for d in ["/usr/share/fonts", "/usr/local/share/fonts"] {
                dirs.push(d.into());
            }
            if let Some(h) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(&h).join(".fonts"));
                dirs.push(PathBuf::from(&h).join(".local/share/fonts"));
            }
        }
    }
    dirs
}

fn scan(dir: &Path, depth: u32, out: &mut HashMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                scan(&p, depth + 1, out);
            }
            continue;
        }
        let ext = p.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), "ttf" | "ttc" | "otf") {
            if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                out.entry(n.to_ascii_lowercase()).or_insert(p);
            }
        }
    }
}

struct Registry {
    opts_key: String,
    files: HashMap<String, PathBuf>,
}

static REGISTRY: OnceLock<Mutex<Vec<Registry>>> = OnceLock::new();
static FONTS: OnceLock<Mutex<HashMap<String, Option<Arc<Font>>>>> = OnceLock::new();

fn file_index(opts: &FontOptions) -> HashMap<String, PathBuf> {
    let key = format!("{}|{:?}", opts.system_fonts, opts.dirs);
    let reg = REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut g) = reg.lock() else { return HashMap::new() };
    if let Some(r) = g.iter().find(|r| r.opts_key == key) {
        return r.files.clone();
    }
    let mut files = HashMap::new();
    for d in font_dirs(opts) {
        scan(&d, 0, &mut files);
    }
    g.push(Registry { opts_key: key, files: files.clone() });
    files
}

fn load_file(path: &Path) -> Option<Arc<Font>> {
    let data = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    Font::from_data(&name, Arc::new(data), 0, false).map(Arc::new)
}

fn embedded_font() -> Arc<Font> {
    static EMB: OnceLock<Arc<Font>> = OnceLock::new();
    EMB.get_or_init(|| {
        Arc::new(Font::from_data("DejaVuSans.ttf", Arc::new(EMBEDDED.to_vec()), 0, true).expect("the embedded font is a valid TrueType file"))
    })
    .clone()
}

fn cached<F: FnOnce() -> Option<Arc<Font>>>(key: String, f: F) -> Option<Arc<Font>> {
    let map = FONTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = map.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return v;
    }
    let v = f();
    if let Ok(mut m) = map.lock() {
        m.insert(key, v.clone());
    }
    v
}

/// Default font: first existing face of ezdxf's default list, else the embedded DejaVu Sans.
pub fn default_font(opts: &FontOptions) -> Arc<Font> {
    let key = format!("default|{}|{:?}", opts.system_fonts, opts.dirs);
    cached(key, || {
        let idx = file_index(opts);
        for n in DEFAULT_FONTS {
            if let Some(p) = idx.get(n) {
                if let Some(f) = load_file(p) {
                    return Some(f);
                }
            }
        }
        Some(embedded_font())
    })
    .unwrap_or_else(embedded_font)
}

fn family_candidates(family: &str, bold: bool, italic: bool) -> Vec<String> {
    let f = family.trim().to_ascii_lowercase();
    let compact: String = f.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = Vec::new();
    let suffixes: &[&str] = match (bold, italic) {
        (false, false) => &["", "-regular", "regular"],
        (true, false) => &["bd", "b", "-bold", "bold"],
        (false, true) => &["i", "-italic", "italic", "-oblique"],
        (true, true) => &["bi", "z", "-bolditalic", "bolditalic"],
    };
    let bases: Vec<String> = match compact.as_str() {
        "timesnewroman" => vec!["times".into(), compact.clone()],
        "couriernew" => vec!["cour".into(), compact.clone()],
        "arial" => vec!["arial".into()],
        _ => vec![compact.clone(), f.clone()],
    };
    for b in bases {
        for s in suffixes {
            out.push(format!("{}{}.ttf", b, s));
        }
    }
    out
}

/// Font for a text style: the style's TrueType file name, else the XDATA family, else the default font.
pub fn font_for_style(style: Option<&crate::dxf::StyleDef>, opts: &FontOptions) -> Arc<Font> {
    let Some(st) = style else { return default_font(opts) };
    let file = st.font.trim().to_string();
    if file.is_empty() {
        if st.family.trim().is_empty() {
            return default_font(opts);
        }
        let key = format!("family|{}|{}|{}|{}|{:?}", st.family.to_ascii_lowercase(), st.bold, st.italic, opts.system_fonts, opts.dirs);
        return cached(key, || {
            let idx = file_index(opts);
            for n in family_candidates(&st.family, st.bold, st.italic) {
                if let Some(p) = idx.get(&n) {
                    if let Some(f) = load_file(p) {
                        return Some(f);
                    }
                }
            }
            None
        })
        .unwrap_or_else(|| default_font(opts));
    }
    let lower = file.to_ascii_lowercase();
    let is_ttf = lower.ends_with(".ttf") || lower.ends_with(".ttc") || lower.ends_with(".otf");
    if !is_ttf {
        // SHX / LFF fonts: the shape files are not available, the default font is used
        return default_font(opts);
    }
    let base = Path::new(&file.replace('\\', "/")).file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or(lower);
    let key = format!("file|{}|{}|{:?}", base, opts.system_fonts, opts.dirs);
    cached(key, || {
        let idx = file_index(opts);
        idx.get(&base).and_then(|p| load_file(p))
    })
    .unwrap_or_else(|| default_font(opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emb() -> Arc<Font> {
        embedded_font()
    }

    #[test]
    fn embedded_font_measurements() {
        let f = emb();
        assert!(f.m.cap_height > 0.0 && f.m.x_height > 0.0 && f.m.x_height < f.m.cap_height);
        assert!(f.m.descender > 0.0);
        let m = f.measurements(10.0);
        assert!((m.cap_height - 10.0).abs() < 1e-9);
    }

    #[test]
    fn text_width_scales_with_height_and_factor() {
        let f = emb();
        let w1 = f.text_width("Hello", 10.0, 1.0);
        let w2 = f.text_width("Hello", 20.0, 1.0);
        let w3 = f.text_width("Hello", 10.0, 0.5);
        assert!(w1 > 20.0);
        assert!((w2 / w1 - 2.0).abs() < 1e-9);
        assert!((w3 / w1 - 0.5).abs() < 1e-9);
        assert_eq!(f.text_width("   ", 10.0, 1.0), 0.0);
    }

    #[test]
    fn glyph_paths_sit_on_the_baseline() {
        let f = emb();
        let paths = f.glyph_paths("Hx", 10.0, 1.0);
        assert_eq!(paths.len(), 2);
        let b = paths[0].bounds().unwrap();
        assert!(b[1].abs() < 1e-6, "H bottom at the baseline, got {}", b[1]);
        assert!((b[3] - 10.0).abs() < 1e-6, "cap height, got {}", b[3]);
        // the second glyph starts to the right of the first
        assert!(paths[1].bounds().unwrap()[0] > b[0]);
    }

    #[test]
    fn czech_letters_have_outlines() {
        let f = emb();
        for c in "ěščřžýáíéúůďťňĚŠČŘŽ".chars() {
            assert!(!f.glyph(c).elems.is_empty(), "missing {c}");
        }
    }

    #[test]
    fn shx_fonts_fall_back_to_default() {
        let opts = FontOptions { system_fonts: false, dirs: vec![] };
        let st = crate::dxf::StyleDef { name: "S".into(), font: "txt.shx".into(), ..Default::default() };
        let f = font_for_style(Some(&st), &opts);
        assert!(f.embedded);
    }
}
