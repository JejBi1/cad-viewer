//! State of the open 3D model: the OCCT document, the instance tree with its check boxes, expansion and selection,
//! the material assignments and the numbers shown in the properties panel.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use cad_core::fmt::fmt_len;
use cad_core::materials::{Assigned, Assignments};
use cad_core::meshmeasure::Mesh;
use cad_render::colors::rgb_to_srgb8;
use cad_types::{Aabb, MassProps, ModelTree, NodeId, ProtoId, ProtoMesh};
use occt_bridge::Document;

use crate::mass::{self, LeafMass, Totals};
use crate::props::{hex_name, PropRow, Props, Span};
use crate::util::base_name;

/// What the document loader hands over.
pub struct Loaded3d {
    pub doc: Document,
    pub tree: ModelTree,
    pub protos: Vec<ProtoMesh>,
    pub from_cache: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Off,
    Partial,
    On,
}

impl Check {
    /// What a click on this check box sets: every leaf below on or off. Qt's `ItemIsAutoTristate` (1.9) turns a
    /// partly checked box into a checked one, and only a fully checked box into an unchecked one.
    pub fn after_click(self) -> bool {
        self != Check::On
    }
}

/// An analysed STL mesh with the loops that were handed to the renderer as edges.
pub struct StlMesh {
    pub mesh: Mesh,
    /// The proto mesh the renderer shows (with the loops as edges), kept to put the model on the GPU again.
    pub proto: ProtoMesh,
}

pub struct Model3 {
    pub path: PathBuf,
    /// Name shown instead of the file name (a file downloaded from Google Drive).
    pub display_name: Option<String>,
    /// `None` while the interference check owns the document.
    pub doc: Option<Document>,
    pub tree: ModelTree,
    /// `ProtoMesh::mesh_only` per prototype.
    pub mesh_only: Vec<bool>,
    pub leaves_total: Vec<u32>,
    pub leaves_on: Vec<u32>,
    /// Leaves whose check box is off.
    pub hidden: HashSet<NodeId>,
    pub expanded: Vec<bool>,
    pub selected: BTreeSet<NodeId>,
    pub anchor: Option<NodeId>,
    /// Visible tree rows: node and depth.
    pub rows: Vec<(NodeId, u16)>,
    pub rows_dirty: bool,
    pub mats: Option<Assignments>,
    pub vol_cache: HashMap<ProtoId, MassProps>,
    /// Numbers were asked for while the document was away (zeros were shown): the panel needs a refresh.
    pub unmeasured: bool,
    /// Bounding box of the whole model.
    pub model_box: Option<Aabb>,
    pub stl: Option<StlMesh>,
    /// Row to bring into view at the next frame.
    pub scroll_to: Option<NodeId>,
    pub from_cache: bool,
}

/// `Úplná cesta ... \n` file name of a path for the title and the status line.
pub fn file_label(path: &std::path::Path, display: &Option<String>) -> String {
    display.clone().unwrap_or_else(|| base_name(path))
}

impl Model3 {
    pub fn new(path: PathBuf, display_name: Option<String>, loaded: Loaded3d, mats: Option<Assignments>) -> Model3 {
        let n = loaded.tree.nodes.len();
        let mut m = Model3 {
            path,
            display_name,
            mesh_only: loaded.protos.iter().map(|p| p.mesh_only).collect(),
            doc: Some(loaded.doc),
            tree: loaded.tree,
            leaves_total: vec![0; n],
            leaves_on: vec![0; n],
            hidden: HashSet::new(),
            expanded: vec![false; n],
            selected: BTreeSet::new(),
            anchor: None,
            rows: Vec::new(),
            rows_dirty: true,
            mats,
            vol_cache: HashMap::new(),
            unmeasured: false,
            model_box: None,
            stl: None,
            scroll_to: None,
            from_cache: loaded.from_cache,
        };
        // only the top level is expanded (main.build_tree_items)
        for &r in &m.tree.roots.clone() {
            m.expanded[r as usize] = true;
        }
        m.recount();
        m.model_box = m.nodes_box(&m.tree.roots.clone());
        m
    }

    pub fn node(&self, id: NodeId) -> &cad_types::TreeNode {
        &self.tree.nodes[id as usize]
    }

    pub fn has_nodes(&self) -> bool {
        !self.tree.nodes.is_empty()
    }

    /// Leaf nodes (parts) of the whole tree.
    pub fn all_leaves(&self) -> Vec<NodeId> {
        self.tree.nodes.iter().filter(|n| !n.is_assembly).map(|n| n.id).collect()
    }

    /// Leaves below a node (the node itself for a part), in tree order.
    pub fn leaves_of(&self, node: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            let n = &self.tree.nodes[id as usize];
            if n.is_assembly {
                stack.extend(n.children.iter().rev());
            } else {
                out.push(id);
            }
        }
        out
    }

    /// Unique leaves below several nodes.
    pub fn unique_leaves(&self, nodes: &[NodeId]) -> Vec<NodeId> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for &n in nodes {
            for l in self.leaves_of(n) {
                if seen.insert(l) {
                    out.push(l);
                }
            }
        }
        out
    }

    /// Nodes in pre-order (a parent before its children), from the roots.
    pub fn pre_order(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.tree.nodes.len());
        let mut stack: Vec<NodeId> = self.tree.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            stack.extend(self.tree.nodes[id as usize].children.iter().rev());
        }
        out
    }

    /// Recounts the checked leaves of every node (after a check box changed).
    pub fn recount(&mut self) {
        self.leaves_total.iter_mut().for_each(|c| *c = 0);
        self.leaves_on.iter_mut().for_each(|c| *c = 0);
        // walking the pre-order backwards adds every child to its parent before the parent is added to its own
        for id in self.pre_order().into_iter().rev() {
            let i = id as usize;
            let node = &self.tree.nodes[i];
            if !node.is_assembly {
                self.leaves_total[i] += 1;
                if !self.hidden.contains(&node.id) {
                    self.leaves_on[i] += 1;
                }
            }
            if let Some(p) = node.parent {
                let (t, o) = (self.leaves_total[i], self.leaves_on[i]);
                self.leaves_total[p as usize] += t;
                self.leaves_on[p as usize] += o;
            }
        }
    }

    pub fn check(&self, node: NodeId) -> Check {
        let (t, o) = (self.leaves_total[node as usize], self.leaves_on[node as usize]);
        if t == 0 || o == t {
            Check::On
        } else if o == 0 {
            Check::Off
        } else {
            Check::Partial
        }
    }

    pub fn is_visible_leaf(&self, leaf: NodeId) -> bool {
        !self.hidden.contains(&leaf)
    }

    /// Sets the check boxes of all leaves below `node`. Returns true when something changed.
    pub fn set_checked(&mut self, nodes: &[NodeId], on: bool) -> bool {
        let before = self.hidden.len();
        let snapshot = self.hidden.clone();
        for &n in nodes {
            for l in self.leaves_of(n) {
                if on {
                    self.hidden.remove(&l);
                } else {
                    self.hidden.insert(l);
                }
            }
        }
        let changed = self.hidden != snapshot || self.hidden.len() != before;
        if changed {
            self.recount();
        }
        changed
    }

    // ------------------------------------------------------------------ tree rows

    /// Rebuilds the visible rows. With a filter text only nodes that match or have a matching descendant are shown;
    /// the ancestors of matches are expanded (`apply_filter`).
    pub fn rebuild_rows(&mut self, filter: &str) {
        let text = filter.trim().to_lowercase();
        self.rows.clear();
        let roots = self.tree.roots.clone();
        if text.is_empty() {
            for r in roots {
                self.push_rows(r, 0);
            }
        } else {
            let n = self.tree.nodes.len();
            let mut shown = vec![false; n];
            for id in self.pre_order().into_iter().rev() {
                let i = id as usize;
                let node = &self.tree.nodes[i];
                let matches = node.name.to_lowercase().contains(&text) || node.proto_name.to_lowercase().contains(&text);
                if matches {
                    shown[i] = true;
                }
                if shown[i] {
                    if let Some(p) = node.parent {
                        shown[p as usize] = true;
                        // a matching descendant opens its ancestors
                        self.expanded[p as usize] = true;
                    }
                }
            }
            for r in roots {
                self.push_filtered(r, 0, &shown);
            }
        }
        self.rows_dirty = false;
    }

    fn push_rows(&mut self, node: NodeId, depth: u16) {
        // iterative: assemblies can be nested 200 levels deep and have 300 000 nodes
        let mut stack: Vec<(NodeId, u16)> = vec![(node, depth)];
        while let Some((id, d)) = stack.pop() {
            self.rows.push((id, d));
            if self.expanded[id as usize] {
                for &c in self.tree.nodes[id as usize].children.iter().rev() {
                    stack.push((c, d + 1));
                }
            }
        }
    }

    fn push_filtered(&mut self, node: NodeId, depth: u16, shown: &[bool]) {
        let mut stack: Vec<(NodeId, u16)> = vec![(node, depth)];
        while let Some((id, d)) = stack.pop() {
            if !shown[id as usize] {
                continue;
            }
            self.rows.push((id, d));
            if self.expanded[id as usize] {
                for &c in self.tree.nodes[id as usize].children.iter().rev() {
                    stack.push((c, d + 1));
                }
            }
        }
    }

    pub fn expand_ancestors(&mut self, node: NodeId) {
        let mut p = self.tree.nodes[node as usize].parent;
        while let Some(a) = p {
            if !self.expanded[a as usize] {
                self.expanded[a as usize] = true;
                self.rows_dirty = true;
            }
            p = self.tree.nodes[a as usize].parent;
        }
    }

    pub fn expand_all(&mut self) {
        for (i, n) in self.tree.nodes.iter().enumerate() {
            self.expanded[i] = !n.children.is_empty();
        }
        self.rows_dirty = true;
    }

    /// `collapse_tree`: everything collapsed, the top level open.
    pub fn collapse_all(&mut self) {
        self.expanded.iter_mut().for_each(|e| *e = false);
        for &r in &self.tree.roots {
            self.expanded[r as usize] = true;
        }
        self.rows_dirty = true;
    }

    // ------------------------------------------------------------------ selection

    pub fn select_only(&mut self, nodes: impl IntoIterator<Item = NodeId>) {
        self.selected = nodes.into_iter().collect();
    }

    pub fn selection(&self) -> Vec<NodeId> {
        self.selected.iter().copied().collect()
    }

    /// Row click with Qt's ExtendedSelection rules: plain = replace, Ctrl = toggle, Shift = range from the anchor.
    pub fn click_row(&mut self, node: NodeId, ctrl: bool, shift: bool) {
        if shift {
            if let Some(a) = self.anchor {
                let pos = |n: NodeId| self.rows.iter().position(|(r, _)| *r == n);
                if let (Some(i), Some(j)) = (pos(a), pos(node)) {
                    let (lo, hi) = (i.min(j), i.max(j));
                    let range: Vec<NodeId> = self.rows[lo..=hi].iter().map(|(r, _)| *r).collect();
                    if !ctrl {
                        self.selected.clear();
                    }
                    self.selected.extend(range);
                    return;
                }
            }
        }
        if ctrl {
            if !self.selected.remove(&node) {
                self.selected.insert(node);
            }
        } else {
            self.selected.clear();
            self.selected.insert(node);
        }
        self.anchor = Some(node);
    }

    // ------------------------------------------------------------------ numbers

    pub fn proto_name_of(&self, leaf: NodeId) -> String {
        let n = self.node(leaf);
        if n.proto_name.is_empty() { n.key.clone() } else { n.proto_name.clone() }
    }

    pub fn material_of(&self, leaf: NodeId) -> Option<Assigned> {
        self.mats.as_ref()?.get(&self.proto_name_of(leaf), true)
    }

    /// Number of instances of the part's prototype in the model.
    pub fn instance_count(&self, leaf: NodeId) -> u32 {
        self.node(leaf).proto.and_then(|p| self.tree.proto_instances.get(p as usize).copied()).unwrap_or(1).max(1)
    }

    /// Mass properties of a prototype, cached. While the interference check owns the document (`doc` is `None`)
    /// nothing can be measured: zeros come back and are *not* cached, or the part would keep a zero volume after the
    /// check (see [`Model3::needs_recalc`]).
    pub fn proto_props(&mut self, proto: ProtoId) -> MassProps {
        if let Some(p) = self.vol_cache.get(&proto) {
            return *p;
        }
        let Some(doc) = self.doc.as_ref() else {
            self.unmeasured = true;
            return MassProps::default();
        };
        let p = doc.mass_props(proto);
        self.vol_cache.insert(proto, p);
        p
    }

    /// True once after a request for numbers had to be answered with zeros because the document was away; the caller
    /// that gives the document back refreshes the panel then.
    pub fn needs_recalc(&mut self) -> bool {
        std::mem::take(&mut self.unmeasured)
    }

    /// Prototypes of these leaves that are not in the volume cache yet.
    pub fn uncached_protos(&self, leaves: &[NodeId]) -> usize {
        leaves
            .iter()
            .filter_map(|l| self.node(*l).proto)
            .filter(|p| !self.vol_cache.contains_key(p))
            .collect::<HashSet<_>>()
            .len()
    }

    /// `mass_props`: totals over leaf instances.
    pub fn mass_totals(&mut self, leaves: &[NodeId]) -> Totals {
        let mut items = Vec::with_capacity(leaves.len());
        for &l in leaves {
            let node = self.node(l).clone();
            let Some(p) = node.proto else { continue };
            let mp = self.proto_props(p);
            items.push(LeafMass {
                volume_mm3: mp.volume_mm3,
                area_mm2: mp.area_mm2,
                centroid: mass::transform_point(&node.location, mp.centroid),
                solids: mp.solids,
                faces: mp.faces,
                material: self.material_of(l).map(|a| (a.name, a.density)),
            });
        }
        mass::totals(&items)
    }

    pub fn node_box(&self, node: NodeId) -> Option<Aabb> {
        self.doc.as_ref()?.node_bounds(node)
    }

    /// Union of the boxes of several nodes.
    pub fn nodes_box(&self, nodes: &[NodeId]) -> Option<Aabb> {
        let mut out: Option<Aabb> = None;
        for &n in nodes {
            if let Some(b) = self.node_box(n) {
                out = Some(match out {
                    None => b,
                    Some(a) => Aabb {
                        min: [a.min[0].min(b.min[0]), a.min[1].min(b.min[1]), a.min[2].min(b.min[2])],
                        max: [a.max[0].max(b.max[0]), a.max[1].max(b.max[1]), a.max[2].max(b.max[2])],
                    },
                });
            }
        }
        out
    }

    pub fn is_stl_leaf(&self, leaf: NodeId) -> bool {
        self.stl.is_some() && self.node(leaf).proto.is_some_and(|p| self.mesh_only.get(p as usize).copied().unwrap_or(false))
    }

    // ------------------------------------------------------------------ tree column texts

    /// Text of the "Info" column.
    pub fn info_text(&self, node: NodeId) -> String {
        let n = self.node(node);
        if n.is_assembly {
            return format!("{} ks", self.leaves_total[node as usize]);
        }
        let mat = self.material_of(node);
        mass::tree_info(self.instance_count(node), mat.as_ref().map(|a| (a.name.as_str(), a.explicit)))
    }

    pub fn tooltip(&self, node: NodeId) -> String {
        let n = self.node(node);
        let mut tip;
        if n.is_assembly {
            tip = format!("Sestava: {}", n.proto_name);
        } else {
            let cnt = self.instance_count(node);
            tip = format!("Díl: {}", n.proto_name);
            if cnt > 1 {
                tip.push_str(&format!("\nPoužit {cnt}× v modelu"));
            }
        }
        if n.name != n.proto_name && !n.proto_name.is_empty() {
            tip = format!("Instance: {}\n{tip}", n.name);
        }
        tip
    }

    // ------------------------------------------------------------------ properties panel

    fn size_rows(b: &Aabb) -> (PropRow, PropRow, PropRow) {
        let d = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
        (
            PropRow::bold("Rozměr X×Y×Z", format!("{} × {} × {} mm", fmt_len(d[0]), fmt_len(d[1]), fmt_len(d[2]))),
            PropRow::text("Min (X, Y, Z)", format!("{}, {}, {}", fmt_len(b.min[0]), fmt_len(b.min[1]), fmt_len(b.min[2]))),
            PropRow::text("Max (X, Y, Z)", format!("{}, {}, {}", fmt_len(b.max[0]), fmt_len(b.max[1]), fmt_len(b.max[2]))),
        )
    }

    /// Dimensions text of the status line: `X × Y × Z mm`.
    pub fn size_text(b: &Aabb) -> String {
        format!(
            "{} × {} × {}",
            fmt_len(b.max[0] - b.min[0]),
            fmt_len(b.max[1] - b.min[1]),
            fmt_len(b.max[2] - b.min[2])
        )
    }

    /// `show_model_info`: the panel when nothing is selected.
    pub fn model_info(&self, load_secs: f64) -> Props {
        let size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let parts = self.tree.nodes.iter().filter(|n| !n.is_assembly).count();
        let asms = self.tree.nodes.len() - parts;
        let mut rows = vec![
            PropRow::text("Složka", self.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
            PropRow::text("Velikost", crate::util::size_mb(size)),
            PropRow::text("Dílů", format!("{parts} ({} unikátních)", self.tree.proto_instances.len())),
            PropRow::text("Sestav", asms.to_string()),
        ];
        if let Some(b) = &self.model_box {
            rows.push(Self::size_rows(b).0);
        }
        rows.push(PropRow::text(
            "Načteno za",
            format!("{:.1} s{}", load_secs, if self.from_cache { " (z mezipaměti)" } else { "" }),
        ));
        Props::rows(Some(file_label(&self.path, &self.display_name)), rows)
    }

    /// `show_props`: the panel of one selected node.
    pub fn node_props(&mut self, node: NodeId) -> Props {
        let n = self.node(node).clone();
        let leaves = self.leaves_of(node);
        let mut rows = vec![PropRow::text("Název", n.name.clone())];
        if !n.proto_name.is_empty() && n.proto_name != n.name {
            rows.push(PropRow::text(if n.is_assembly { "Sestava" } else { "Díl / dokument" }, n.proto_name.clone()));
        }
        rows.push(PropRow::text("Typ", if n.is_assembly { "sestava" } else { "díl" }));
        if n.is_assembly {
            rows.push(PropRow::text("Obsahuje", format!("{} dílů, {} přímých položek", leaves.len(), n.children.len())));
        } else {
            rows.push(PropRow::text("Počet v modelu", format!("{}×", self.instance_count(node))));
        }
        if let Some(b) = self.node_box(node) {
            let (a, mn, mx) = Self::size_rows(&b);
            rows.extend([a, mn, mx]);
        } else if self.doc.is_none() {
            // the check owns the document: the size comes when the panel is built again (`needs_recalc`)
            self.unmeasured = true;
            rows.push(PropRow::text("Rozměr X×Y×Z", crate::strings::COMPUTING_AFTER_CHECK));
        }
        if let Some(c) = n.color {
            let name = hex_name(rgb_to_srgb8(c));
            rows.push(PropRow::spans("Barva", vec![Span::Swatch(rgb_to_srgb8(c), name)]));
        }
        if !n.is_assembly && self.is_stl_leaf(node) {
            if let Some(s) = self.stl.as_mut() {
                for (k, v) in s.mesh.info_rows() {
                    rows.push(PropRow::text(k, v));
                }
            }
        }
        if !n.is_assembly || self.uncached_protos(&leaves) <= 60 {
            let totals = self.mass_totals(&leaves);
            rows.extend(mass::rows(&totals, Some(node)));
        } else {
            let k = self.uncached_protos(&leaves);
            rows.push(PropRow::spans(
                "Objem / hmotnost",
                vec![Span::Link(
                    format!("spočítat ({k} unikátních dílů)"),
                    crate::props::LinkTarget::Calc(node),
                )],
            ));
        }
        Props::rows(None, rows)
    }

    /// The panel of several selected nodes.
    pub fn multi_props(&mut self, nodes: &[NodeId]) -> Props {
        let lv = self.unique_leaves(nodes);
        let mut rows = vec![PropRow::text("Vybráno", format!("{} položek, {} dílů", nodes.len(), lv.len()))];
        if let Some(b) = self.nodes_box(nodes) {
            rows.push(Self::size_rows(&b).0);
        } else if self.doc.is_none() {
            self.unmeasured = true;
            rows.push(PropRow::text("Rozměr X×Y×Z", crate::strings::COMPUTING_AFTER_CHECK));
        }
        Props::rows(None, rows)
    }

    /// Row text of the part name for the clipboard ("Kopírovat název").
    pub fn names_of(&self, nodes: &[NodeId]) -> String {
        nodes.iter().map(|n| self.node(*n).name.clone()).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_types::{TreeNode, IDENTITY};

    fn node(id: NodeId, parent: Option<NodeId>, name: &str, asm: bool, proto: Option<ProtoId>) -> TreeNode {
        TreeNode {
            id,
            parent,
            children: Vec::new(),
            name: name.into(),
            proto_name: name.into(),
            key: format!("0:1:{id}"),
            is_assembly: asm,
            proto,
            location: IDENTITY,
            color: None,
            own_style: false,
        }
    }

    /// root (asm) -> [sub (asm) -> [a, b], c]
    pub(crate) fn sample_tree() -> ModelTree {
        let mut nodes = vec![
            node(0, None, "Sestava", true, None),
            node(1, Some(0), "Podsestava", true, None),
            node(2, Some(1), "Šroub", false, Some(0)),
            node(3, Some(1), "Matice", false, Some(1)),
            node(4, Some(0), "Deska", false, Some(2)),
        ];
        for i in 1..nodes.len() {
            let p = nodes[i].parent.unwrap() as usize;
            let id = nodes[i].id;
            nodes[p].children.push(id);
        }
        ModelTree { roots: vec![0], nodes, proto_instances: vec![1, 1, 1] }
    }

    /// A model without a document, for tests of the tree logic.
    pub(crate) fn bare(tree: ModelTree) -> Model3 {
        let n = tree.nodes.len();
        let mut m = Model3 {
            path: PathBuf::from("model.step"),
            display_name: None,
            doc: None,
            mesh_only: vec![false; tree.proto_instances.len()],
            tree,
            leaves_total: vec![0; n],
            leaves_on: vec![0; n],
            hidden: HashSet::new(),
            expanded: vec![false; n],
            selected: BTreeSet::new(),
            anchor: None,
            rows: Vec::new(),
            rows_dirty: true,
            mats: None,
            vol_cache: HashMap::new(),
            unmeasured: false,
            model_box: None,
            stl: None,
            scroll_to: None,
            from_cache: false,
        };
        m.expanded[0] = true;
        m.recount();
        m
    }

    #[test]
    fn clicking_a_partly_checked_assembly_checks_it() {
        let mut m = bare(sample_tree());
        assert!(m.set_checked(&[2], false));
        assert_eq!(m.check(1), Check::Partial);
        // as in Qt: off -> on, partly -> on, on -> off
        assert!(Check::Off.after_click());
        assert!(Check::Partial.after_click());
        assert!(!Check::On.after_click());
        // the whole sequence on the tree: the click on the partly checked assembly shows both bolts again
        assert!(m.set_checked(&[1], m.check(1).after_click()));
        assert_eq!(m.check(1), Check::On);
        assert!(m.hidden.is_empty());
        // and the next click switches the assembly off
        assert!(m.set_checked(&[1], m.check(1).after_click()));
        assert_eq!(m.check(1), Check::Off);
    }

    #[test]
    fn check_states_follow_the_leaves() {
        let mut m = bare(sample_tree());
        assert_eq!(m.check(0), Check::On);
        assert!(m.set_checked(&[2], false));
        assert_eq!(m.check(2), Check::Off);
        assert_eq!(m.check(1), Check::Partial);
        assert_eq!(m.check(0), Check::Partial);
        assert!(m.set_checked(&[3], false));
        assert_eq!(m.check(1), Check::Off);
        assert_eq!(m.check(0), Check::Partial);
        // clicking an assembly sets every leaf below
        assert!(m.set_checked(&[0], true));
        assert_eq!(m.check(0), Check::On);
        assert!(m.hidden.is_empty());
        assert!(m.set_checked(&[0], false));
        assert_eq!(m.check(0), Check::Off);
        assert_eq!(m.hidden.len(), 3);
        // setting what is already set reports no change
        assert!(!m.set_checked(&[0], false));
    }

    #[test]
    fn rows_follow_expansion_and_filter() {
        let mut m = bare(sample_tree());
        m.rebuild_rows("");
        // only the top level is open
        assert_eq!(m.rows, vec![(0, 0), (1, 1), (4, 1)]);
        m.expand_all();
        m.rebuild_rows("");
        assert_eq!(m.rows, vec![(0, 0), (1, 1), (2, 2), (3, 2), (4, 1)]);
        m.collapse_all();
        m.rebuild_rows("");
        assert_eq!(m.rows.len(), 3);
        // the filter shows matches and their ancestors and opens them
        m.rebuild_rows("šroub");
        assert_eq!(m.rows, vec![(0, 0), (1, 1), (2, 2)]);
        m.rebuild_rows("DESKA");
        assert_eq!(m.rows, vec![(0, 0), (4, 1)]);
        m.rebuild_rows("zzz");
        assert!(m.rows.is_empty());
    }

    #[test]
    fn selection_rules() {
        let mut m = bare(sample_tree());
        m.expand_all();
        m.rebuild_rows("");
        m.click_row(2, false, false);
        assert_eq!(m.selection(), vec![2]);
        m.click_row(4, true, false);
        assert_eq!(m.selection(), vec![2, 4]);
        m.click_row(2, true, false);
        assert_eq!(m.selection(), vec![4]);
        m.click_row(1, false, false);
        m.click_row(4, false, true);
        // range from the anchor (1) to 4 in row order: 1, 2, 3, 4
        assert_eq!(m.selection(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn leaves_and_tree_texts() {
        let mut m = bare(sample_tree());
        assert_eq!(m.leaves_of(1), vec![2, 3]);
        assert_eq!(m.unique_leaves(&[0, 1]), vec![2, 3, 4]);
        assert_eq!(m.info_text(0), "3 ks");
        assert_eq!(m.info_text(2), "");
        m.tree.proto_instances[0] = 4;
        assert_eq!(m.info_text(2), "×4");
        assert_eq!(m.tooltip(2), "Díl: Šroub\nPoužit 4× v modelu");
        assert_eq!(m.tooltip(0), "Sestava: Sestava");
        m.tree.nodes[2].name = "Šroub<1>".into();
        assert_eq!(m.tooltip(2), "Instance: Šroub<1>\nDíl: Šroub\nPoužit 4× v modelu");
        m.expand_ancestors(3);
        assert!(m.expanded[1]);
    }

    #[test]
    fn model_info_rows() {
        let mut m = bare(sample_tree());
        m.model_box = Some(Aabb { min: [0.0, 0.0, 0.0], max: [100.0, 50.0, 1234.5] });
        let Props::Rows { title, rows } = m.model_info(1.25) else { panic!() };
        assert_eq!(title.as_deref(), Some("model.step"));
        let get = |l: &str| rows.iter().find(|r| r.label == l).map(|r| r.value_text());
        assert_eq!(get("Dílů").unwrap(), "3 (3 unikátních)");
        assert_eq!(get("Sestav").unwrap(), "2");
        assert_eq!(get("Rozměr X×Y×Z").unwrap(), "100,00 × 50,00 × 1 234,50 mm");
        assert_eq!(get("Načteno za").unwrap(), "1.2 s");
        m.from_cache = true;
        let Props::Rows { rows, .. } = m.model_info(0.04) else { panic!() };
        assert_eq!(rows.last().unwrap().value_text(), "0.0 s (z mezipaměti)");
    }
}
