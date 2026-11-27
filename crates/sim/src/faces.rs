//! Land enclosed by straight roads laid out as one grid.
//!
//! The faces of the road graph are the pieces of land the roads enclose. A
//! face whose sides are all straight roads running in two directions (a
//! square, a rectangle, a trapezoid, a block a few degrees off true, an L)
//! gets one grid fitted between its kerbs, whatever the spacing of its
//! roads: column lines run from kerb to kerb across it, each stretch between
//! two kerbs of the same direction divided into whole cells (stretched or
//! squeezed by under half a cell), and so do the rows. The rows of facing
//! and meeting streets are then the same rows, and corner cells front both
//! roads as on the cell grid.
//!
//! The grid is stored as strips ([`Strip`]): one per band of rows between
//! two kerbs that run along the face's first direction (one for a
//! four-sided block, two for an L), each column boundary a straight line
//! across the band. Strips travel in the save file already, so nothing new
//! is stored: a face's grid is a pure function of its roads.
//!
//! Faces already on whole cells and right angles (the grid town) keep the
//! shared straight lattices of `blocks`; so do faces with curved sides, and
//! faces whose sides do not fall into two directions (a triangle).

use crate::curve::{line_meet, right_of, unit};
use crate::geom;
use crate::roads::{NodeId, RoadNetwork, SegmentId};
use crate::zoning::{Side, Strip};
use crate::{CELL_SIZE, Facing, balance};

/// A segment travelled forwards (`a` to `b`, `true`) or backwards; its face
/// is the land on the right of travel.
pub(crate) type Half = (SegmentId, bool);

/// A block's cell rectangle and the side facing the road: `(min, max,
/// front)`.
pub(crate) type Rect = ([i32; 2], [i32; 2], Facing);

/// A line: a point on it and its unit direction (or two points on it).
type Line = ([f32; 2], [f32; 2]);

/// The half-edge whose face is the land on `side` of `s`.
pub(crate) fn half(s: SegmentId, side: Side) -> Half {
    (s, side == Side::Right)
}

/// The side of its segment a half-edge's face lies on.
pub(crate) fn side_of(h: Half) -> Side {
    if h.1 { Side::Right } else { Side::Left }
}

/// Faces with more pieces than this are not walked to the end.
const MAX_EDGES: usize = 64;
/// Pieces meeting at a node that turn less than this are one side.
const SIDE_BEND_DEG: f32 = 8.0;
/// How far a side's inner nodes may lie off the line between its corners
/// (the grid's row along it follows that line), metres.
const SIDE_DEV_M: f32 = 1.5;
/// How far a side may turn from the face's two directions, degrees.
const FAMILY_TOL_DEG: f32 = 30.0;
/// Faces wider than this keep per-road lattices, metres.
const MAX_SPAN_M: f32 = 720.0;
/// Narrowest and widest a cell edge may become.
const MIN_W: f32 = 4.0;
const MAX_W: f32 = 13.0;

fn ends(roads: &RoadNetwork, h: Half) -> Option<(NodeId, NodeId)> {
    let s = roads.segments.get(h.0)?;
    Some(if h.1 { (s.a, s.b) } else { (s.b, s.a) })
}

/// The face on the right of `start`: its half-edges in order (each one's
/// end node is the next one's start), or `None` when it is not closed
/// within `MAX_EDGES` pieces.
pub(crate) fn walk(roads: &RoadNetwork, start: Half) -> Option<Vec<Half>> {
    let mut out = vec![start];
    let mut cur = start;
    loop {
        let seg = roads.segments.get(cur.0)?;
        if seg.a == seg.b {
            return None;
        }
        let (_, n) = ends(roads, cur)?;
        let node = roads.nodes.get(n)?;
        // Turn as far right as possible: the first road clockwise (in
        // `atan2` terms) from the one arrived on.
        let back = seg.dir_from(n);
        let mut best: Option<(f32, Half)> = None;
        for &t in &node.segments {
            if t == cur.0 {
                continue;
            }
            let ts = roads.segments.get(t)?;
            let d = ts.dir_from(n);
            let ccw = libm::atan2f(geom::cross(back, d), geom::dot(back, d));
            let mut cw = -ccw;
            if cw <= 0.0 {
                cw += std::f32::consts::TAU;
            }
            let h = (t, ts.a == n);
            if best.is_none_or(|b| cw < b.0 || (cw == b.0 && h < b.1)) {
                best = Some((cw, h));
            }
        }
        // A dead end: back along the same road.
        let next = best.map_or((cur.0, !cur.1), |b| b.1);
        if next == start {
            return Some(out);
        }
        if out.len() >= MAX_EDGES {
            return None;
        }
        out.push(next);
        cur = next;
    }
}

/// The half-edge a face is known by (its smallest).
pub(crate) fn key(face: &[Half]) -> Half {
    face.iter().copied().min().unwrap_or_default()
}

/// One side of a face: consecutive straight pieces between two corners.
struct FaceSide {
    halves: Vec<Half>,
    /// Corner at the start, inner nodes, corner at the end.
    nodes: Vec<[f32; 2]>,
    dir: [f32; 2],
    /// A point on the kerb line and its direction.
    kerb: Line,
    /// 0: runs along the face's `u` (its kerb is a band boundary); 1:
    /// across.
    family: usize,
    /// Index of its kerb among its family's kerbs.
    anchor: usize,
}

/// A face's grid and the blocks of its roads on it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    /// The bands of rows, in order along the face's `v`.
    pub bands: Vec<Strip>,
    /// Rows of each band.
    pub rows: Vec<i32>,
    /// Blocks per segment side: segment, side, band, rectangle in the
    /// band's strip.
    pub blocks: Vec<(SegmentId, Side, usize, Rect)>,
}

fn point(p: [f32; 2], d: [f32; 2], t: f32) -> [f32; 2] {
    [p[0] + d[0] * t, p[1] + d[1] * t]
}

fn meet(a: Line, b: Line) -> Option<[f32; 2]> {
    let (t, _) = line_meet(a.0, a.1, b.0, b.1)?;
    Some(point(a.0, a.1, t))
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Distance from `p` to the line through `l.0` along unit `l.1`.
fn line_dist(l: Line, p: [f32; 2]) -> f32 {
    geom::cross(l.1, geom::sub(p, l.0)).abs()
}

/// Even-odd point in polygon.
fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for k in 0..n {
        let (a, b) = (poly[k], poly[(k + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                c = !c;
            }
        }
    }
    c
}

/// Lines of one direction between kerbs: the kerbs in order and, between
/// two neighbours, whole cells. Returns the lines (two points each, on the
/// rails) and each kerb's line index; `None` when lines would cross or the
/// cells come out too narrow or wide.
#[allow(clippy::type_complexity)]
fn lines_between(kerbs: &[Line], rail0: Line, rail1: Line) -> Option<(Vec<Line>, Vec<usize>)> {
    let pts: Vec<Line> = kerbs
        .iter()
        .map(|&k| Some((meet(k, rail0)?, meet(k, rail1)?)))
        .collect::<Option<_>>()?;
    let mut lines = Vec::new();
    let mut index = Vec::new();
    for k in 0..pts.len() {
        index.push(lines.len());
        lines.push(pts[k]);
        if k + 1 == pts.len() {
            break;
        }
        let (a, b) = (pts[k], pts[k + 1]);
        let mid = lerp(a.0, a.1, 0.5);
        let gap = line_dist(kerbs[k + 1], mid);
        let n = (libm::roundf(gap / CELL_SIZE) as i32).max(1);
        for s in 1..n {
            let t = s as f32 / n as f32;
            lines.push((lerp(a.0, b.0, t), lerp(a.1, b.1, t)));
        }
    }
    Some((lines, index))
}

/// The grid of a face and its roads' blocks, or `None` when the face keeps
/// per-road lattices (see the module docs). `depth`: rows a block may
/// reach from its road.
pub(crate) fn plan(roads: &RoadNetwork, face: &[Half], depth: i32) -> Option<Plan> {
    if face.len() < 4 {
        return None;
    }
    // Straight pieces only, each once (no dead ends into the face).
    let mut segs: Vec<SegmentId> = face.iter().map(|h| h.0).collect();
    segs.sort();
    if segs.windows(2).any(|w| w[0] == w[1]) {
        return None;
    }
    let mut pts = Vec::with_capacity(face.len());
    for &h in face {
        let s = roads.segments.get(h.0)?;
        if !s.curve.is_straight() {
            return None;
        }
        let (a, _) = ends(roads, h)?;
        pts.push(roads.nodes.get(a)?.pos);
    }
    let n = pts.len();
    let area: f32 = (0..n).map(|k| geom::cross(pts[k], pts[(k + 1) % n])).sum::<f32>() * 0.5;
    if area <= 0.0 {
        return None;
    }
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    if hi[0] - lo[0] > MAX_SPAN_M || hi[1] - lo[1] > MAX_SPAN_M {
        return None;
    }
    // Sides: pieces between corners.
    let dir = |k: usize| unit(geom::sub(pts[(k + 1) % n], pts[k]));
    let bend = SIDE_BEND_DEG.to_radians();
    let corner = |k: usize| crate::curve::angle_between(dir((k + n - 1) % n), dir(k)).abs() >= bend;
    let c0 = (0..n).find(|&k| corner(k))?;
    let mut sides: Vec<FaceSide> = Vec::new();
    let mut k = c0;
    loop {
        let mut halves = vec![face[k]];
        let mut nodes = vec![pts[k]];
        let mut e = (k + 1) % n;
        while e != c0 && !corner(e) {
            halves.push(face[e]);
            nodes.push(pts[e]);
            e = (e + 1) % n;
        }
        nodes.push(pts[e]);
        let (p, q) = (nodes[0], *nodes.last().expect("two nodes"));
        let d = unit(geom::sub(q, p));
        if nodes[1..nodes.len() - 1]
            .iter()
            .any(|&m| line_dist((p, d), m) > SIDE_DEV_M)
        {
            return None;
        }
        let reserve = halves
            .iter()
            .filter_map(|h| roads.segments.get(h.0))
            .map(|s| s.kind.reserve_half_width_m())
            .fold(0.0, f32::max);
        sides.push(FaceSide {
            halves,
            nodes,
            dir: d,
            kerb: (point(p, right_of(d), reserve), d),
            family: 0,
            anchor: 0,
        });
        k = e;
        if k == c0 {
            break;
        }
    }
    let m = sides.len();
    if m < 4 || m % 2 == 1 {
        return None;
    }
    // The face's directions: along its longest side, and to its right
    // (into the face from that side).
    let len = |s: &FaceSide| geom::dist(s.nodes[0], *s.nodes.last().expect("node"));
    let longest = (0..m).max_by(|&a, &b| len(&sides[a]).total_cmp(&len(&sides[b])).then(b.cmp(&a)))?;
    let u = sides[longest].dir;
    let v = right_of(u);
    let quarter = std::f32::consts::FRAC_PI_2;
    let tol = FAMILY_TOL_DEG.to_radians();
    let mut square = true;
    for s in &mut sides {
        let a = crate::curve::angle_between(u, s.dir);
        let q = libm::roundf(a / quarter);
        let r = a - q * quarter;
        if r.abs() > tol {
            return None;
        }
        square &= r.abs() <= balance::LATTICE_ANGLE_TOL_DEG.to_radians();
        s.family = (q as i32).rem_euclid(2) as usize;
    }
    if (0..m).any(|k| sides[k].family == sides[(k + 1) % m].family) {
        return None;
    }
    // Kerbs of each family in order along the other axis; collinear ones
    // are one kerb.
    let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let mut kerbs: [Vec<(f32, Line)>; 2] = [Vec::new(), Vec::new()];
    for (f, list_f) in kerbs.iter_mut().enumerate() {
        let axis = if f == 0 { v } else { u };
        let mut list: Vec<(f32, usize)> = sides
            .iter()
            .enumerate()
            .filter(|(_, s)| s.family == f)
            .map(|(k, s)| {
                let (p, d) = s.kerb;
                let q = point(p, d, geom::dot(geom::sub(centre, p), d));
                (geom::dot(geom::sub(q, centre), axis), k)
            })
            .collect();
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (pos, k) in list {
            match list_f.last() {
                Some(&(last, _)) if pos - last <= 0.25 => {}
                Some(&(last, _)) if pos - last < CELL_SIZE * 0.5 => return None,
                _ => list_f.push((pos, sides[k].kerb)),
            }
            sides[k].anchor = list_f.len() - 1;
        }
        if list_f.len() < 2 {
            return None;
        }
    }
    let ys: Vec<Line> = kerbs[0].iter().map(|k| k.1).collect();
    let xs: Vec<Line> = kerbs[1].iter().map(|k| k.1).collect();
    // On whole cells at right angles already: the shared lattices do it.
    let whole = |list: &[(f32, Line)]| {
        list.windows(2).all(|w| {
            let g = (w[1].0 - w[0].0) / CELL_SIZE;
            (g - libm::roundf(g)).abs() <= balance::LATTICE_OFFSET_TOL
        })
    };
    if square && whole(&kerbs[0]) && whole(&kerbs[1]) {
        return None;
    }
    // Column lines across the whole face, between its first and last
    // row kerbs.
    let (cols, col_at) = lines_between(&xs, ys[0], ys[ys.len() - 1])?;
    // Bands of rows between neighbouring row kerbs.
    let mut bands = Vec::new();
    let mut rows = Vec::new();
    for b in 0..ys.len() - 1 {
        let (top, bottom) = (ys[b], ys[b + 1]);
        let mut frames = Vec::with_capacity(cols.len());
        let mid_gap = {
            let (p, q) = cols[cols.len() / 2];
            let d = unit(geom::sub(q, p));
            let a = meet((p, d), top)?;
            let c = meet((p, d), bottom)?;
            geom::dist(a, c)
        };
        let nr = (libm::roundf(mid_gap / CELL_SIZE) as i32).max(1);
        for &(p, q) in &cols {
            let d = unit(geom::sub(q, p));
            let a = meet((p, d), top)?;
            let c = meet((p, d), bottom)?;
            let step = 1.0 / (nr as f32 * CELL_SIZE);
            frames.push((a, [(c[0] - a[0]) * step, (c[1] - a[1]) * step]));
        }
        let st = Strip::spanning(0, frames, nr as f32);
        // Every cell a proper quadrilateral of sensible size.
        for i in 0..st.columns() {
            for j in 0..nr {
                let q = [
                    st.at(i as f32, j as f32),
                    st.at((i + 1) as f32, j as f32),
                    st.at((i + 1) as f32, (j + 1) as f32),
                    st.at(i as f32, (j + 1) as f32),
                ];
                for e in 0..4 {
                    let (a, b, c) = (q[e], q[(e + 1) % 4], q[(e + 2) % 4]);
                    let w = geom::dist(a, b);
                    if !(MIN_W..=MAX_W).contains(&w) || geom::cross(geom::sub(b, a), geom::sub(c, b)) <= 0.0 {
                        return None;
                    }
                }
            }
        }
        bands.push(st);
        rows.push(nr);
    }
    // The land inside the kerbs.
    let poly: Vec<[f32; 2]> = (0..m)
        .map(|k| meet(sides[(k + m - 1) % m].kerb, sides[k].kerb))
        .collect::<Option<_>>()?;
    let ncols = cols.len() as i32 - 1;
    // Global row index of each band's first row.
    let first_row: Vec<i32> = rows
        .iter()
        .scan(0, |acc, &r| {
            let s = *acc;
            *acc += r;
            Some(s)
        })
        .collect();
    let total_rows: i32 = rows.iter().sum();
    let band_of_row = |g: i32| -> (usize, i32) {
        let b = first_row.partition_point(|&f| f <= g).saturating_sub(1);
        (b, g - first_row[b])
    };
    let centre_in = |b: usize, i: i32, j: i32| inside(&poly, bands[b].at(i as f32 + 0.5, j as f32 + 0.5));
    // Position of a node along a side (column line or global row index).
    let along = |s: &FaceSide, p: [f32; 2]| -> f32 {
        let k = point(p, right_of(s.dir), geom::dist(s.kerb.0, s.nodes[0]));
        if s.family == 0 {
            let b = s.anchor.min(bands.len() - 1);
            bands[b].local(k).map_or(0.0, |l| l[0])
        } else {
            // Which band holds it, then the row within.
            for (b, st) in bands.iter().enumerate() {
                if let Some(l) = st.local(k)
                    && (-0.5..=rows[b] as f32 + 0.5).contains(&l[1])
                {
                    return first_row[b] as f32 + l[1];
                }
            }
            0.0
        }
    };
    let mut blocks: Vec<(SegmentId, Side, usize, Rect)> = Vec::new();
    for (k, s) in sides.iter().enumerate() {
        let prev = &sides[(k + m - 1) % m];
        let next = &sides[(k + 1) % m];
        let inward = right_of(s.dir);
        // The side's span along its kerb, as line indices: its corners sit
        // on the kerbs of the sides before and after it.
        let at_corner = |o: &FaceSide| -> i32 {
            if o.family == 1 {
                col_at[o.anchor] as i32
            } else {
                first_row.get(o.anchor).copied().unwrap_or(total_rows)
            }
        };
        let (c_start, c_end) = (at_corner(prev), at_corner(next));
        let (lo_c, hi_c) = (c_start.min(c_end), c_start.max(c_end));
        // Each piece's span: its nodes, rounded to lines, in order.
        let mut marks: Vec<i32> = vec![c_start];
        for &p in &s.nodes[1..s.nodes.len() - 1] {
            marks.push((libm::roundf(along(s, p)) as i32).clamp(lo_c, hi_c));
        }
        marks.push(c_end);
        for (h, w) in s.halves.iter().zip(marks.windows(2)) {
            let (a, b) = (w[0].min(w[1]), w[0].max(w[1]));
            if a == b {
                continue;
            }
            let side = side_of(*h);
            if s.family == 0 {
                // Rows of one band, towards the inside.
                let down = geom::dot(inward, v) > 0.0;
                let (band, front) = if down {
                    (s.anchor, Facing::MinusY)
                } else {
                    (s.anchor.wrapping_sub(1), Facing::PlusY)
                };
                if band >= bands.len() {
                    continue;
                }
                let nr = rows[band];
                let row = |r: i32| if down { r } else { nr - 1 - r };
                let deep = |i: i32| (0..depth.min(nr)).take_while(|&r| centre_in(band, i, row(r))).count() as i32;
                let mut i = a;
                while i < b {
                    let d = deep(i);
                    let mut e = i;
                    while e + 1 < b && deep(e + 1) == d {
                        e += 1;
                    }
                    if d > 0 {
                        let (j0, j1) = if down { (0, d - 1) } else { (nr - d, nr - 1) };
                        blocks.push((h.0, side, band, ([i, j0], [e, j1], front)));
                    }
                    i = e + 1;
                }
            } else {
                // Columns from the kerb, in every band the piece passes.
                let right = geom::dot(inward, u) > 0.0;
                let front = if right { Facing::MinusX } else { Facing::PlusX };
                let line = col_at[s.anchor] as i32;
                let col = |r: i32| if right { line + r } else { line - 1 - r };
                let deep = |band: usize, j: i32| {
                    (0..depth)
                        .take_while(|&r| (0..ncols).contains(&col(r)) && centre_in(band, col(r), j))
                        .count() as i32
                };
                let mut g = a;
                while g < b {
                    let (band, j) = band_of_row(g);
                    let d = deep(band, j);
                    let mut e = g;
                    while e + 1 < b && {
                        let (b2, j2) = band_of_row(e + 1);
                        b2 == band && deep(b2, j2) == d
                    } {
                        e += 1;
                    }
                    let j1 = j + (e - g);
                    if d > 0 {
                        let (i0, i1) = if right {
                            (line, line + d - 1)
                        } else {
                            (line - d, line - 1)
                        };
                        blocks.push((h.0, side, band, ([i0, j], [i1, j1], front)));
                    }
                    g = e + 1;
                }
            }
        }
    }
    Some(Plan { bands, rows, blocks })
}

// ------------------------------------------------------------- leftovers ----

/// Faces with more land than this (square metres) are open country, not
/// leftovers.
const MAX_LEFTOVER_FACE_M2: f32 = 40_000.0;
/// Smallest leftover worth calling one (square metres).
const MIN_LEFTOVER_M2: f32 = 96.0;
/// Sample spacing of leftover land, metres.
pub const LEFTOVER_STEP_M: f32 = 2.0;

/// Land enclosed by roads that no zoning block reaches: the middle of a
/// small ring, the middle of a block wider than two blocks' depth, the
/// corner of a triangle the rows leave out. Nothing can be built there (no
/// lot has a road in front); renderers draw it as a small green. Found by
/// sampling: `inside[y * size[0] + x]` for the point `origin + (x, y) *
/// step`. Derived from the roads and blocks only (not stored).
#[derive(Debug, Clone, PartialEq)]
pub struct Leftover {
    /// The face it lies in, by its smallest half-edge.
    pub face: (SegmentId, bool),
    /// Changes whenever anything it is made from changes.
    pub signature: u64,
    pub origin: [f32; 2],
    pub step: f32,
    pub size: [u32; 2],
    pub inside: Vec<bool>,
    pub area_m2: f32,
}

impl Leftover {
    /// Is the point in it (the sample nearest to it)?
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let x = libm::roundf((p[0] - self.origin[0]) / self.step);
        let y = libm::roundf((p[1] - self.origin[1]) / self.step);
        if x < 0.0 || y < 0.0 || x >= self.size[0] as f32 || y >= self.size[1] as f32 {
            return false;
        }
        self.inside[y as usize * self.size[0] as usize + x as usize]
    }
}

/// Every bounded face of the road graph small enough to hold leftovers:
/// `(key, signature)`. The signature changes with the face's roads, their
/// blocks and lattices.
pub fn leftover_faces(world: &crate::World) -> Vec<((SegmentId, bool), u64)> {
    use std::hash::{Hash, Hasher};
    let roads = &world.roads;
    let mut seen: std::collections::BTreeSet<Half> = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    // Blocks by segment (they are sorted by segment).
    let blocks = &world.zoning.blocks;
    let of = |s: SegmentId| {
        let a = blocks.partition_point(|b| b.segment < s);
        let e = blocks.partition_point(|b| b.segment <= s);
        &blocks[a..e]
    };
    for (s, _) in roads.segments.iter() {
        for h in [(s, true), (s, false)] {
            if seen.contains(&h) {
                continue;
            }
            let Some(face) = walk(roads, h) else {
                seen.insert(h);
                continue;
            };
            seen.extend(face.iter().copied());
            let Some(poly) = face_polygon(roads, &face) else {
                continue;
            };
            let area: f32 = poly_area(&poly);
            if area <= 0.0 || area > MAX_LEFTOVER_FACE_M2 {
                continue;
            }
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            for &x in &face {
                x.hash(&mut hs);
                if let Some(seg) = roads.segments.get(x.0) {
                    seg.serial.hash(&mut hs);
                    for p in seg.curve.p {
                        p[0].to_bits().hash(&mut hs);
                        p[1].to_bits().hash(&mut hs);
                    }
                }
                for b in of(x.0).iter().filter(|b| b.side == side_of(x)) {
                    (b.lattice, b.min, b.max).hash(&mut hs);
                    if let Some(l) = world.zoning.lattice(b.lattice) {
                        l.origin[0].to_bits().hash(&mut hs);
                        l.origin[1].to_bits().hash(&mut hs);
                        l.u[0].to_bits().hash(&mut hs);
                        l.u[1].to_bits().hash(&mut hs);
                        l.strip.as_ref().map(|st| st.frames.len()).hash(&mut hs);
                    }
                }
            }
            out.push((key(&face), hs.finish()));
        }
    }
    out
}

/// The centre line around a face, in order (curves flattened).
fn face_polygon(roads: &RoadNetwork, face: &[Half]) -> Option<Vec<[f32; 2]>> {
    let mut poly: Vec<[f32; 2]> = Vec::new();
    for &h in face {
        let seg = roads.segments.get(h.0)?;
        let mut pts = seg.curve.flatten(0.25);
        if !h.1 {
            pts.reverse();
        }
        pts.pop();
        poly.extend(pts);
    }
    (poly.len() >= 3).then_some(poly)
}

fn poly_area(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    (0..n).map(|k| geom::cross(p[k], p[(k + 1) % n])).sum::<f32>() * 0.5
}

/// The leftover land of one face (see [`Leftover`]), if it has any worth
/// a green.
pub fn leftover(world: &crate::World, face_key: (SegmentId, bool), signature: u64) -> Option<Leftover> {
    let roads = &world.roads;
    let face = walk(roads, face_key)?;
    let poly = face_polygon(roads, &face)?;
    let (mut lo, mut hi) = (poly[0], poly[0]);
    for p in &poly {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    // Blocks of the face's own roads (on its side).
    let mut blocks: Vec<(&crate::zoning::Lattice, [f32; 4])> = Vec::new();
    let mut reserve = 0.0f32;
    for &h in &face {
        if let Some(seg) = roads.segments.get(h.0) {
            reserve = reserve.max(seg.kind.reserve_half_width_m());
        }
        for b in world.zoning.blocks_of(h.0).filter(|b| b.side == side_of(h)) {
            if let Some(l) = world.zoning.lattice(b.lattice) {
                blocks.push((
                    l,
                    [
                        b.min[0] as f32,
                        b.min[1] as f32,
                        (b.max[0] + 1) as f32,
                        (b.max[1] + 1) as f32,
                    ],
                ));
            }
        }
    }
    let step = LEFTOVER_STEP_M;
    let origin = [libm::floorf(lo[0] / step) * step, libm::floorf(lo[1] / step) * step];
    let size = [
        (libm::ceilf((hi[0] - origin[0]) / step) as u32 + 1).min(512),
        (libm::ceilf((hi[1] - origin[1]) / step) as u32 + 1).min(512),
    ];
    let clear = reserve + 1.0;
    let mut inside = vec![false; (size[0] * size[1]) as usize];
    let mut count = 0usize;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let p = [origin[0] + x as f32 * step, origin[1] + y as f32 * step];
            if !inside_poly(&poly, p) {
                continue;
            }
            let near_road = poly
                .iter()
                .zip(poly.iter().cycle().skip(1))
                .any(|(&a, &b)| crate::build::point_seg_dist(p, a, b) < clear);
            if near_road {
                continue;
            }
            let covered = blocks.iter().any(|(l, r)| {
                let q = l.local(p);
                q[0] >= r[0] - 0.02 && q[0] <= r[2] + 0.02 && q[1] >= r[1] - 0.02 && q[1] <= r[3] + 0.02
            });
            if !covered {
                inside[(y * size[0] + x) as usize] = true;
                count += 1;
            }
        }
    }
    let area_m2 = count as f32 * step * step;
    (area_m2 >= MIN_LEFTOVER_M2).then_some(Leftover {
        face: face_key,
        signature,
        origin,
        step,
        size,
        inside,
        area_m2,
    })
}

/// Even-odd point in polygon (public form for the leftovers).
fn inside_poly(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    inside(poly, p)
}

/// Every leftover in the world (see [`Leftover`]).
pub fn leftovers(world: &crate::World) -> Vec<Leftover> {
    leftover_faces(world)
        .into_iter()
        .filter_map(|(k, sig)| leftover(world, k, sig))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Curve, RoadKind, World};

    fn road(w: &mut World, a: [f32; 2], b: [f32; 2]) {
        w.apply(Command::BuildCurve {
            curve: Curve::straight(a, b),
            kind: RoadKind::Street,
        })
        .unwrap();
    }

    /// A closed loop of straight streets through the points.
    fn loop_of(w: &mut World, pts: &[[f32; 2]]) {
        for k in 0..pts.len() {
            road(w, pts[k], pts[(k + 1) % pts.len()]);
        }
    }

    fn face_plans(w: &World) -> Vec<Plan> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (s, _) in w.roads.segments.iter() {
            for side in [Side::Left, Side::Right] {
                let Some(f) = walk(&w.roads, half(s, side)) else {
                    continue;
                };
                if seen.insert(key(&f))
                    && let Some(p) = plan(&w.roads, &f, 6)
                {
                    out.push(p);
                }
            }
        }
        out
    }

    #[test]
    fn a_hand_drawn_square_gets_one_grid() {
        let mut w = World::new(1);
        loop_of(
            &mut w,
            &[[403.0, 401.0], [470.3, 402.5], [469.0, 475.7], [401.5, 474.0]],
        );
        let plans = face_plans(&w);
        assert_eq!(plans.len(), 1, "one face planned");
        let p = &plans[0];
        assert_eq!(p.bands.len(), 1);
        // Four sides, each with blocks.
        let sides: std::collections::BTreeSet<_> = p.blocks.iter().map(|b| (b.0, b.1)).collect();
        assert_eq!(sides.len(), 4, "{:?}", p.blocks);
    }

    #[test]
    fn a_grid_block_on_whole_cells_keeps_the_shared_lattice() {
        let mut w = World::new(1);
        loop_of(
            &mut w,
            &[[404.0, 404.0], [484.0, 404.0], [484.0, 484.0], [404.0, 484.0]],
        );
        assert!(face_plans(&w).is_empty());
    }

    #[test]
    fn an_l_shaped_block_is_two_bands_of_one_grid() {
        let mut w = World::new(1);
        loop_of(
            &mut w,
            &[
                [400.0, 400.0],
                [503.0, 400.0],
                [503.0, 451.0],
                [452.0, 451.0],
                [452.0, 505.0],
                [400.0, 505.0],
            ],
        );
        let plans = face_plans(&w);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].bands.len(), 2);
    }

    #[test]
    fn a_triangle_is_not_planned() {
        let mut w = World::new(1);
        loop_of(&mut w, &[[400.0, 400.0], [500.0, 400.0], [400.0, 500.0]]);
        assert!(face_plans(&w).is_empty());
    }

    #[test]
    fn a_small_ring_leaves_a_green_in_the_middle_and_a_full_block_none() {
        let mut w = World::new(1);
        let c = [600.0, 600.0];
        let r = 40.0f32;
        let p = |deg: f32| {
            let a = deg.to_radians();
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        };
        let t = |deg: f32| {
            let a = deg.to_radians();
            [-a.sin(), a.cos()]
        };
        for k in 0..4 {
            let a = k as f32 * 90.0;
            let cv = Curve::from_tangents(p(a), t(a), p(a + 90.0), t(a + 90.0)).unwrap();
            w.apply(Command::BuildCurve {
                curve: cv,
                kind: RoadKind::Street,
            })
            .unwrap();
        }
        let l = leftovers(&w);
        assert_eq!(l.len(), 1, "one green");
        assert!(l[0].contains(c), "at the centre");
        assert!(l[0].area_m2 > 100.0 && l[0].area_m2 < std::f32::consts::PI * r * r);
        // A hand-drawn block 80 m across: the rows meet, nothing is left.
        let mut w = World::new(1);
        loop_of(
            &mut w,
            &[[403.0, 401.0], [470.3, 402.5], [469.0, 475.7], [401.5, 474.0]],
        );
        assert!(leftovers(&w).is_empty());
    }
}
