//! Property resolution (colour, linetype, lineweight, visibility, layer) of ezdxf's drawing add-on `RenderContext`.
use crate::colors::*;
use crate::dxf::*;

pub const MODEL_SPACE_BG: [u8; 3] = [0x21, 0x28, 0x30];
pub const PAPER_SPACE_BG: [u8; 3] = [0xFF, 0xFF, 0xFF];

pub const TRANSPARENCY_BYBLOCK: i64 = 0x0100_0000;

/// Where the colour of a drawn thing comes from, when it comes from the background of the layout: the pens of the colour
/// 7 and of BYLAYER / BYBLOCK on a layer of that colour are the foreground colour (white on the dark model space, black on
/// white paper), and a WIPEOUT is filled with the background. Those colours change with the background; every other
/// colour (a true colour, an ACI colour that is not 7, even one that happens to be the same white) does not. A scene
/// records the role of each of its batches, so that the colours can be changed when the scene is drawn, without making it
/// again (`Drawing::batch_roles`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Role {
    #[default]
    None,
    /// The foreground colour of the layout.
    Fg,
    /// The background colour of the layout.
    Bg,
}

impl Role {
    /// The number the renderer takes: 0 any other colour, 1 foreground, 2 background (`Renderer2d::set_scene_owned`).
    pub fn code(self) -> u8 {
        match self {
            Role::None => 0,
            Role::Fg => 1,
            Role::Bg => 2,
        }
    }
}

/// Properties of a drawn thing after all BYLAYER / BYBLOCK rules.
#[derive(Clone, Debug)]
pub struct Properties {
    pub color: Rgba,
    /// `Fg` when `color` is the foreground colour of the layout (see `Role`), `Bg` for the background colour.
    pub role: Role,
    pub pen: i32,
    pub linetype: u32,
    /// Simplified dash pattern: empty = continuous.
    pub pattern: std::rc::Rc<Vec<f64>>,
    pub linetype_scale: f64,
    /// mm
    pub lineweight: f64,
    pub is_visible: bool,
    /// Resolved layer (layer 0 inside a block reference takes the layer of the INSERT).
    pub layer: u32,
}

#[derive(Clone, Debug)]
pub struct LayerProps {
    pub name_id: u32,
    pub color: Rgba,
    pub pen: i32,
    pub has_aci7: bool,
    pub linetype: u32,
    pub pattern: std::rc::Rc<Vec<f64>>,
    pub lineweight: f64,
    pub visible: bool,
}

/// Dash pattern of a linetype: ezdxf `compile_line_pattern` (dash-gap-dash ...; 0 = point). Empty for continuous.
pub fn compile_pattern(def: &LinetypeDef) -> Vec<f64> {
    if def.complex || def.elements.len() < 2 {
        return Vec::new();
    }
    // merge consecutive elements of the same sign
    let sign = |v: f64| if v < 0.0 { -1 } else if v > 0.0 { 1 } else { 0 };
    let mut merged: Vec<f64> = Vec::new();
    let mut buf = def.elements[0];
    let mut prev = sign(buf);
    for &e in &def.elements[1..] {
        if sign(e) == prev {
            buf += e;
        } else {
            merged.push(buf);
            buf = e;
            prev = sign(e);
        }
    }
    merged.push(buf);
    if merged.len() < 2 || def.pattern_length <= 0.0 {
        return Vec::new();
    }
    let sum: f64 = merged.iter().map(|e| e.abs()).sum();
    if def.pattern_length > sum {
        merged.push(sum - def.pattern_length);
    }
    if merged[0] < 0.0 {
        let e = merged.remove(0);
        let last = merged.len() - 1;
        if merged[last] < 0.0 {
            merged[last] += e;
        } else {
            merged.push(e);
        }
    }
    merged.into_iter().map(f64::abs).collect()
}

pub struct RenderContext<'d> {
    pub doc: &'d Document,
    /// Layout default (foreground) colour: white on the dark model space, black on paper / light background.
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub layers: Vec<Option<LayerProps>>,
    patterns: Vec<std::rc::Rc<Vec<f64>>>,
    /// Properties of the INSERT whose block content is being drawn.
    pub block_stack: Vec<Properties>,
    pub ltscale: f64,
    pub pdsize: f64,
    pub pdmode: i32,
    pub units: i32,
}

fn alpha_of(color: Rgba) -> u8 {
    color[3]
}

impl<'d> RenderContext<'d> {
    pub fn new(doc: &'d Document, model: bool, light_bg: bool) -> Self {
        let bg = if light_bg || !model { [0xFF, 0xFF, 0xFF] } else { MODEL_SPACE_BG };
        let fg = if luminance(bg) <= 0.2 { [0xFF, 0xFF, 0xFF] } else { [0, 0, 0] };
        let ltscale = doc.header.num("$LTSCALE").unwrap_or(1.0);
        let pdsize = doc.header.num("$PDSIZE").unwrap_or(1.0);
        let pdmode = doc.header.num("$PDMODE").unwrap_or(0.0) as i32;
        let insunits = doc.header.num("$INSUNITS").unwrap_or(0.0) as i32;
        let measurement = doc.header.num("$MEASUREMENT").unwrap_or(0.0) as i32;
        let units = if insunits == 0 {
            if measurement == 1 {
                6
            } else {
                1
            }
        } else {
            insunits
        };
        let mut ctx = RenderContext {
            doc,
            fg,
            bg,
            layers: Vec::new(),
            patterns: Vec::new(),
            block_stack: Vec::new(),
            ltscale,
            pdsize,
            pdmode,
            units,
        };
        ctx.setup_patterns();
        ctx.setup_layers();
        ctx
    }

    fn setup_patterns(&mut self) {
        let n = self.doc.linetype_names.len();
        self.patterns = vec![std::rc::Rc::new(Vec::new()); n];
        for def in &self.doc.linetypes {
            if let Some(id) = self.doc.linetype_names.find(&def.name) {
                self.patterns[id as usize] = std::rc::Rc::new(compile_pattern(def));
            }
        }
    }

    pub fn pattern_of(&self, linetype: u32) -> std::rc::Rc<Vec<f64>> {
        self.patterns.get(linetype as usize).cloned().unwrap_or_default()
    }

    fn setup_layers(&mut self) {
        let n = self.doc.layer_names.len();
        self.layers = vec![None; n];
        for l in &self.doc.layers {
            let lp = self.resolve_layer_props(l);
            if (l.id as usize) < self.layers.len() {
                self.layers[l.id as usize] = Some(lp);
            }
        }
        // ezdxf adds the layers "0" and "Defpoints" to every document that lacks them (`Drawing._create_required_layers`):
        // colour 7, continuous, default lineweight. Without them an entity on layer 0 of a file without LAYER records
        // would get the colour of an undefined layer (white), which cannot be seen on white paper.
        for name in ["0", "Defpoints"] {
            let Some(id) = self.doc.layer_names.find(name) else { continue };
            if self.layers.get(id as usize).is_some_and(|l| l.is_none()) {
                let def = LayerDef {
                    name: name.to_string(),
                    id,
                    handle: 0,
                    flags: 0,
                    color: 7,
                    true_color: None,
                    linetype: self.doc.continuous,
                    lineweight: -3,
                    transparency: 0.0,
                    plot: name == "0",
                };
                self.layers[id as usize] = Some(self.resolve_layer_props(&def));
            }
        }
    }

    fn resolve_layer_props(&self, l: &LayerDef) -> LayerProps {
        let rgb = if let Some(tc) = l.true_color {
            rgb_from_u32(tc)
        } else {
            let aci = l.color.abs();
            let aci = if !(1..=255).contains(&aci) { 7 } else { aci };
            self.aci_to_rgb(aci)
        };
        let alpha = ((1.0 - l.transparency.clamp(0.0, 1.0)) * 255.0).round() as u8;
        let default_lw = 0.25;
        let lw = if l.lineweight < 0 { default_lw } else { l.lineweight as f64 / 100.0 };
        LayerProps {
            name_id: l.id,
            color: rgba(rgb, alpha),
            pen: l.color,
            has_aci7: l.true_color.is_none() && l.color == 7,
            linetype: l.linetype,
            pattern: self.pattern_of(l.linetype),
            lineweight: lw,
            visible: l.is_on() && !l.is_frozen(),
        }
    }

    /// The role of the colour `resolve_aci_color` gives for `aci`: the foreground where it gives `fg`.
    pub fn aci_role(&self, aci: i32, layer: u32) -> Role {
        match aci {
            256 => match self.layer_props(layer) {
                Some(l) if l.has_aci7 => Role::Fg,
                Some(_) => Role::None,
                None => Role::None,
            },
            0 => match self.block_props() {
                None => Role::Fg,
                Some(b) => b.role,
            },
            7 => Role::Fg,
            1..=255 => Role::None,
            _ => Role::Fg,
        }
    }

    /// The role of the colour `resolve_color` gives for `c`.
    pub fn color_role(&self, c: &Common, layer: u32) -> Role {
        if c.true_color.is_some() {
            Role::None
        } else {
            self.aci_role(c.color, layer)
        }
    }

    pub fn aci_to_rgb(&self, aci: i32) -> [u8; 3] {
        if aci == 7 {
            self.fg
        } else {
            aci_rgb(aci)
        }
    }

    /// Layer visibility "as in the file" for the layer id.
    pub fn layer_visible_in_file(&self, id: u32) -> bool {
        self.layers.get(id as usize).and_then(|l| l.as_ref()).map(|l| l.visible).unwrap_or(true)
    }

    fn layer_props(&self, id: u32) -> Option<&LayerProps> {
        self.layers.get(id as usize).and_then(|l| l.as_ref())
    }

    pub fn inside_block(&self) -> bool {
        !self.block_stack.is_empty()
    }

    fn block_props(&self) -> Option<&Properties> {
        self.block_stack.last()
    }

    pub fn resolve_layer(&self, e: &Common) -> u32 {
        if e.layer == self.doc.layer0 {
            if let Some(b) = self.block_props() {
                return b.layer;
            }
        }
        e.layer
    }

    pub fn resolve_color(&self, c: &Common, layer: u32) -> Rgba {
        let lp = self.layer_props(layer);
        let rgb: [u8; 3];
        if let Some(tc) = c.true_color {
            rgb = rgb_from_u32(tc);
        } else {
            let aci = c.color;
            rgb = match aci {
                256 => match lp {
                    Some(l) if l.has_aci7 => self.fg,
                    Some(l) => [l.color[0], l.color[1], l.color[2]],
                    None => [255, 255, 255],
                },
                0 => match self.block_props() {
                    None => self.fg,
                    Some(b) => [b.color[0], b.color[1], b.color[2]],
                },
                1..=255 => self.aci_to_rgb(aci),
                _ => self.fg,
            };
        }
        // alpha
        let layer_alpha = lp.map(|l| alpha_of(l.color)).unwrap_or(255);
        let alpha = match c.transparency {
            Some(raw) if raw == TRANSPARENCY_BYBLOCK => match self.block_props() {
                Some(b) => b.color[3],
                None => 255,
            },
            None => layer_alpha,
            Some(raw) => (raw & 0xFF) as u8,
        };
        rgba(rgb, alpha)
    }

    /// `RenderContext.resolve_aci_color`: an ACI value (256 BYLAYER, 0 BYBLOCK) as RGB.
    pub fn resolve_aci_color(&self, aci: i32, layer: u32) -> [u8; 3] {
        match aci {
            256 => match self.layer_props(layer) {
                Some(l) if l.has_aci7 => self.fg,
                Some(l) => [l.color[0], l.color[1], l.color[2]],
                None => [255, 255, 255],
            },
            0 => match self.block_props() {
                None => self.fg,
                Some(b) => [b.color[0], b.color[1], b.color[2]],
            },
            1..=255 => self.aci_to_rgb(aci),
            _ => self.fg,
        }
    }

    pub fn resolve_pen(&self, c: &Common, layer: u32) -> i32 {
        match c.color {
            256 => self.layer_props(layer).map(|l| l.pen).unwrap_or(7),
            0 => self.block_props().map(|b| b.pen).unwrap_or(7),
            257 => 7,
            v => v,
        }
    }

    pub fn resolve_linetype(&self, c: &Common, layer: u32) -> (u32, std::rc::Rc<Vec<f64>>) {
        let doc = self.doc;
        if c.linetype == doc.bylayer || !c.has_linetype {
            return match self.layer_props(layer) {
                Some(l) => (l.linetype, l.pattern.clone()),
                None => (doc.continuous, Default::default()),
            };
        }
        if c.linetype == doc.byblock {
            return match self.block_props() {
                Some(b) => (b.linetype, b.pattern.clone()),
                None => (doc.continuous, Default::default()),
            };
        }
        (c.linetype, self.pattern_of(c.linetype))
    }

    pub fn resolve_lineweight(&self, c: &Common, layer: u32) -> f64 {
        let default = 0.25;
        let v = match c.lineweight {
            -1 => self.layer_props(layer).map(|l| l.lineweight).unwrap_or(default),
            -2 => self.block_props().map(|b| b.lineweight).unwrap_or(default),
            -3 => default,
            w => w as f64 / 100.0,
        };
        v.max(0.01)
    }

    /// `resolve_all` of ezdxf: every property of the entity in the current block context.
    pub fn resolve(&self, e: &Entity) -> Properties {
        let c = &e.c;
        let layer = self.resolve_layer(c);
        let (linetype, pattern) = self.resolve_linetype(c, layer);
        Properties {
            color: self.resolve_color(c, layer),
            role: self.color_role(c, layer),
            pen: self.resolve_pen(c, layer),
            linetype,
            pattern,
            linetype_scale: c.ltscale * self.ltscale,
            lineweight: self.resolve_lineweight(c, layer),
            is_visible: self.resolve_visible(e, layer),
            layer,
        }
    }

    /// Properties of an entity-less thing that takes everything from a layer (used for generated geometry).
    pub fn resolve_visible(&self, e: &Entity, layer: u32) -> bool {
        match &e.d {
            Data::Insert(i) if !i.table => !e.c.invisible,
            Data::Face3d { edge_flags, .. } => edge_flags & 15 != 15,
            Data::Viewport(v) => v.status > 0,
            _ => {
                // every layer is drawn (the scene tracks layers instead, see `Frontend`): off / frozen layers are only
                // flagged in the scene, so that they can be switched on without drawing again
                let _ = layer;
                match &e.d {
                    Data::Text(t) if t.kind == TextKind::Attrib => !e.c.invisible && t.attr_flags & 1 == 0,
                    _ => !e.c.invisible,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_pattern_simplification() {
        let def = LinetypeDef { name: "DASHED".into(), pattern_length: 0.75, elements: vec![0.5, -0.25], ..Default::default() };
        assert_eq!(compile_pattern(&def), vec![0.5, 0.25]);
        // dash, gap, dot, gap
        let def = LinetypeDef { name: "X".into(), pattern_length: 1.4, elements: vec![1.0, -0.2, 0.0, -0.2], ..Default::default() };
        assert_eq!(compile_pattern(&def), vec![1.0, 0.2, 0.0, 0.2]);
        // pattern starting with a gap moves it to the end
        let def = LinetypeDef { name: "Y".into(), pattern_length: 0.75, elements: vec![-0.25, 0.5], ..Default::default() };
        assert_eq!(compile_pattern(&def), vec![0.5, 0.25]);
        // too short or complex: continuous
        assert!(compile_pattern(&LinetypeDef { elements: vec![1.0], pattern_length: 1.0, ..Default::default() }).is_empty());
        assert!(compile_pattern(&LinetypeDef { elements: vec![1.0, -1.0], pattern_length: 2.0, complex: true, ..Default::default() }).is_empty());
    }
}
