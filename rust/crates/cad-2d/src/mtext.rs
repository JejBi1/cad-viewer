//! MTEXT: inline code parser and layout (ezdxf `MTextParser`, `text_layout` and the complex MTEXT renderer).
use crate::font::Font;
use crate::geom::V2;

pub const LEADING: f64 = 1.667; // baseline to baseline = cap height * 1.667 * line spacing factor
pub const FRACTION_HEIGHT_SCALE: f64 = 1.2;
/// Most columns of one MTEXT that are laid out. A column count of 2 000 000 000 in a 130 byte file used to allocate
/// 30 GB; no real text has more columns than a page has lines.
pub const MAX_COLUMNS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParaAlign {
    Default,
    Left,
    Right,
    Center,
    Justified,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParaProps {
    pub indent: f64,
    pub left: f64,
    pub right: f64,
    pub align: ParaAlign,
    /// Shared between all tokens of a paragraph: a context is cloned for every token.
    pub tab_stops: std::sync::Arc<Vec<f64>>,
}

impl Default for ParaProps {
    fn default() -> Self {
        ParaProps { indent: 0.0, left: 0.0, right: 0.0, align: ParaAlign::Default, tab_stops: std::sync::Arc::new(Vec::new()) }
    }
}

/// Text properties that inline codes change.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub underline: bool,
    pub overline: bool,
    pub strike: bool,
    pub continue_stroke: bool,
    pub aci: i32,
    pub rgb: Option<[u8; 3]>,
    /// 0 bottom, 1 centre, 2 top
    pub align: u8,
    pub font_family: Option<(String, bool, bool)>,
    pub cap_height: f64,
    pub width_factor: f64,
    pub oblique: f64,
    pub para: ParaProps,
}

impl Ctx {
    pub fn new(cap_height: f64, width_factor: f64, oblique: f64, aci: i32) -> Ctx {
        Ctx {
            underline: false,
            overline: false,
            strike: false,
            continue_stroke: false,
            aci,
            rgb: None,
            align: 0,
            font_family: None,
            cap_height,
            width_factor,
            oblique,
            para: ParaProps::default(),
        }
    }

    fn has_stroke(&self) -> bool {
        self.underline || self.overline || self.strike
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Word(String),
    Space,
    Nbsp,
    Tab,
    NewParagraph,
    NewColumn,
    /// numerator, denominator, type char ('^', '/', '#' or empty)
    Stack(String, String, String),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub ctx: Ctx,
}

/// `^I` style caret notation: `^` followed by a char becomes (c - 64) % 126.
pub fn caret_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '^' {
            if let Some(&n) = it.peek() {
                it.next();
                let v = ((n as i64 - 64).rem_euclid(126)) as u32;
                out.push(char::from_u32(v).unwrap_or('?'));
                continue;
            }
        }
        out.push(c);
    }
    out
}

struct Scan {
    c: Vec<char>,
    i: usize,
}

impl Scan {
    fn peek(&self, n: usize) -> char {
        self.c.get(self.i + n).copied().unwrap_or('\0')
    }
    fn has(&self) -> bool {
        self.i < self.c.len()
    }
    fn get(&mut self) -> char {
        let c = self.peek(0);
        self.i += 1;
        c
    }
    fn tail(&self) -> String {
        self.c[self.i.min(self.c.len())..].iter().collect()
    }
    /// Expression up to the next `;` (excluded), consumes the `;`.
    fn expression(&mut self, escape: bool) -> String {
        let mut j = self.i;
        let mut stop = None;
        while j < self.c.len() {
            if escape && self.c[j] == '\\' {
                j += 2;
                continue;
            }
            if self.c[j] == ';' {
                stop = Some(j);
                break;
            }
            j += 1;
        }
        let end = stop.unwrap_or(self.c.len());
        let s: String = self.c[self.i..end.min(self.c.len())].iter().collect();
        self.i = (end + 1).min(self.c.len() + 1);
        s
    }
    fn float_expr(&mut self, relative: bool) -> String {
        // [+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?x?
        let t: &[char] = &self.c[self.i.min(self.c.len())..];
        let mut k = 0;
        if k < t.len() && (t[k] == '+' || t[k] == '-') {
            k += 1;
        }
        let ds = k;
        while k < t.len() && t[k].is_ascii_digit() {
            k += 1;
        }
        if k < t.len() && t[k] == '.' {
            k += 1;
            while k < t.len() && t[k].is_ascii_digit() {
                k += 1;
            }
        }
        if k == ds || (k == ds + 1 && t[ds] == '.') {
            return String::new();
        }
        if k < t.len() && (t[k] == 'e' || t[k] == 'E') {
            let mut m = k + 1;
            if m < t.len() && (t[m] == '+' || t[m] == '-') {
                m += 1;
            }
            let ms = m;
            while m < t.len() && t[m].is_ascii_digit() {
                m += 1;
            }
            if m > ms {
                k = m;
            }
        }
        if relative && k < t.len() && t[k] == 'x' {
            k += 1;
        }
        let s: String = t[..k].iter().collect();
        self.i += k;
        if s.starts_with('.') {
            format!("0{}", s)
        } else {
            s
        }
    }
    fn int_expr(&mut self) -> String {
        let mut s = String::new();
        while self.peek(0).is_ascii_digit() {
            s.push(self.get());
        }
        s
    }
    fn optional_terminator(&mut self) {
        if self.peek(0) == ';' {
            self.i += 1;
        }
    }
}

fn special_char(code: char) -> Option<char> {
    match code.to_ascii_lowercase() {
        'c' => Some('Ø'),
        'd' => Some('°'),
        'p' => Some('±'),
        _ => None,
    }
}

/// Port of `MTextParser`.
pub fn parse_mtext(content: &str, base: Ctx) -> Vec<Token> {
    let mut sc = Scan { c: caret_decode(content).chars().collect(), i: 0 };
    let mut ctx = base;
    let mut stack: Vec<Ctx> = Vec::new();
    let mut continue_stroke = false;
    let mut out: Vec<Token> = Vec::new();
    let mut word = String::new();
    macro_rules! flush {
        () => {
            if !word.is_empty() {
                out.push(Token { tok: Tok::Word(std::mem::take(&mut word)), ctx: ctx.clone() });
            }
        };
    }
    while sc.has() {
        let mut letter = sc.peek(0);
        let mut escape = false;
        if letter == '\\' {
            if matches!(sc.peek(1), '\\' | '{' | '}') {
                escape = true;
                sc.i += 1;
                letter = sc.peek(0);
            } else {
                flush!();
                sc.i += 1;
                let cmd = sc.get();
                match cmd {
                    '~' => out.push(Token { tok: Tok::Nbsp, ctx: ctx.clone() }),
                    'P' => out.push(Token { tok: Tok::NewParagraph, ctx: ctx.clone() }),
                    'N' => out.push(Token { tok: Tok::NewColumn, ctx: ctx.clone() }),
                    'X' => {}
                    'S' => {
                        let expr = sc.expression(true);
                        let mut e = Scan { c: expr.chars().collect(), i: 0 };
                        // numerator until an unescaped ^ / #
                        let mut num = String::new();
                        let mut ty = String::new();
                        while e.has() {
                            let mut c = e.get();
                            let mut esc = false;
                            if c == '\\' {
                                esc = true;
                                c = e.get();
                            }
                            if !esc && matches!(c, '^' | '/' | '#') {
                                ty = c.to_string();
                                break;
                            }
                            num.push(if (c as u32) < 32 { ' ' } else { c });
                        }
                        let mut den = String::new();
                        if !ty.is_empty() {
                            while e.has() {
                                let mut c = e.get();
                                if c == '\\' {
                                    c = e.get();
                                }
                                den.push(if (c as u32) < 32 { ' ' } else { c });
                            }
                        }
                        out.push(Token { tok: Tok::Stack(num, den, ty), ctx: ctx.clone() });
                    }
                    '\0' => {}
                    other => {
                        let mut new = ctx.clone();
                        match other {
                            'L' => {
                                new.underline = true;
                                continue_stroke = true;
                            }
                            'l' => {
                                new.underline = false;
                                if !new.has_stroke() {
                                    continue_stroke = false;
                                }
                            }
                            'O' => {
                                new.overline = true;
                                continue_stroke = true;
                            }
                            'o' => {
                                new.overline = false;
                                if !new.has_stroke() {
                                    continue_stroke = false;
                                }
                            }
                            'K' => {
                                new.strike = true;
                                continue_stroke = true;
                            }
                            'k' => {
                                new.strike = false;
                                if !new.has_stroke() {
                                    continue_stroke = false;
                                }
                            }
                            'A' => {
                                let c = sc.get();
                                new.align = match c {
                                    '0' => 0,
                                    '1' => 1,
                                    '2' => 2,
                                    _ => 0,
                                };
                                sc.optional_terminator();
                            }
                            'C' => {
                                let e = sc.int_expr();
                                if let Ok(v) = e.parse::<i32>() {
                                    if v < 257 {
                                        new.aci = v;
                                        new.rgb = None;
                                    }
                                }
                                sc.optional_terminator();
                            }
                            'c' => {
                                let e = sc.int_expr();
                                if let Ok(v) = e.parse::<u32>() {
                                    // reversed order: the integer is BGR
                                    let v = v & 0xFF_FFFF;
                                    new.rgb = Some([(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]);
                                }
                                sc.optional_terminator();
                            }
                            'H' | 'W' | 'T' => {
                                let e = sc.float_expr(true);
                                if !e.is_empty() {
                                    let (cur, relative) = match other {
                                        'H' => (new.cap_height, e.ends_with('x')),
                                        'W' => (new.width_factor, e.ends_with('x')),
                                        _ => (1.0, e.ends_with('x')),
                                    };
                                    let v = if relative {
                                        cur * e.trim_end_matches('x').parse::<f64>().unwrap_or(1.0).abs()
                                    } else {
                                        e.parse::<f64>().unwrap_or(cur).abs()
                                    };
                                    match other {
                                        'H' => new.cap_height = v,
                                        'W' => new.width_factor = v,
                                        _ => {}
                                    }
                                }
                                sc.optional_terminator();
                            }
                            'Q' => {
                                let e = sc.float_expr(false);
                                if !e.is_empty() {
                                    new.oblique = e.parse::<f64>().unwrap_or(0.0);
                                }
                                sc.optional_terminator();
                            }
                            'p' => {
                                let e = sc.expression(false);
                                let mut p = new.para.clone();
                                parse_paragraph(&e, &mut p);
                                new.para = p;
                            }
                            'f' | 'F' => {
                                let e = sc.expression(false);
                                let parts: Vec<&str> = e.split('|').collect();
                                if !parts.is_empty() && !parts[0].is_empty() {
                                    let mut bold = false;
                                    let mut italic = false;
                                    for p in &parts[1..] {
                                        if p.starts_with("b1") {
                                            bold = true;
                                        } else if p.starts_with("i1") {
                                            italic = true;
                                        }
                                    }
                                    new.font_family = Some((parts[0].to_string(), bold, italic));
                                }
                            }
                            _ => {
                                // unknown command is printed verbatim
                                word.push('\\');
                                word.push(other);
                                continue;
                            }
                        }
                        new.continue_stroke = continue_stroke;
                        ctx = new;
                    }
                }
                continue;
            }
        }
        if (letter as u32) < 32 {
            match letter {
                '\t' => {
                    flush!();
                    sc.i += 1;
                    out.push(Token { tok: Tok::Tab, ctx: ctx.clone() });
                    continue;
                }
                '\n' => {
                    flush!();
                    sc.i += 1;
                    out.push(Token { tok: Tok::NewParagraph, ctx: ctx.clone() });
                    continue;
                }
                _ => letter = ' ',
            }
        } else if letter == '%' && sc.peek(1) == '%' {
            if let Some(sp) = special_char(sc.peek(2)) {
                sc.i += 2;
                letter = sp;
            }
        }
        if letter == ' ' {
            flush!();
            sc.i += 1;
            out.push(Token { tok: Tok::Space, ctx: ctx.clone() });
            continue;
        }
        if !escape {
            if letter == '{' {
                flush!();
                sc.i += 1;
                stack.push(ctx.clone());
                continue;
            } else if letter == '}' {
                flush!();
                sc.i += 1;
                if let Some(c) = stack.pop() {
                    ctx = c;
                }
                continue;
            }
        }
        sc.i += 1;
        word.push(letter);
    }
    flush!();
    // \U+XXXX escapes were handled by the DXF reader; remaining ones in the text are decoded here
    out
}

fn parse_paragraph(expr: &str, p: &mut ParaProps) {
    let mut sc = Scan { c: expr.chars().collect(), i: 0 };
    let parse_float = |sc: &mut Scan| -> f64 {
        let e = sc.float_expr(false);
        while sc.peek(0) == ',' {
            sc.i += 1;
        }
        e.parse::<f64>().unwrap_or(0.0)
    };
    while sc.has() {
        match sc.get() {
            'i' => p.indent = parse_float(&mut sc),
            'l' => p.left = parse_float(&mut sc),
            'r' => p.right = parse_float(&mut sc),
            'x' => {}
            'q' => {
                p.align = match sc.get() {
                    'l' => ParaAlign::Left,
                    'r' => ParaAlign::Right,
                    'c' => ParaAlign::Center,
                    'j' | 'd' => ParaAlign::Justified,
                    _ => ParaAlign::Default,
                };
                while sc.peek(0) == ',' {
                    sc.i += 1;
                }
            }
            't' => {
                let mut stops: Vec<f64> = Vec::new();
                while sc.has() {
                    let c = sc.peek(0);
                    if c == 'r' || c == 'c' {
                        sc.i += 1;
                        let v = parse_float(&mut sc);
                        stops.push(v);
                    } else {
                        let e = sc.float_expr(false);
                        if e.is_empty() {
                            sc.i += 1;
                        } else {
                            stops.push(e.parse::<f64>().unwrap_or(0.0));
                            while sc.peek(0) == ',' {
                                sc.i += 1;
                            }
                        }
                    }
                }
                p.tab_stops = std::sync::Arc::new(stops);
            }
            _ => {}
        }
    }
    let _ = sc.tail();
}

pub fn has_inline_codes(text: &str) -> bool {
    text.replace("\\P", "").replace("\\~", "").contains('\\')
}

/// Plain text of an MTEXT: `\P` as line break, inline codes removed.
pub fn plain_mtext(text: &str) -> String {
    let base = Ctx::new(1.0, 1.0, 0.0, 256);
    let mut out = String::new();
    for t in parse_mtext(text, base) {
        match t.tok {
            Tok::Word(w) => out.push_str(&w),
            Tok::Space | Tok::Nbsp => out.push(' '),
            Tok::Tab => out.push_str("    "),
            Tok::NewParagraph | Tok::NewColumn => out.push('\n'),
            Tok::Stack(a, b, ty) => {
                out.push_str(&a);
                if !ty.is_empty() {
                    out.push_str(&ty);
                    out.push_str(&b);
                }
            }
        }
    }
    out
}

/// Port of `ezdxf.tools.text.text_wrap`: wrap at newlines and at `box_width`. `width` has to be additive (the width of a
/// text is the sum of the widths of its parts, blanks included, e.g. `Font::advance_width`): the width of the line under
/// construction is kept, so every word is measured once.
pub fn text_wrap(text: &str, box_width: Option<f64>, width: &dyn Fn(&str) -> f64) -> Vec<String> {
    if text.is_empty() || text.chars().all(|c| c.is_whitespace()) {
        return Vec::new();
    }
    // tokens: "\n" as its own token, runs of whitespace and runs of other characters
    let mut tokens: Vec<String> = Vec::new();
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            tokens.push("\n".into());
        }
        let mut cur = String::new();
        let mut cur_space: Option<bool> = None;
        for c in line.chars() {
            let sp = c.is_whitespace();
            match cur_space {
                Some(s) if s == sp => cur.push(c),
                _ => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                    cur.push(c);
                    cur_space = Some(sp);
                }
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0.0;
    let mut just_wrapped = false;
    for t in tokens {
        let on_first = lines.is_empty();
        if t == "\n" && just_wrapped {
            continue;
        }
        just_wrapped = false;
        if t == "\n" {
            lines.push(current.trim_end().to_string());
            current.clear();
            current_w = 0.0;
        } else if t.chars().all(|c| c.is_whitespace()) {
            if !current.is_empty() || on_first {
                current_w += width(&t);
                current.push_str(&t);
            }
        } else {
            let tw = width(&t);
            if box_width.map(|w| current_w + tw > w).unwrap_or(false) {
                if current.is_empty() {
                    current.push_str(&t);
                    current_w = tw;
                } else {
                    lines.push(current.trim_end().to_string());
                    current = t;
                    current_w = tw;
                    just_wrapped = true;
                }
            } else {
                current.push_str(&t);
                current_w += tw;
            }
        }
    }
    if !current.is_empty() && !current.chars().all(|c| c.is_whitespace()) {
        lines.push(current.trim_end().to_string());
    }
    lines
}

// ------------------------------------------------------------------------------------------------ layout
/// What the layout hands to the renderer, in layout coordinates (x right, y up, origin at the insertion point).
#[derive(Clone, Debug)]
pub enum Cmd {
    /// A word: baseline origin (`left`, `bottom`), width factor `sx` and oblique shear `tx` (tan of the angle).
    Word { text: String, left: f64, bottom: f64, ctx: Ctx },
    Line { a: V2, b: V2 },
}

#[derive(Clone)]
struct Cell {
    kind: u8, // 0 word, 1 space, 2 nbsp, 3 tab, 4 fraction
    width: f64,
    height: f64,
    text: String,
    frac: Option<(String, String, String)>,
    ctx: Ctx,
}

struct Line {
    cells: Vec<(Cell, f64)>, // cell, x offset
    width: f64,
    height: f64,
}

struct Para {
    lines: Vec<Line>,
    props: ParaProps,
    cap_height: f64,
    empty: bool,
}

pub struct LayoutOptions {
    pub width: f64,
    pub attachment: i32,
    pub line_spacing: f64,
    pub base_ctx: Ctx,
    pub columns: Option<(usize, f64, f64, Vec<Option<f64>>)>, // count, width, gutter, heights
}

fn measure(font_of: &dyn Fn(&Ctx) -> std::sync::Arc<Font>, text: &str, ctx: &Ctx) -> f64 {
    font_of(ctx).text_width(text, ctx.cap_height, ctx.width_factor)
}

fn cell_from(tok: &Token, font_of: &dyn Fn(&Ctx) -> std::sync::Arc<Font>) -> Cell {
    let ctx = tok.ctx.clone();
    match &tok.tok {
        Tok::Word(w) => Cell { kind: 0, width: measure(font_of, w, &ctx), height: ctx.cap_height, text: w.clone(), frac: None, ctx },
        Tok::Space | Tok::Tab | Tok::Nbsp => {
            let sw = font_of(&ctx).space_width(ctx.cap_height, ctx.width_factor);
            let kind = match tok.tok {
                Tok::Space => 1,
                Tok::Nbsp => 2,
                _ => 3,
            };
            Cell { kind, width: sw, height: 0.0, text: String::new(), frac: None, ctx }
        }
        Tok::Stack(a, b, ty) => {
            if ty.is_empty() {
                return Cell { kind: 0, width: measure(font_of, a, &ctx), height: ctx.cap_height, text: a.clone(), frac: None, ctx };
            }
            let wa = measure(font_of, a, &ctx);
            let wb = measure(font_of, b, &ctx);
            let (w, h) = if ty == "#" {
                (wa + wb, 2.0 * ctx.cap_height)
            } else {
                (wa.max(wb), FRACTION_HEIGHT_SCALE * 2.0 * ctx.cap_height)
            };
            Cell { kind: 4, width: w, height: h, text: String::new(), frac: Some((a.clone(), b.clone(), ty.clone())), ctx }
        }
        _ => Cell { kind: 1, width: 0.0, height: 0.0, text: String::new(), frac: None, ctx },
    }
}

fn breakable(c: &Cell) -> bool {
    c.kind == 1 || c.kind == 3
}

fn make_lines(cells: &[Cell], first_w: f64, other_w: f64, tab_pos: &dyn Fn(f64) -> f64) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Vec<(Cell, f64)> = Vec::new();
    let mut x = 0.0;
    let limit = |lines: &Vec<Line>| if lines.is_empty() { first_w } else { other_w };
    let flush = |cur: &mut Vec<(Cell, f64)>, lines: &mut Vec<Line>| {
        // remove the trailing space that caused the break
        while matches!(cur.last(), Some((c, _)) if c.kind == 1) {
            cur.pop();
        }
        let width = cur.last().map(|(c, o)| o + c.width).unwrap_or(0.0);
        let height = cur.iter().map(|(c, _)| c.height).fold(0.0, f64::max);
        lines.push(Line { cells: std::mem::take(cur), width, height });
    };
    // group cells into non-breakable runs (words glued to words / stacks)
    let mut i = 0;
    while i < cells.len() {
        let c = &cells[i];
        if breakable(c) {
            if c.kind == 3 {
                x = tab_pos(x).max(x);
                cur.push((Cell { width: 0.0, ..c.clone() }, x));
            } else if !cur.is_empty() {
                cur.push((c.clone(), x));
                x += c.width;
            }
            i += 1;
            continue;
        }
        // a run of non-breaking cells
        let mut j = i;
        let mut run_w = 0.0;
        while j < cells.len() && !breakable(&cells[j]) {
            run_w += cells[j].width;
            j += 1;
        }
        if x + run_w > limit(&lines) + 1e-9 && !cur.is_empty() && cur.iter().any(|(c, _)| c.kind == 0 || c.kind == 4) {
            flush(&mut cur, &mut lines);
            x = 0.0;
        }
        for c in &cells[i..j] {
            cur.push((c.clone(), x));
            x += c.width;
        }
        i = j;
    }
    if !cur.is_empty() {
        flush(&mut cur, &mut lines);
    }
    lines
}

/// Result of a layout: the drawing commands and the width of the widest line.
pub struct Laid {
    pub cmds: Vec<Cmd>,
    pub max_line: f64,
}

pub fn layout(tokens: &[Token], opt: &LayoutOptions, font_of: &dyn Fn(&Ctx) -> std::sync::Arc<Font>) -> Laid {
    let initial_cap = opt.base_ctx.cap_height;
    let width = opt.width;
    // paragraphs
    let mut paras: Vec<(Vec<Cell>, ParaProps, f64, bool)> = Vec::new(); // cells, props, cap height, new column before
    let mut cells: Vec<Cell> = Vec::new();
    let mut ctx = opt.base_ctx.clone();
    let mut column_break = false;
    let push_para = |cells: &mut Vec<Cell>, ctx: &Ctx, paras: &mut Vec<(Vec<Cell>, ParaProps, f64, bool)>, cb: &mut bool| {
        paras.push((std::mem::take(cells), ctx.para.clone(), ctx.cap_height, *cb));
        *cb = false;
    };
    for t in tokens {
        ctx = t.ctx.clone();
        match t.tok {
            Tok::NewParagraph => push_para(&mut cells, &ctx, &mut paras, &mut column_break),
            Tok::NewColumn => {
                push_para(&mut cells, &ctx, &mut paras, &mut column_break);
                column_break = true;
            }
            _ => cells.push(cell_from(t, font_of)),
        }
    }
    if !cells.is_empty() {
        push_para(&mut cells, &ctx, &mut paras, &mut column_break);
    }
    // columns
    let (ncols, col_w, gutter, heights) = match &opt.columns {
        Some((n, w, g, h)) if *n > 1 => ((*n).min(MAX_COLUMNS), if *w > 0.0 { *w } else { width }, *g, h.iter().take(MAX_COLUMNS).copied().collect::<Vec<_>>()),
        _ => (1, width, 0.0, vec![None]),
    };
    let lead = |h: f64| h * LEADING * opt.line_spacing;
    // lay out the paragraphs into lines
    let step = 4.0 * initial_cap.max(1e-9);
    let tab_pos = |x: f64| ((x / step).floor() + 1.0) * step;
    let mut built: Vec<Para> = Vec::new();
    for (cells, props, cap, _) in &paras {
        if cells.is_empty() {
            built.push(Para { lines: Vec::new(), props: props.clone(), cap_height: *cap, empty: true });
            continue;
        }
        let left = props.left * initial_cap;
        let right = props.right * initial_cap;
        let first = left + props.indent * initial_cap;
        let first_w = (col_w - first - right).max(1e-6);
        let other_w = (col_w - left - right).max(1e-6);
        let lines = make_lines(cells, first_w, other_w, &tab_pos);
        built.push(Para { lines, props: props.clone(), cap_height: *cap, empty: false });
    }
    // paragraph metrics
    struct Pm {
        height: f64,
        gap: f64,
    }
    let metrics: Vec<Pm> = built
        .iter()
        .map(|p| {
            if p.empty {
                return Pm { height: p.cap_height, gap: lead(p.cap_height) - p.cap_height };
            }
            let n = p.lines.len();
            let mut h = 0.0;
            for (k, l) in p.lines.iter().enumerate() {
                if k + 1 < n {
                    h += lead(l.height);
                }
            }
            let last = p.lines.last().map(|l| l.height).unwrap_or(0.0);
            Pm { height: h + last, gap: lead(last) - last }
        })
        .collect();
    // flow paragraphs into columns
    let mut col_of: Vec<usize> = vec![0; built.len()];
    let mut col_y: Vec<f64> = vec![0.0; ncols.max(1)];
    let mut col = 0usize;
    for (i, (_, _, _, cb)) in paras.iter().enumerate() {
        if *cb && col + 1 < ncols {
            col += 1;
        }
        if let Some(Some(hmax)) = heights.get(col) {
            if col_y[col] > 0.0 && col_y[col] + metrics[i].height > *hmax + 1e-9 && col + 1 < ncols {
                col += 1;
            }
        }
        col_of[i] = col;
        col_y[col] += metrics[i].height + metrics[i].gap;
    }
    let mut used: Vec<f64> = vec![0.0; ncols.max(1)];
    let mut last_in_col: Vec<Option<usize>> = vec![None; ncols.max(1)];
    for i in 0..built.len() {
        used[col_of[i]] += metrics[i].height + metrics[i].gap;
        last_in_col[col_of[i]] = Some(i);
    }
    for (c, last) in last_in_col.iter().enumerate() {
        if let Some(l) = last {
            used[c] -= metrics[*l].gap;
        }
    }
    let total_h = used.iter().copied().fold(0.0, f64::max);
    let total_w = if ncols > 1 { col_w * ncols as f64 + gutter * (ncols as f64 - 1.0) } else { width };
    // alignment of the layout (attachment point)
    let (left0, top0) = match opt.attachment {
        2 => (-total_w / 2.0, 0.0),
        3 => (-total_w, 0.0),
        4 => (0.0, total_h / 2.0),
        5 => (-total_w / 2.0, total_h / 2.0),
        6 => (-total_w, total_h / 2.0),
        7 => (0.0, total_h),
        8 => (-total_w / 2.0, total_h),
        9 => (-total_w, total_h),
        _ => (0.0, 0.0),
    };
    let default_align = match opt.attachment {
        2 | 5 | 8 => ParaAlign::Center,
        3 | 6 | 9 => ParaAlign::Right,
        _ => ParaAlign::Left,
    };
    let mut cmds: Vec<Cmd> = Vec::new();
    let mut ycur: Vec<f64> = vec![0.0; ncols.max(1)];
    for (i, p) in built.iter().enumerate() {
        let c = col_of[i];
        let x0 = left0 + c as f64 * (col_w + gutter);
        let mut y = top0 - ycur[c];
        ycur[c] += metrics[i].height + metrics[i].gap;
        if p.empty {
            continue;
        }
        let left = p.props.left * initial_cap;
        let right = p.props.right * initial_cap;
        let first = left + p.props.indent * initial_cap;
        let align = match p.props.align {
            ParaAlign::Default => default_align,
            a => a,
        };
        for (li, line) in p.lines.iter().enumerate() {
            let indent = if li == 0 { first } else { left };
            let avail = col_w - indent - right;
            let shift = match align {
                ParaAlign::Center => ((avail - line.width) / 2.0).max(0.0),
                ParaAlign::Right => (avail - line.width).max(0.0),
                _ => 0.0,
            };
            let lx = x0 + indent + shift;
            let gh = line.height;
            for (k, (cell, off)) in line.cells.iter().enumerate() {
                let dy = if cell.ctx.align == 2 {
                    0.0
                } else {
                    let d = cell.height - gh;
                    if cell.ctx.align == 1 {
                        d / 2.0
                    } else {
                        d
                    }
                };
                let top = y + dy;
                let cx = lx + off;
                match cell.kind {
                    0 => {
                        let bottom = top - cell.height;
                        cmds.push(Cmd::Word { text: cell.text.clone(), left: cx, bottom, ctx: cell.ctx.clone() });
                        stroke(&mut cmds, cell, cx, top, bottom, &line.cells, k);
                    }
                    4 => {
                        let (a, b, ty) = cell.frac.clone().unwrap_or_default();
                        let wa = measure(font_of, &a, &cell.ctx);
                        let wb = measure(font_of, &b, &cell.ctx);
                        let ch = cell.ctx.cap_height;
                        let th = cell.height;
                        if ty == "#" {
                            cmds.push(Cmd::Word { text: a, left: cx, bottom: top - ch, ctx: cell.ctx.clone() });
                            cmds.push(Cmd::Word { text: b, left: cx + cell.width - wb, bottom: top - th, ctx: cell.ctx.clone() });
                            let delta = cell.width.min(th) / 2.0;
                            let (mx, my) = (cx + wa, top - ch);
                            cmds.push(Cmd::Line { a: [mx - delta, my - delta], b: [mx + delta, my + delta] });
                        } else {
                            let center = cx + cell.width / 2.0;
                            cmds.push(Cmd::Word { text: a, left: center - wa / 2.0, bottom: top - ch, ctx: cell.ctx.clone() });
                            cmds.push(Cmd::Word { text: b, left: center - wb / 2.0, bottom: top - th, ctx: cell.ctx.clone() });
                            if ty == "/" {
                                cmds.push(Cmd::Line { a: [cx, top - th / 2.0], b: [cx + cell.width, top - th / 2.0] });
                            }
                        }
                    }
                    _ => {}
                }
            }
            y -= lead(line.height);
        }
    }
    let max_line = built.iter().flat_map(|p| p.lines.iter().map(|l| l.width)).fold(0.0, f64::max);
    Laid { cmds, max_line }
}

fn stroke(cmds: &mut Vec<Cmd>, cell: &Cell, x: f64, top: f64, bottom: f64, line: &[(Cell, f64)], idx: usize) {
    let c = &cell.ctx;
    if !(c.underline || c.overline || c.strike) {
        return;
    }
    let mut right = x + cell.width;
    if c.continue_stroke {
        // extend across the adjacent glue cells
        for (g, _) in &line[idx + 1..] {
            if g.kind == 1 || g.kind == 2 || g.kind == 3 {
                right += g.width;
            } else {
                break;
            }
        }
    }
    let spacing = (top - bottom) / 5.0;
    if c.underline {
        cmds.push(Cmd::Line { a: [x, bottom - spacing], b: [right, bottom - spacing] });
    }
    if c.strike {
        let y = (top + bottom) / 2.0;
        cmds.push(Cmd::Line { a: [x, y], b: [right, y] });
    }
    if c.overline {
        cmds.push(Cmd::Line { a: [x, top + spacing], b: [right, top + spacing] });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<Token> {
        parse_mtext(s, Ctx::new(2.5, 1.0, 0.0, 256))
    }

    #[test]
    fn plain_text_and_paragraphs() {
        assert_eq!(plain_mtext("Hello\\Pworld"), "Hello\nworld");
        assert_eq!(plain_mtext("{\\C1;red} and more"), "red and more");
        assert_eq!(plain_mtext("%%c10 %%d"), "Ø10 °");
        assert_eq!(plain_mtext("a\\~b"), "a b");
    }

    #[test]
    fn inline_codes_change_context() {
        let t = tokens("A\\H2x;B{\\C3;C}D");
        let words: Vec<(&Tok, f64, i32)> = t.iter().map(|k| (&k.tok, k.ctx.cap_height, k.ctx.aci)).collect();
        assert_eq!(words[0].1, 2.5);
        assert_eq!(words[1].1, 5.0);
        assert_eq!(words[2].2, 3);
        assert_eq!(words[3].2, 256); // group closed
        assert_eq!(words[3].1, 5.0);
    }

    #[test]
    fn stacking_tokens() {
        let t = tokens("\\S1/2;");
        assert_eq!(t[0].tok, Tok::Stack("1".into(), "2".into(), "/".into()));
        // a caret followed by a space is the notation of a plain "^" (a caret and a letter is a control character)
        let t = tokens("\\SA^ B;");
        assert_eq!(t[0].tok, Tok::Stack("A".into(), "B".into(), "^".into()));
    }

    #[test]
    fn wrap_text() {
        let w = |s: &str| s.chars().count() as f64;
        assert_eq!(text_wrap("aaa bbb ccc", Some(7.0), &w), vec!["aaa bbb", "ccc"]);
        assert_eq!(text_wrap("a\nb", None, &w), vec!["a", "b"]);
        assert!(text_wrap("   ", None, &w).is_empty());
    }

    #[test]
    fn unknown_command_is_kept() {
        assert_eq!(plain_mtext("a\\zb"), "a\\zb");
    }
}
