//! Snapping for the construction tools, as pure functions of the road graph.
//!
//! Roads are built between cell centres along the 8 straight / 45-degree
//! directions, so everything here works on the cell lattice: every road and
//! every guide is a line of one of four *families* (east-west, north-south
//! and the two diagonals), described by a normal coordinate `n` (constant
//! along the line) and an along coordinate `p` (see [`fam_n`], [`fam_p`]).
//! Intersections of two such lines are found exactly with integers.
//!
//! [`snap_point`] snaps a free point (the start of a road, or a block
//! corner); [`snap_leg`] snaps the end of a leg drawn from a placed corner.
//! Candidates, strongest first: an existing node, a point on an existing
//! road (a T-junction), the crossing of two guides, an extension of a road
//! end, an alignment perpendicular to a nearby road through one of its
//! nodes, the spacing of the neighbouring block, then the plain straight /
//! 45-degree ray (legs) or the grid cell (points).

use citysim_sim::{CELL_SIZE, Cell, NodeId, RoadKind, SegmentId, World, geom};

use crate::shared::{Guide, GuideKind, SnapTarget};

/// The 8 lattice directions, east first, clockwise in sim space (+y down).
pub const DIRS8: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// How far (cells) guides reach from the node that makes them.
const GUIDE_REACH: i32 = 48;
/// Nodes and roads further than this (cells) from the cursor are ignored.
const LOOK_RADIUS: i32 = 40;
/// Parallel roads up to this far (cells) count for the parallel snap.
const PARALLEL_REACH: i32 = 14;

/// Line family of a lattice direction: 0 east-west, 1 north-south,
/// 2 the `(1, 1)` diagonal, 3 the `(1, -1)` diagonal.
pub fn family(d: (i32, i32)) -> usize {
    match (d.0.signum(), d.1.signum()) {
        (_, 0) => 0,
        (0, _) => 1,
        (a, b) if a == b => 2,
        _ => 3,
    }
}

/// The family at right angles.
pub fn perpendicular(f: usize) -> usize {
    [1, 0, 3, 2][f]
}

/// Coefficients `(a, b)` of a family's normal coordinate `n = a x + b y`.
fn fam_ab(f: usize) -> (i32, i32) {
    [(0, 1), (1, 0), (1, -1), (1, 1)][f]
}

/// Normal coordinate: the same for every cell of one line of family `f`.
pub fn fam_n(f: usize, c: Cell) -> i32 {
    let (a, b) = fam_ab(f);
    a * c.x + b * c.y
}

/// Along coordinate: increases by one per step along a line of family `f`.
pub fn fam_p(f: usize, c: Cell) -> i32 {
    if f == 1 { c.y } else { c.x }
}

/// The cell at normal `n` and along `p` of family `f`.
pub fn fam_cell(f: usize, n: i32, p: i32) -> Cell {
    match f {
        0 => Cell::new(p, n),
        1 => Cell::new(n, p),
        2 => Cell::new(p, p - n),
        _ => Cell::new(p, n - p),
    }
}

/// Distance in cells between neighbouring lines (`n` and `n + 1`).
pub fn normal_unit(f: usize) -> f32 {
    if f < 2 { 1.0 } else { std::f32::consts::FRAC_1_SQRT_2 }
}

/// Exact crossing cell of line `(f1, n1)` and line `(f2, n2)`, if they
/// cross at a cell centre.
pub fn cross(f1: usize, n1: i32, f2: usize, n2: i32) -> Option<Cell> {
    if f1 == f2 {
        return None;
    }
    let (a1, b1) = fam_ab(f1);
    let (a2, b2) = fam_ab(f2);
    let det = a1 * b2 - a2 * b1;
    let xn = n1 * b2 - n2 * b1;
    let yn = a1 * n2 - a2 * n1;
    (xn % det == 0 && yn % det == 0).then(|| Cell::new(xn / det, yn / det))
}

fn dist_m(c: Cell, p: [f32; 2]) -> f32 {
    geom::dist(c.center(), p)
}

/// A straight road segment on the lattice.
#[derive(Debug, Clone, PartialEq)]
pub struct RoadLine {
    pub seg: SegmentId,
    pub a: Cell,
    pub b: Cell,
    pub kind: RoadKind,
    pub fam: usize,
    pub n: i32,
    /// Along-range, `p_min <= p_max`.
    pub p_min: i32,
    pub p_max: i32,
}

impl RoadLine {
    pub fn contains(&self, c: Cell) -> bool {
        fam_n(self.fam, c) == self.n && (self.p_min..=self.p_max).contains(&fam_p(self.fam, c))
    }

    /// Distance (m) from `p` to the segment.
    pub fn dist_to(&self, p: [f32; 2]) -> f32 {
        let (a, b) = (self.a.center(), self.b.center());
        let ab = geom::sub(b, a);
        let len2 = geom::dot(ab, ab);
        let t = if len2 > 0.0 {
            (geom::dot(geom::sub(p, a), ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        geom::dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
    }

    /// The lattice cell of this road nearest to `p`.
    pub fn nearest_cell(&self, p: [f32; 2]) -> Cell {
        let f = self.fam;
        // Along coordinate of p (fractional): x for families 0, 2, 3; y for 1.
        let cp = if f == 1 {
            p[1] / CELL_SIZE - 0.5
        } else {
            p[0] / CELL_SIZE - 0.5
        };
        let along = match f {
            0 | 1 => cp,
            // Diagonals: project onto the line's direction.
            _ => {
                let a = self.a.center();
                let d = if f == 2 { [1.0, 1.0] } else { [1.0, -1.0] };
                let t = geom::dot(geom::sub(p, a), d) / (2.0 * CELL_SIZE);
                fam_p(f, self.a) as f32 + t
            }
        };
        let pp = (along.round() as i32).clamp(self.p_min, self.p_max);
        fam_cell(f, self.n, pp)
    }
}

/// A node with the directions of its arms.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub id: NodeId,
    pub cell: Cell,
    /// Unit lattice directions from the node along each arm.
    pub arms: Vec<(i32, i32)>,
    pub highway: bool,
}

/// The road graph as lattice lines, rebuilt when the roads change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoadIndex {
    pub lines: Vec<RoadLine>,
    pub nodes: Vec<NodeInfo>,
}

impl RoadIndex {
    pub fn build(world: &World) -> RoadIndex {
        let roads = &world.roads;
        let mut lines = Vec::new();
        for (s, seg) in roads.segments.iter() {
            let (pa, pb) = roads.segment_ends(s);
            let (a, b) = (Cell::from_pos(pa), Cell::from_pos(pb));
            if a == b || !geom::same_pos(a.center(), pa) || !geom::same_pos(b.center(), pb) {
                continue;
            }
            let Some(_) = a.straight_line_to(b) else { continue };
            let fam = family((b.x - a.x, b.y - a.y));
            let (p0, p1) = (fam_p(fam, a), fam_p(fam, b));
            lines.push(RoadLine {
                seg: s,
                a,
                b,
                kind: seg.kind,
                fam,
                n: fam_n(fam, a),
                p_min: p0.min(p1),
                p_max: p0.max(p1),
            });
        }
        let mut nodes = Vec::new();
        for (id, node) in roads.nodes.iter() {
            let c = Cell::from_pos(node.pos);
            if !geom::same_pos(c.center(), node.pos) {
                continue;
            }
            let mut arms = Vec::new();
            let mut highway = false;
            for &s in &node.segments {
                let Some(seg) = roads.segments.get(s) else { continue };
                highway |= seg.kind == RoadKind::Highway;
                let o = Cell::from_pos(roads.node_pos(seg.other(id)));
                arms.push(((o.x - c.x).signum(), (o.y - c.y).signum()));
            }
            nodes.push(NodeInfo {
                id,
                cell: c,
                arms,
                highway,
            });
        }
        RoadIndex { lines, nodes }
    }

    /// Is there a road (or node) on this cell?
    pub fn on_road(&self, c: Cell) -> bool {
        self.lines.iter().any(|l| l.contains(c))
    }
}

/// A guide line: part of a lattice line, with the cell it is drawn from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideLine {
    pub fam: usize,
    pub n: i32,
    pub p_min: i32,
    pub p_max: i32,
    pub kind: GuideKind,
    /// Where the guide is drawn from (road end, node, reference road).
    pub origin: Cell,
    /// Spacing in cells this guide stands for (spacing guides).
    pub spacing: Option<i32>,
}

impl GuideLine {
    fn contains(&self, c: Cell) -> bool {
        fam_n(self.fam, c) == self.n && (self.p_min..=self.p_max).contains(&fam_p(self.fam, c))
    }

    fn target(&self) -> SnapTarget {
        match self.kind {
            GuideKind::Extension => SnapTarget::Extension,
            GuideKind::Perpendicular => SnapTarget::Perpendicular,
            GuideKind::Parallel => SnapTarget::Parallel,
            GuideKind::Spacing => SnapTarget::Spacing,
            GuideKind::Angle => SnapTarget::Angle,
        }
    }
}

/// Guides near `focus`: road-end extensions, perpendicular alignments
/// through nearby nodes, and the neighbouring block's spacing.
pub fn guides_near(index: &RoadIndex, focus: Cell) -> Vec<GuideLine> {
    let mut out = Vec::new();
    for node in &index.nodes {
        if node.cell.chebyshev(focus) > LOOK_RADIUS {
            continue;
        }
        // Extension of a dead end: straight on, away from its only arm.
        if let [arm] = node.arms[..]
            && !node.highway
        {
            let f = family(arm);
            let p0 = fam_p(f, node.cell);
            let back = (-arm.0, -arm.1);
            let step = fam_p(f, Cell::new(back.0, back.1));
            let p1 = p0 + step * GUIDE_REACH;
            out.push(GuideLine {
                fam: f,
                n: fam_n(f, node.cell),
                p_min: (p0 + step).min(p1),
                p_max: (p0 + step).max(p1),
                kind: GuideKind::Extension,
                origin: node.cell,
                spacing: None,
            });
        }
        // Alignment: through the node, at right angles to each of its arms.
        let mut fams: Vec<usize> = node.arms.iter().map(|&a| perpendicular(family(a))).collect();
        fams.sort_unstable();
        fams.dedup();
        for f in fams {
            // Skip directions the node already has a road along.
            if node.arms.iter().any(|&a| family(a) == f) {
                continue;
            }
            let p0 = fam_p(f, node.cell);
            out.push(GuideLine {
                fam: f,
                n: fam_n(f, node.cell),
                p_min: p0 - GUIDE_REACH,
                p_max: p0 + GUIDE_REACH,
                kind: GuideKind::Perpendicular,
                origin: node.cell,
                spacing: None,
            });
        }
    }
    out.extend(spacing_guides(index, focus));
    out
}

/// Lines at the neighbouring block's spacing from the roads around `focus`:
/// for each direction family, the nearest parallel road on each side and
/// the one beyond it give a spacing `s`; the guide is `s` further on.
pub fn spacing_guides(index: &RoadIndex, focus: Cell) -> Vec<GuideLine> {
    let mut out = Vec::new();
    for f in 0..4 {
        let pf = fam_p(f, focus);
        let nf = fam_n(f, focus);
        // Roads of this family that run past the focus (with some slack).
        let mut ns: Vec<(i32, &RoadLine)> = index
            .lines
            .iter()
            .filter(|l| l.fam == f && l.kind != RoadKind::Highway)
            .filter(|l| (l.p_min - 4..=l.p_max + 4).contains(&pf))
            .filter(|l| (l.n - nf).abs() as f32 * normal_unit(f) <= LOOK_RADIUS as f32)
            .map(|l| (l.n, l))
            .collect();
        ns.sort_by_key(|&(n, _)| n);
        ns.dedup_by_key(|(n, _)| *n);
        for side in [-1i32, 1] {
            // Nearest road on this side of the focus (or through it), and the next one beyond.
            let mut it: Vec<&(i32, &RoadLine)> = ns.iter().filter(|(n, _)| (n - nf) * side > 0).collect();
            it.sort_by_key(|(n, _)| (n - nf).abs());
            let (Some(&&(n1, l1)), Some(&&(n0, _))) = (it.first(), it.get(1)) else {
                continue;
            };
            let s = (n1 - n0).abs();
            if s < 2 {
                continue;
            }
            let stop = n1 - side * s;
            if ns.iter().any(|&(n, _)| n == stop) {
                continue;
            }
            let p_ref = pf.clamp(l1.p_min, l1.p_max);
            out.push(GuideLine {
                fam: f,
                n: stop,
                p_min: l1.p_min - 6,
                p_max: l1.p_max + 6,
                kind: GuideKind::Spacing,
                origin: fam_cell(f, n1, p_ref),
                spacing: Some((s as f32 * normal_unit(f)).round() as i32),
            });
        }
    }
    out
}

/// The result of snapping.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub cell: Cell,
    pub target: SnapTarget,
    /// Every rule that agrees on `cell` (includes `target`).
    pub matched: Vec<SnapTarget>,
    pub guides: Vec<Guide>,
    /// Spacing to the neighbouring road, cells (spacing / parallel snaps).
    pub spacing: Option<i32>,
}

impl Snap {
    fn plain(cell: Cell, target: SnapTarget) -> Snap {
        Snap {
            cell,
            target,
            matched: if target == SnapTarget::None {
                Vec::new()
            } else {
                vec![target]
            },
            guides: Vec::new(),
            spacing: None,
        }
    }
}

/// Priority of a snap kind (lower wins) and how far it reaches, as a
/// fraction of the snap radius.
fn rank(t: SnapTarget) -> (u8, f32) {
    match t {
        SnapTarget::Node => (0, 1.0),
        SnapTarget::Road => (1, 0.85),
        SnapTarget::Extension => (3, 0.7),
        SnapTarget::Perpendicular => (4, 0.6),
        SnapTarget::Spacing => (5, 0.6),
        SnapTarget::Parallel => (6, 0.6),
        SnapTarget::Angle | SnapTarget::Grid | SnapTarget::None => (9, f32::INFINITY),
    }
}

#[derive(Debug, Clone)]
struct Cand {
    cell: Cell,
    kinds: Vec<SnapTarget>,
    /// Guides that made it.
    from: Vec<GuideLine>,
    dist: f32,
}

impl Cand {
    fn prio(&self) -> u8 {
        let best = self.kinds.iter().map(|&k| rank(k).0).min().unwrap_or(9);
        // Two guides crossing beat a single guide.
        if self.kinds.len() >= 2 && best > 1 { 2 } else { best }
    }

    fn reach(&self) -> f32 {
        self.kinds.iter().map(|&k| rank(k).1).fold(0.0, f32::max)
    }
}

fn best(cands: Vec<Cand>, radius: f32) -> Option<Cand> {
    let mut ok: Vec<Cand> = cands
        .into_iter()
        .filter(|c| c.cell.in_bounds() && c.dist <= radius * c.reach())
        .collect();
    // Merge candidates on the same cell.
    ok.sort_by_key(|c| (c.cell.x, c.cell.y));
    let mut merged: Vec<Cand> = Vec::new();
    for c in ok {
        match merged.last_mut() {
            Some(m) if m.cell == c.cell => {
                for k in c.kinds {
                    if !m.kinds.contains(&k) {
                        m.kinds.push(k);
                    }
                }
                m.from.extend(c.from);
            }
            _ => merged.push(c),
        }
    }
    merged.into_iter().min_by(|a, b| {
        a.prio()
            .cmp(&b.prio())
            .then(a.dist.total_cmp(&b.dist))
            .then((a.cell.x, a.cell.y).cmp(&(b.cell.x, b.cell.y)))
    })
}

fn guide_draw(g: &GuideLine, to: Cell) -> Guide {
    Guide {
        from: g.origin.center(),
        to: to.center(),
        kind: g.kind,
    }
}

fn finish(c: Cand) -> Snap {
    let mut kinds = c.kinds.clone();
    kinds.sort_by_key(|&k| rank(k).0);
    kinds.dedup();
    let target = kinds.first().copied().unwrap_or(SnapTarget::Grid);
    let mut guides: Vec<Guide> = c.from.iter().map(|g| guide_draw(g, c.cell)).collect();
    guides.dedup();
    let spacing = c.from.iter().find_map(|g| g.spacing);
    Snap {
        cell: c.cell,
        target,
        matched: kinds,
        guides,
        spacing,
    }
}

/// Snap a free point (no previous corner): road start, block corner.
pub fn snap_point(index: &RoadIndex, cursor: [f32; 2], radius: f32, enabled: bool) -> Snap {
    let cell = Cell::from_pos(cursor);
    if !enabled {
        return Snap::plain(cell, SnapTarget::None);
    }
    let mut cands = Vec::new();
    for n in &index.nodes {
        if n.cell.chebyshev(cell) <= LOOK_RADIUS {
            cands.push(Cand {
                cell: n.cell,
                kinds: vec![SnapTarget::Node],
                from: Vec::new(),
                dist: dist_m(n.cell, cursor),
            });
        }
    }
    for l in &index.lines {
        if l.dist_to(cursor) <= radius {
            let c = l.nearest_cell(cursor);
            cands.push(Cand {
                cell: c,
                kinds: vec![SnapTarget::Road],
                from: Vec::new(),
                dist: dist_m(c, cursor),
            });
        }
    }
    let guides = guides_near(index, cell);
    for (i, g) in guides.iter().enumerate() {
        // Projection onto the guide.
        let c = fam_cell(g.fam, g.n, fam_p(g.fam, cell).clamp(g.p_min, g.p_max));
        let c = nearest_on_guide(g, cursor).unwrap_or(c);
        if g.contains(c) && !index.on_road(c) {
            cands.push(Cand {
                cell: c,
                kinds: vec![g.target()],
                from: vec![*g],
                dist: dist_m(c, cursor),
            });
        }
        for h in &guides[i + 1..] {
            if let Some(c) = cross(g.fam, g.n, h.fam, h.n)
                && g.contains(c)
                && h.contains(c)
                && !index.on_road(c)
            {
                cands.push(Cand {
                    cell: c,
                    kinds: vec![g.target(), h.target()],
                    from: vec![*g, *h],
                    dist: dist_m(c, cursor),
                });
            }
        }
    }
    match best(cands, radius) {
        Some(c) => finish(c),
        None => Snap::plain(cell, SnapTarget::Grid),
    }
}

/// Cell of a guide nearest to `p` (by distance, among the two around the
/// projection).
fn nearest_on_guide(g: &GuideLine, p: [f32; 2]) -> Option<Cell> {
    let probe = Cell::from_pos(p);
    let base = fam_p(g.fam, probe);
    (base - 2..=base + 2)
        .map(|pp| fam_cell(g.fam, g.n, pp.clamp(g.p_min, g.p_max)))
        .min_by(|a, b| dist_m(*a, p).total_cmp(&dist_m(*b, p)))
}

/// Snap the end of a leg from `from` (roads must run straight or at 45
/// degrees, so the end always lies on one of the 8 rays from `from`; with
/// snapping off only that rule applies).
pub fn snap_leg(index: &RoadIndex, from: Cell, cursor: [f32; 2], radius: f32, enabled: bool) -> Snap {
    let start = from.center();
    // On the corner itself: no leg yet (the next click there ends the path).
    if geom::dist(cursor, start) < CELL_SIZE * 0.6 {
        return Snap::plain(from, SnapTarget::None);
    }
    let rel = geom::sub(cursor, start);
    // The plain ray point nearest the cursor.
    let mut plain: Option<(f32, Cell, (i32, i32))> = None;
    for d in DIRS8 {
        let dl = ((d.0 * d.0 + d.1 * d.1) as f32).sqrt() * CELL_SIZE;
        let t = geom::dot(rel, [d.0 as f32 * CELL_SIZE, d.1 as f32 * CELL_SIZE]) / (dl * dl);
        let t = (t.round() as i32).max(1);
        let c = clamp_ray(from, d, t);
        if c == from {
            continue;
        }
        let dd = dist_m(c, cursor);
        if plain.is_none_or(|(b, ..)| dd < b) {
            plain = Some((dd, c, d));
        }
    }
    let Some((_, plain_cell, plain_dir)) = plain else {
        return Snap::plain(from, SnapTarget::None);
    };
    let angle_guide = |to: Cell| Guide {
        from: start,
        to: to.center(),
        kind: GuideKind::Angle,
    };
    if !enabled {
        let mut s = Snap::plain(plain_cell, SnapTarget::None);
        s.guides.push(angle_guide(plain_cell));
        return s;
    }
    let mut cands: Vec<Cand> = Vec::new();
    let guides = guides_near(index, Cell::from_pos(cursor));
    for d in DIRS8 {
        let f = family(d);
        let n_ray = fam_n(f, from);
        let on_ray = |c: Cell| {
            let (dx, dy) = (c.x - from.x, c.y - from.y);
            let t = dx.abs().max(dy.abs());
            t >= 1 && dx == d.0 * t && dy == d.1 * t
        };
        for n in &index.nodes {
            if on_ray(n.cell) {
                cands.push(Cand {
                    cell: n.cell,
                    kinds: vec![SnapTarget::Node],
                    from: Vec::new(),
                    dist: dist_m(n.cell, cursor),
                });
            }
        }
        for l in &index.lines {
            if let Some(c) = cross(f, n_ray, l.fam, l.n)
                && on_ray(c)
                && l.contains(c)
            {
                cands.push(Cand {
                    cell: c,
                    kinds: vec![SnapTarget::Road],
                    from: Vec::new(),
                    dist: dist_m(c, cursor),
                });
            }
        }
        for g in &guides {
            if let Some(c) = cross(f, n_ray, g.fam, g.n)
                && on_ray(c)
                && g.contains(c)
                && !index.on_road(c)
            {
                cands.push(Cand {
                    cell: c,
                    kinds: vec![g.target()],
                    from: vec![*g],
                    dist: dist_m(c, cursor),
                });
            }
        }
    }
    let (mut snap, dir) = match best(cands, radius) {
        Some(c) => {
            let d = ((c.cell.x - from.x).signum(), (c.cell.y - from.y).signum());
            let mut s = finish(c);
            if !s.matched.contains(&SnapTarget::Angle) {
                s.matched.push(SnapTarget::Angle);
            }
            (s, d)
        }
        None => (Snap::plain(plain_cell, SnapTarget::Angle), plain_dir),
    };
    snap.guides.insert(0, angle_guide(snap.cell));
    // Leg-level relations: parallel to a nearby road, perpendicular to the
    // road it starts or ends on.
    let f = family(dir);
    let (p0, p1) = (fam_p(f, from), fam_p(f, snap.cell));
    let (lo, hi) = (p0.min(p1), p0.max(p1));
    let n_leg = fam_n(f, from);
    let parallel = index
        .lines
        .iter()
        .filter(|l| l.fam == f && l.n != n_leg && l.p_max >= lo && l.p_min <= hi)
        .filter(|l| (l.n - n_leg).abs() as f32 * normal_unit(f) <= PARALLEL_REACH as f32)
        .min_by_key(|l| ((l.n - n_leg).abs(), l.p_min));
    if let Some(l) = parallel {
        let off = ((l.n - n_leg).abs() as f32 * normal_unit(f)).round() as i32;
        if !snap.matched.contains(&SnapTarget::Parallel) {
            snap.matched.push(SnapTarget::Parallel);
        }
        let (a, b) = (lo.max(l.p_min), hi.min(l.p_max));
        snap.guides.push(Guide {
            from: fam_cell(f, l.n, a).center(),
            to: fam_cell(f, l.n, b).center(),
            kind: GuideKind::Parallel,
        });
        if snap.spacing.is_none() {
            snap.spacing = Some(off);
        }
        if snap.target == SnapTarget::Angle {
            snap.target = SnapTarget::Parallel;
        }
    }
    let pf = perpendicular(f);
    let perp_at = |c: Cell| index.lines.iter().any(|l| l.fam == pf && l.contains(c));
    if (perp_at(from) || perp_at(snap.cell)) && !snap.matched.contains(&SnapTarget::Perpendicular) {
        snap.matched.push(SnapTarget::Perpendicular);
    }
    snap
}

/// `from + t * d`, pulled back inside the map.
fn clamp_ray(from: Cell, d: (i32, i32), t: i32) -> Cell {
    let mut t = t;
    while t > 0 && !from.offset(d.0 * t, d.1 * t).in_bounds() {
        t -= 1;
    }
    from.offset(d.0 * t, d.1 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::Command;

    fn world_with(roads: &[(Cell, Cell)]) -> World {
        let mut w = World::new(1);
        w.economy.money = 10_000_000;
        for &(a, b) in roads {
            w.apply(Command::BuildRoad {
                from: a,
                to: b,
                kind: citysim_sim::RoadKind::Street,
            })
            .expect("road");
        }
        w
    }

    fn at(c: Cell) -> [f32; 2] {
        c.center()
    }

    fn nudge(c: Cell, dx: f32, dy: f32) -> [f32; 2] {
        let p = c.center();
        [p[0] + dx, p[1] + dy]
    }

    #[test]
    fn lattice_coordinates_round_trip() {
        for f in 0..4 {
            for c in [Cell::new(3, 7), Cell::new(-2, 5), Cell::new(100, 40)] {
                assert_eq!(fam_cell(f, fam_n(f, c), fam_p(f, c)), c, "family {f}");
            }
        }
        assert_eq!(cross(0, 5, 1, 9), Some(Cell::new(9, 5)));
        assert_eq!(cross(2, 0, 3, 10), Some(Cell::new(5, 5)));
        assert_eq!(cross(2, 0, 3, 11), None, "diagonals meeting between centres");
        assert_eq!(cross(0, 1, 0, 2), None);
    }

    #[test]
    fn free_point_snaps_to_nodes_and_roads() {
        let w = world_with(&[(Cell::new(11, 128), Cell::new(30, 128))]);
        let idx = RoadIndex::build(&w);
        // Near the dead end at (30, 128): node.
        let s = snap_point(&idx, nudge(Cell::new(30, 128), 5.0, 3.0), 12.0, true);
        assert_eq!((s.cell, s.target), (Cell::new(30, 128), SnapTarget::Node));
        // Near the middle of the road: a T-junction point on it.
        let s = snap_point(&idx, nudge(Cell::new(20, 129), 1.0, 2.0), 12.0, true);
        assert_eq!((s.cell, s.target), (Cell::new(20, 128), SnapTarget::Road));
        // Alt: the plain cell.
        let s = snap_point(&idx, nudge(Cell::new(20, 129), 1.0, 2.0), 12.0, false);
        assert_eq!((s.cell, s.target), (Cell::new(20, 129), SnapTarget::None));
        // Far from everything: the grid.
        let s = snap_point(&idx, at(Cell::new(200, 30)), 12.0, true);
        assert_eq!(s.target, SnapTarget::Grid);
    }

    #[test]
    fn leg_stays_straight_or_diagonal() {
        let w = World::new(1);
        let idx = RoadIndex::build(&w);
        let from = Cell::new(60, 60);
        for p in [
            [600.0, 410.0],
            [300.0, 900.0],
            [1000.0, 1000.0],
            [10.0, 20.0],
            [490.0, 487.0],
        ] {
            for enabled in [true, false] {
                let s = snap_leg(&idx, from, p, 12.0, enabled);
                assert!(from.straight_line_to(s.cell).is_some(), "{p:?} -> {:?}", s.cell);
                assert_ne!(s.cell, from);
            }
        }
    }

    #[test]
    fn leg_forms_a_t_junction_on_a_road_it_reaches() {
        // A vertical road at x = 40; drawing east from x = 30 towards it.
        let w = world_with(&[
            (Cell::new(11, 128), Cell::new(40, 128)),
            (Cell::new(40, 100), Cell::new(40, 140)),
        ]);
        let idx = RoadIndex::build(&w);
        let s = snap_leg(
            &idx,
            Cell::new(30, 110),
            nudge(Cell::new(41, 111), 0.0, -2.0),
            14.0,
            true,
        );
        assert_eq!(s.cell, Cell::new(40, 110));
        assert_eq!(s.target, SnapTarget::Road);
        assert!(s.matched.contains(&SnapTarget::Perpendicular));
    }

    #[test]
    fn leg_follows_the_extension_of_a_road_end() {
        // A dead end at (30, 128) pointing east; draw a vertical leg at x = 40
        // crossing y = 128: snaps where the extension meets it.
        let w = world_with(&[(Cell::new(11, 128), Cell::new(30, 128))]);
        let idx = RoadIndex::build(&w);
        let s = snap_leg(
            &idx,
            Cell::new(40, 110),
            nudge(Cell::new(40, 127), 3.0, 4.0),
            12.0,
            true,
        );
        assert_eq!(s.cell, Cell::new(40, 128));
        assert!(s.matched.contains(&SnapTarget::Extension), "{:?}", s.matched);
        assert!(s.guides.iter().any(|g| g.kind == GuideKind::Extension));
    }

    #[test]
    fn spacing_matches_the_neighbouring_block() {
        // Horizontal streets at y = 100 and y = 106 (spacing 6), joined by a
        // vertical one; a stub drawn south from y = 106 snaps at y = 112.
        let w = world_with(&[
            (Cell::new(11, 128), Cell::new(20, 128)),
            (Cell::new(20, 128), Cell::new(20, 100)),
            (Cell::new(20, 100), Cell::new(40, 100)),
            (Cell::new(20, 106), Cell::new(40, 106)),
        ]);
        let idx = RoadIndex::build(&w);
        let s = snap_leg(
            &idx,
            Cell::new(30, 106),
            nudge(Cell::new(30, 111), 2.0, 5.0),
            14.0,
            true,
        );
        assert_eq!(s.cell, Cell::new(30, 112), "{s:?}");
        assert!(s.matched.contains(&SnapTarget::Spacing));
        assert_eq!(s.spacing, Some(6));
        // And a free point near y = 112 next to the block too.
        let s = snap_point(&idx, nudge(Cell::new(33, 112), 0.0, 6.0), 14.0, true);
        assert_eq!(fam_n(0, s.cell), 112, "{s:?}");
    }

    #[test]
    fn parallel_leg_reports_its_offset() {
        let w = world_with(&[(Cell::new(11, 128), Cell::new(40, 128))]);
        let idx = RoadIndex::build(&w);
        let s = snap_leg(&idx, Cell::new(15, 123), at(Cell::new(35, 123)), 10.0, true);
        assert_eq!(s.cell.y, 123);
        assert!(s.matched.contains(&SnapTarget::Parallel));
        assert_eq!(s.spacing, Some(5));
        assert!(s.guides.iter().any(|g| g.kind == GuideKind::Parallel));
    }

    #[test]
    fn two_candidates_resolve_deterministically() {
        // Two dead ends equally far from the cursor: the same one every time,
        // and a node beats a guide crossing nearer to the cursor.
        let w = world_with(&[
            (Cell::new(11, 128), Cell::new(20, 128)),
            (Cell::new(20, 128), Cell::new(20, 120)),
            (Cell::new(20, 128), Cell::new(20, 136)),
        ]);
        let idx = RoadIndex::build(&w);
        let mid = at(Cell::new(20, 128));
        let a = snap_point(&idx, [mid[0] + 12.0, mid[1]], 30.0, true);
        let b = snap_point(&idx, [mid[0] + 12.0, mid[1]], 30.0, true);
        assert_eq!(a, b);
        assert_eq!(a.target, SnapTarget::Node);
    }

    #[test]
    fn diagonal_roads_index_and_snap() {
        let w = world_with(&[
            (Cell::new(11, 128), Cell::new(20, 128)),
            (Cell::new(20, 128), Cell::new(30, 118)),
        ]);
        let idx = RoadIndex::build(&w);
        assert!(idx.lines.iter().any(|l| l.fam == 3));
        let s = snap_point(&idx, nudge(Cell::new(25, 123), 2.0, 1.0), 10.0, true);
        assert_eq!(s.target, SnapTarget::Road);
        assert_eq!(s.cell, Cell::new(25, 123));
    }
}
