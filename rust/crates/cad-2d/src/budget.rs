//! Resource limits: entities a layout expands to (port of `view2d.expanded_entity_count`).
use crate::dxf::*;
use std::collections::HashMap;

/// Entities a layout may expand to through INSERTs (`view2d.MAX_EXPANDED_ENTITIES` of the Python viewer, which counted
/// nothing else: a DIMENSION was one entity there).
pub const MAX_EXPANDED_ENTITIES: usize = 300_000;
/// Entities that the blocks of DIMENSIONs, ACAD_TABLEs and MLEADERs may add up to. Real drawings are full of dimensions
/// (30 000 dimensions of ten entities each are 300 000) and cannot be told from a bomb by this number alone, so it is
/// much higher; cycles, the depth of the nesting and the amplification rule below still catch the bombs.
pub const MAX_CONTENT_ENTITIES: usize = 6_000_000;
/// The absolute limit alone still lets a 25 KB file through that expands to 211 000 entities (minutes in the GUI
/// thread). Real drawings re-use blocks, but not by a factor of hundreds: many more drawn entities than entity
/// records in the file means nested re-use, i.e. a bomb.
pub const AMPLIFY_MIN: usize = 50_000;
pub const AMPLIFY_RATIO: usize = 100;
pub const MAX_BLOCK_DEPTH: usize = 100;

/// The layout would expand to too many entities (or contains a cyclic block reference).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DrawingTooComplex(pub String);

fn spaced(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Entities one block expands to: `ins` through INSERTs (what the Python viewer counted), `cont` through the blocks of
/// DIMENSIONs, ACAD_TABLEs and MLEADERs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Count {
    ins: usize,
    cont: usize,
}

impl Count {
    fn total(&self) -> usize {
        self.ins.saturating_add(self.cont)
    }
}

/// What an entity is replaced by: the block, how many instances of it are drawn, and whether it is the block of a
/// DIMENSION, ACAD_TABLE or MLEADER (counted apart from INSERTs).
struct Expansion<'e> {
    name: std::borrow::Cow<'e, str>,
    times: usize,
    content: bool,
}

struct Counter<'a> {
    doc: &'a Document,
    limit: usize,
    content_limit: usize,
    memo: HashMap<String, Count>,
    active: Vec<String>,
    stored: usize,
}

impl<'a> Counter<'a> {
    fn over(&self) -> DrawingTooComplex {
        DrawingTooComplex(format!(
            "Výkres se rozvinutím bloků rozpadne na více než {} entit – to je příliš (pravděpodobně vnořené bloky, které se mnohonásobně opakují). Výkres se nevykreslí.",
            spaced(self.limit)
        ))
    }

    fn over_content(&self) -> DrawingTooComplex {
        DrawingTooComplex(format!(
            "Výkres se rozvinutím kót, tabulek a vícenásobných odkazů rozpadne na více než {} entit – to je příliš (pravděpodobně vnořené bloky, které se mnohonásobně opakují). Výkres se nevykreslí.",
            spaced(self.content_limit)
        ))
    }

    fn block_count(&mut self, name: &str) -> Result<Count, DrawingTooComplex> {
        let key = name.to_lowercase();
        if let Some(&n) = self.memo.get(&key) {
            return Ok(n);
        }
        if self.active.contains(&key) {
            return Err(DrawingTooComplex(format!(
                "Cyklický blok: „{}“ se přímo nebo nepřímo vkládá sám do sebe – výkres nelze vykreslit.",
                name
            )));
        }
        if self.active.len() >= MAX_BLOCK_DEPTH {
            return Err(DrawingTooComplex(format!(
                "Bloky jsou vnořené příliš hluboko (více než {} úrovní) – výkres nelze vykreslit.",
                MAX_BLOCK_DEPTH
            )));
        }
        let Some(b) = self.doc.blocks.get(&key) else {
            self.memo.insert(key, Count::default());
            return Ok(Count::default());
        };
        self.active.push(key.clone());
        let r = self.entities_count(&b.entities);
        self.active.pop();
        let n = r?;
        self.memo.insert(key, n);
        Ok(n)
    }

    /// The block an entity expands to. Every kind of entity that the frontend replaces by the content of a block counts:
    /// INSERT / MINSERT, ACAD_TABLE and DIMENSION (their anonymous blocks), MLEADER with block content.
    fn expansion<'e>(&self, e: &'e Entity) -> Option<Expansion<'e>> {
        match &e.d {
            Data::Insert(i) if !i.from_inventor => {
                Some(Expansion { name: std::borrow::Cow::Borrowed(i.name.as_str()), times: i.instances(), content: i.table })
            }
            Data::Dimension(d) if !d.block.is_empty() => Some(Expansion { name: std::borrow::Cow::Borrowed(d.block.as_str()), times: 1, content: true }),
            Data::MLeader(m) if m.content_type == 1 && m.block_handle != 0 => {
                let key = self.doc.block_by_record.get(&m.block_handle)?;
                Some(Expansion { name: std::borrow::Cow::Owned(key.clone()), times: 1, content: true })
            }
            _ => None,
        }
    }

    fn entities_count(&mut self, ents: &[Entity]) -> Result<Count, DrawingTooComplex> {
        let mut total = Count { ins: ents.len(), cont: 0 };
        self.stored += ents.len();
        for e in ents {
            if let Some(x) = self.expansion(e) {
                if let Data::Insert(i) = &e.d {
                    total.ins = total.ins.saturating_add(i.attribs.len());
                }
                let inner = self.block_count(&x.name)?;
                if x.content {
                    // all of the block (and what it contains in turn) is content of this dimension / table / leader
                    let n = inner.total();
                    let work = if x.times <= 1 { n } else { x.times.saturating_mul(n.max(1)) };
                    total.cont = total.cont.saturating_add(work);
                } else {
                    // a block reference draws its block once, an array once per instance; an empty block still costs the
                    // instance (a 2 000 000 000 x 2 000 000 000 array of an empty block passed as "0 entities")
                    let (wi, wc) = if x.times <= 1 {
                        (inner.ins, inner.cont)
                    } else {
                        (x.times.saturating_mul(inner.ins.max(1)), x.times.saturating_mul(inner.cont))
                    };
                    total.ins = total.ins.saturating_add(wi);
                    total.cont = total.cont.saturating_add(wc);
                }
            }
            if total.ins > self.limit {
                return Err(self.over());
            }
            if total.cont > self.content_limit {
                return Err(self.over_content());
            }
        }
        Ok(total)
    }
}

/// The scene of a layout reached the limit of recorded path elements (`scene::MAX_SCENE_ELEMS`), of output vertices
/// (`scene::MAX_OUT_VERTS`) or of primitives: the last resort behind the entity budget, far above what a real drawing
/// needs (dashes, hatch lines and glyph outlines are made simpler long before, texts and fills are drawn as boxes
/// before it comes to this). It depends on the memory that is free, so it is not remembered: the next scene is
/// worked out again.
pub fn scene_too_large(max_elems: usize) -> DrawingTooComplex {
    DrawingTooComplex(format!(
        "Výkres se při vykreslení rozpadne na více než {} bodů čar a tvarů – to je příliš (pravděpodobně velmi jemné přerušované čáry, šrafování nebo obrovský text). Výkres se nevykreslí.",
        spaced(max_elems)
    ))
}

/// Note about shapes whose detail was reduced to keep a very large drawing in memory.
pub fn note_simplified(shapes: usize) -> String {
    format!(
        "Velmi rozsáhlý výkres: u {} tvarů a textů je pro úsporu paměti zjednodušen obrys (hrubší křivky, bez obrysové čáry). Všechny entity zůstávají zobrazeny.",
        spaced(shapes)
    )
}

/// Note about shapes (texts above all) that are drawn as a box only, because even the simplified outlines would not fit
/// in the memory that is free.
pub fn note_boxed(shapes: usize) -> String {
    format!(
        "Velmi rozsáhlý výkres: u {} tvarů a textů je pro úsporu paměti místo obrysu nakreslen jen obdélník jejich rozsahu (bez písmen). Všechny entity zůstávají zobrazeny.",
        spaced(shapes)
    )
}

/// Note about lines whose dashes would be too many and are drawn as one solid line.
pub fn note_dashes(lines: usize) -> String {
    format!(
        "{} čar má příliš hustě čárkování (miliony čárek): jsou vykresleny plnou čarou, aby se výkres vešel do paměti.",
        spaced(lines)
    )
}

/// Note about a scene that stopped at its time limit.
pub fn note_timeout(seconds: u64) -> String {
    format!(
        "Vykreslování výkresu překročilo časový limit ({} s): zobrazena je jen část entit. Zbytek se nevykreslil.",
        seconds
    )
}

/// Note about hatches that are filled solid because the pattern lines would take too long or be too many.
pub fn note_hatches(n: usize) -> String {
    format!(
        "{} šrafování má příliš hustý vzor nebo se jeho kreslení nestihlo v časovém limitu: je vyplněno plnou barvou.",
        spaced(n)
    )
}

/// Entities that would be drawn for the layout; `ents` are its entities, `is_model` false adds the model space once
/// per active viewport. Error above `limit` or for cyclic / too deeply nested blocks.
pub fn expanded_entity_count(doc: &Document, ents: &[Entity], is_model: bool, limit: usize) -> Result<usize, DrawingTooComplex> {
    expanded_entity_count_with(doc, ents, is_model, limit, MAX_CONTENT_ENTITIES)
}

/// `expanded_entity_count` with its own limit for the content of dimensions, tables and leaders.
pub fn expanded_entity_count_with(doc: &Document, ents: &[Entity], is_model: bool, limit: usize, content_limit: usize) -> Result<usize, DrawingTooComplex> {
    let mut c = Counter { doc, limit, content_limit, memo: HashMap::new(), active: Vec::new(), stored: 0 };
    let mut total = c.entities_count(ents)?;
    if !is_model {
        let views = ents.iter().filter(|e| matches!(&e.d, Data::Viewport(v) if v.status > 0)).count();
        if views > 0 {
            let msp = doc.layout_entities.get("model").map(|v| v.as_slice()).unwrap_or(&[]);
            let m = c.entities_count(msp)?;
            total.ins = total.ins.saturating_add(views.saturating_mul(m.ins));
            total.cont = total.cont.saturating_add(views.saturating_mul(m.cont));
        }
    }
    if total.ins > limit {
        return Err(c.over());
    }
    if total.cont > content_limit {
        return Err(c.over_content());
    }
    let all = total.total();
    if all > AMPLIFY_MIN && all > AMPLIFY_RATIO * c.stored {
        return Err(DrawingTooComplex(format!(
            "Výkres má v souboru jen {} entit, ale rozvinutím vnořených bloků by jich vykreslil {} ({}× víc) – to vypadá jako záměrně vytvořená „bomba“ a vykreslování by trvalo minuty. Výkres se nevykreslí.",
            c.stored,
            all,
            all / c.stored.max(1)
        )));
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_as_thousands_separator() {
        assert_eq!(spaced(300_000), "300 000");
        assert_eq!(spaced(999), "999");
        assert_eq!(spaced(1_234_567), "1 234 567");
    }
}
