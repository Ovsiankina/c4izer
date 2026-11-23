//! Bridges where two drawn lines cross (SRS 4.8 P4-500..P4-523): the
//! view setting `crossings gap|jump|plain` marks each proper crossing of
//! two routed lines. One line, the *carrier*, is cut around the crossing
//! with a T bar on each cut end (`gap`) or makes a small rectangular hop
//! (`jump`); the other passes whole. Drawing only: routes, crossing counts
//! and hit-testing (`SceneLine::points`) are unchanged.
//!
//! `add` runs at the end of `scene::build_with`, on the scene's lines in
//! px; it fills `SceneLine::runs` and `SceneLine::bars` and returns one
//! `SceneBridge` per crossing. `flipped` is the editor's "Bridge on the
//! other line".
//!
//! Depends on: `geom`, `model` (`CrossingStyle`), `route`
//! (`RoutedBundle`), `scene`, `view`.

// Geometry reads best with the textbook names (a, b, p, q, t ...).
#![allow(clippy::many_single_char_names)]

// DECISION: decisions in f64 px, output on whole px: a crossing of two
// grid lines may fall on a half px (two diagonals), so `t` along the
// carrier is real; every drawn point is rounded once, per axis step on a
// diagonal so it stays exactly on the carrier's own line.

use std::collections::BTreeMap;
use std::f64::consts::SQRT_2;

use crate::geom::{Contact, Point, Rect, Segment};
use crate::model::CrossingStyle;
use crate::route::RoutedBundle;
use crate::scene::{Scene, SceneBox, SceneBridge, SceneLabel, SceneLine, Tip};
use crate::view::{EdgeId, ViewPlan};

/// Half the gap (and hop) along the carrier at a square (90°) crossing,
/// px: a 16px gap. Serves P4-506.
// APPROVER: new pub item
pub const GAP_HALF_SQUARE_PX: i32 = 8;

/// Half the gap (and hop) along the carrier at a slanted (45°) crossing,
/// px: a 28px gap, so the bars stay as clear of the slanted passing line
/// as they are at 90°. Serves P4-506.
// APPROVER: new pub item
pub const GAP_HALF_SLANT_PX: i32 = 14;

/// Half the length of a T bar, px: a 10px bar centred on the cut end,
/// square to the carrier. Serves P4-506.
// APPROVER: new pub item
pub const BAR_HALF_PX: i32 = 5;

/// How far a hop stands out from the carrier, px. Serves P4-508.
// APPROVER: new pub item
pub const HOP_PX: i32 = 8;

/// Least distance, px, from a gap or hop to a corner of its carrier (either
/// end of the carrier's segment). Serves P4-516.
// APPROVER: new pub item
pub const CORNER_CLEAR_PX: i32 = 12;

/// Least distance, px, from a bar, hop or gap to a box, label, callout
/// mark, chevron, arrowhead, join dot or any line it does not pass,
/// measured centreline to edge. Serves P4-516.
// APPROVER: new pub item
pub const CLEAR_PX: i32 = 3;

/// The farthest a gap or hop is moved along its carrier off the centre of
/// its crossings, px, in 1px steps. Serves P4-516.
// APPROVER: new pub item
pub const MAX_SHIFT_PX: i32 = 4;

/// The least line left between two gaps or hops on one carrier, px; closer
/// crossings merge or are drawn plain. Serves P4-515.
// APPROVER: new pub item
pub const MIN_PIECE_PX: i32 = 20;

/// Two bars of one bus closer than this, px, centre to centre, on one
/// line square to their tracks, are fused (the track pitch is 20px).
const FUSE_REACH_PX: f64 = 32.0;

type V = (f64, f64);

/// Which lines decide together: a whole bus, or one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Bus(usize),
    Line(EdgeId),
}

/// One proper crossing of two routed lines.
#[derive(Debug, Clone)]
struct Site {
    /// Index into the scene's lines of the carrier and the passing line.
    carrier: usize,
    passing: usize,
    /// Segment of the carrier, of the passing line.
    seg: usize,
    pseg: usize,
    /// Distance from the carrier segment's start, px.
    t: f64,
    at: V,
    /// Half the gap at this crossing (P4-506).
    half: f64,
    by_default: bool,
    drawn: bool,
}

/// One gap or hop: crossings merged on one carrier segment.
#[derive(Debug, Clone)]
struct Cluster {
    carrier: usize,
    seg: usize,
    lo: f64,
    hi: f64,
    sites: Vec<usize>,
    /// The shift chosen, px, once placed.
    shift: f64,
}

/// What the scene holds besides the lines, read-only.
pub(crate) struct Around<'a> {
    /// `ViewPlan::bridges`.
    pub(crate) written: &'a [(EdgeId, EdgeId)],
    pub(crate) bundles: &'a [RoutedBundle],
    pub(crate) boxes: &'a [SceneBox],
    pub(crate) labels: &'a [SceneLabel],
}

/// Marks every crossing of the routed `lines` in `style` (P4-503..P4-517):
/// fills `runs` and `bars` of the carriers and returns one `SceneBridge`
/// per crossing, in the order of their (lower, higher) line pair, then
/// along the lower line. `crossings` is the routed count: 0 skips all
/// work (P4-519).
pub(crate) fn add(style: CrossingStyle, around: &Around<'_>, lines: &mut [SceneLine], crossings: u32) -> Vec<SceneBridge> {
    if style == CrossingStyle::Plain || crossings == 0 {
        return Vec::new();
    }
    let groups = group_of(around.bundles);
    let group = |e: EdgeId| groups.get(&e).copied().unwrap_or(Group::Line(e));
    let mut sites = find_sites(around.written, lines, &group);
    if sites.is_empty() {
        return Vec::new();
    }
    let mut clusters = cluster(lines, &mut sites);
    place(style, around, lines, &mut sites, &mut clusters, &group);
    draw(style, lines, &clusters, &group);
    clear_chevrons(style, around, lines, &clusters);
    sites
        .iter()
        .map(|s| SceneBridge {
            at: round(s.at),
            carrier: lines[s.carrier].edge,
            passing: lines[s.passing].edge,
            drawn: s.drawn,
            by_default: s.by_default,
        })
        .collect()
}

/// The `bridge` lines the view needs so that `site` is carried by its
/// passing line instead (the editor's "Bridge on the other line",
/// P4-522): every entry of `plan.bridges` naming the same pair of lines or
/// buses is removed, and when `site` follows the default rule an entry
/// with the passing line as carrier is added. Serves P4-505, P4-522.
// APPROVER: new pub item
#[must_use]
pub fn flipped(plan: &ViewPlan, scene: &Scene, site: &SceneBridge) -> Vec<(EdgeId, EdgeId)> {
    let groups = group_of(&scene.bundles);
    let group = |e: EdgeId| groups.get(&e).copied().unwrap_or(Group::Line(e));
    let pair = sorted(group(site.carrier), group(site.passing));
    let mut out: Vec<(EdgeId, EdgeId)> = plan
        .bridges
        .iter()
        .copied()
        .filter(|&(c, o)| sorted(group(c), group(o)) != pair)
        .collect();
    if site.by_default {
        out.push((site.passing, site.carrier));
    }
    out
}

fn sorted(a: Group, b: Group) -> (Group, Group) {
    (a.min(b), a.max(b))
}

/// Each line on a bus (a bundle with a shared stretch, members that left
/// it excepted) mapped to that bus.
fn group_of(bundles: &[RoutedBundle]) -> BTreeMap<EdgeId, Group> {
    let mut out = BTreeMap::new();
    for (k, b) in bundles.iter().enumerate() {
        if b.fallback || b.tracks.is_empty() {
            continue;
        }
        for &m in b.members.iter().filter(|m| !b.left.contains(m)) {
            out.insert(m, Group::Bus(k));
        }
    }
    out
}

/// Every proper crossing between two routed lines with its carrier
/// (P4-503..P4-505), skipping a crossing a `jump true` hop already marks
/// (P2-422).
fn find_sites(written: &[(EdgeId, EdgeId)], lines: &[SceneLine], group: &dyn Fn(EdgeId) -> Group) -> Vec<Site> {
    // The lowest member of each bus, for the default rule.
    let mut lowest: BTreeMap<Group, EdgeId> = BTreeMap::new();
    for l in lines {
        let g = group(l.edge);
        let e = lowest.entry(g).or_insert(l.edge);
        *e = (*e).min(l.edge);
    }
    let low = |g: Group| -> EdgeId {
        match g {
            Group::Line(e) => e,
            Group::Bus(_) => lowest.get(&g).copied().unwrap_or(EdgeId(u32::MAX)),
        }
    };
    let mut out = Vec::new();
    // DECISION: lines whose boxes are apart have no crossing: skipped
    // (same sites, CORE-24).
    let boxes: Vec<Option<(Point, Point)>> = lines.iter().map(|l| (!l.points.is_empty()).then(|| crate::geom::bounds_of(&l.points))).collect();
    for i in 0..lines.len() {
        if !lines[i].routed {
            continue;
        }
        for j in i + 1..lines.len() {
            if !lines[j].routed {
                continue;
            }
            if let (Some(x), Some(y)) = (boxes[i], boxes[j])
                && !crate::geom::boxes_meet(x, y)
            {
                continue;
            }
            let (a, b) = (&lines[i], &lines[j]);
            let (ga, gb) = (group(a.edge), group(b.edge));
            // P4-504: the default carrier.
            let default_a = match (ga, gb) {
                _ if ga == gb => a.edge < b.edge,
                (Group::Bus(_), Group::Line(_)) => false,
                (Group::Line(_), Group::Bus(_)) => true,
                _ => low(ga) < low(gb),
            };
            // P4-505: a `bridge` line for the pair wins.
            let written = written
                .iter()
                .find(|&&(c, o)| ga != gb && sorted(group(c), group(o)) == sorted(ga, gb))
                .map(|&(c, _)| group(c) == ga);
            let a_carries = written.unwrap_or(default_a);
            let by_default = a_carries == default_a;
            let (ci, pi) = if a_carries { (i, j) } else { (j, i) };
            let (c, p) = (&lines[ci], &lines[pi]);
            let c_last = c.points.len() - 1;
            let p_last = p.points.len() - 1;
            for (s, cw) in c.points.windows(2).enumerate() {
                for (ps, pw) in p.points.windows(2).enumerate() {
                    // A joined line's last segment meets its trunk: no
                    // crossing (the L4 rule).
                    if (c.joins == Some(p.edge) && s + 1 == c_last) || (p.joins == Some(c.edge) && ps + 1 == p_last) {
                        continue;
                    }
                    let (sc, sp) = (Segment::new(cw[0], cw[1]), Segment::new(pw[0], pw[1]));
                    if sc.contact(&sp) != Contact::Cross {
                        continue;
                    }
                    let Some(at) = intersect(sc, sp) else { continue };
                    if near_jump(c, at) || near_jump(p, at) {
                        continue;
                    }
                    let a0 = v(cw[0]);
                    let t = dist(a0, at);
                    let square = dot(unit(sc), unit(sp)).abs() < 0.1;
                    out.push(Site {
                        carrier: ci,
                        passing: pi,
                        seg: s,
                        pseg: ps,
                        t,
                        at,
                        half: f64::from(if square { GAP_HALF_SQUARE_PX } else { GAP_HALF_SLANT_PX }),
                        by_default,
                        drawn: true,
                    });
                }
            }
        }
    }
    // Pair order, then along the carrier.
    out.sort_by(|x, y| {
        let kx = (x.carrier.min(x.passing), x.carrier.max(x.passing), x.seg, x.pseg);
        let ky = (y.carrier.min(y.passing), y.carrier.max(y.passing), y.seg, y.pseg);
        kx.cmp(&ky).then(x.t.total_cmp(&y.t))
    });
    out
}

/// True when a `jump true` hop of `line` (P2-422) sits on `at`.
fn near_jump(line: &SceneLine, at: V) -> bool {
    line.jumps.iter().any(|&j| dist(v(j), at) <= 1.5)
}

/// Merges the crossings of each carrier segment into gaps (P4-515):
/// crossings whose gaps overlap or leave less than `MIN_PIECE_PX` between
/// them form one cluster; a cluster whose passing segments are not all
/// parallel is drawn plain.
fn cluster(lines: &[SceneLine], sites: &mut [Site]) -> Vec<Cluster> {
    let mut order: Vec<usize> = (0..sites.len()).collect();
    order.sort_by(|&x, &y| {
        let (a, b) = (&sites[x], &sites[y]);
        (a.carrier, a.seg).cmp(&(b.carrier, b.seg)).then(a.t.total_cmp(&b.t))
    });
    let mut out: Vec<Cluster> = Vec::new();
    for k in order {
        let s = &sites[k];
        let (lo, hi) = (s.t - s.half, s.t + s.half);
        if let Some(last) = out.last_mut()
            && last.carrier == s.carrier
            && last.seg == s.seg
            && lo < last.hi + f64::from(MIN_PIECE_PX)
        {
            last.lo = last.lo.min(lo);
            last.hi = last.hi.max(hi);
            last.sites.push(k);
            continue;
        }
        out.push(Cluster {
            carrier: s.carrier,
            seg: s.seg,
            lo,
            hi,
            sites: vec![k],
            shift: 0.0,
        });
    }
    // Not all parallel: plain.
    out.retain(|c| {
        let dirs: Vec<V> = c
            .sites
            .iter()
            .map(|&k| {
                let p = &lines[sites[k].passing].points;
                unit(Segment::new(p[sites[k].pseg], p[sites[k].pseg + 1]))
            })
            .collect();
        let parallel = dirs.iter().all(|&d| cross(d, dirs[0]).abs() < 1e-9);
        if !parallel {
            for &k in &c.sites {
                sites[k].drawn = false;
            }
        }
        parallel
    });
    out
}

/// Finds each cluster a legal place (P4-516), the same shift for the
/// tracks of one bus crossing the same lines (P4-510, P4-512); a cluster
/// with none, or too near the previous one on its segment, is drawn plain.
#[allow(clippy::too_many_arguments)]
fn place(
    style: CrossingStyle,
    around: &Around<'_>,
    lines: &[SceneLine],
    sites: &mut [Site],
    clusters: &mut Vec<Cluster>,
    group: &dyn Fn(EdgeId) -> Group,
) {
    // Clusters that move together: a bus's tracks crossing the same lines.
    let mut sets: BTreeMap<(Group, Vec<usize>), Vec<usize>> = BTreeMap::new();
    for (k, c) in clusters.iter().enumerate() {
        let mut passing: Vec<usize> = c.sites.iter().map(|&s| sites[s].passing).collect();
        passing.sort_unstable();
        passing.dedup();
        let g = group(lines[c.carrier].edge);
        let key = match g {
            Group::Bus(_) => (g, passing),
            Group::Line(_) => (g, vec![usize::MAX - k]),
        };
        sets.entry(key).or_default().push(k);
    }
    let obstacles = Obstacles::new(around, lines);
    let mut keep = vec![false; clusters.len()];
    for members in sets.values() {
        let shift = shifts().find(|&d| {
            members.iter().all(|&k| {
                let c = &clusters[k];
                let passing: Vec<(usize, usize)> = c.sites.iter().map(|&s| (sites[s].passing, sites[s].pseg)).collect();
                legal(style, lines, &obstacles, c.carrier, c.seg, c.lo + d, c.hi + d, &passing)
            })
        });
        if let Some(d) = shift {
            for &k in members {
                clusters[k].shift = d;
                keep[k] = true;
            }
        }
    }
    // Neighbours on one segment keep MIN_PIECE_PX apart.
    for k in 1..clusters.len() {
        let (a, b) = (&clusters[k - 1], &clusters[k]);
        if keep[k - 1] && keep[k] && a.carrier == b.carrier && a.seg == b.seg && b.lo + b.shift < a.hi + a.shift + f64::from(MIN_PIECE_PX) {
            keep[k] = false;
        }
    }
    let mut k = 0;
    clusters.retain(|c| {
        let ok = keep[k];
        k += 1;
        if !ok {
            for &s in &c.sites {
                sites[s].drawn = false;
            }
        }
        ok
    });
}

/// 0, then 1, -1, 2, -2 ... up to `MAX_SHIFT_PX`: nearer first, forward
/// first.
fn shifts() -> impl Iterator<Item = f64> {
    std::iter::once(0).chain((1..=MAX_SHIFT_PX).flat_map(|d| [d, -d])).map(f64::from)
}

/// What a bridge keeps clear of, px.
struct Obstacles {
    rects: Vec<Rect>,
    /// Segments of arrowheads and callout marks, with the line they
    /// belong to (`usize::MAX` for none).
    marks: Vec<(usize, V, V)>,
    /// Segments of chevrons, with their line.
    chevrons: Vec<(usize, V, V)>,
    dots: Vec<(V, f64)>,
    /// Per line, the box of its points.
    line_boxes: Vec<Bbox>,
}

impl Obstacles {
    fn new(around: &Around<'_>, lines: &[SceneLine]) -> Obstacles {
        let mut rects: Vec<Rect> = around.boxes.iter().map(|b| b.rect).collect();
        let mut marks = Vec::new();
        let mut chevrons = Vec::new();
        let mut dots = Vec::new();
        for l in around.labels {
            rects.push(l.rect);
            if let Some(c) = &l.callout {
                marks.push((usize::MAX, v(c.shelf[0]), v(c.shelf[1])));
                marks.push((usize::MAX, v(c.leader[0]), v(c.leader[1])));
                dots.push((v(c.dot), f64::from(c.dot_radius_px)));
            }
        }
        for (i, l) in lines.iter().enumerate() {
            for c in &l.chevrons {
                chevrons.push((i, v(c.points[0]), v(c.points[1])));
                chevrons.push((i, v(c.points[1]), v(c.points[2])));
            }
            match l.tip {
                Tip::Arrow(p) => {
                    for k in 0..3 {
                        marks.push((i, v(p[k]), v(p[(k + 1) % 3])));
                    }
                }
                Tip::Dot { at, radius_px } => dots.push((v(at), f64::from(radius_px))),
            }
        }
        let line_boxes = lines
            .iter()
            .map(|l| Bbox::of_segments(&l.points.iter().map(|&p| (v(p), v(p))).collect::<Vec<_>>()))
            .collect();
        Obstacles { rects, marks, chevrons, dots, line_boxes }
    }
}

/// True when a gap or hop over `lo..hi` of segment `seg` of `carrier`
/// passing `passing` keeps clear of everything else (P4-516).
#[allow(clippy::too_many_arguments)]
fn legal(
    style: CrossingStyle,
    lines: &[SceneLine],
    obstacles: &Obstacles,
    carrier: usize,
    seg: usize,
    lo: f64,
    hi: f64,
    passing: &[(usize, usize)],
) -> bool {
    let pts = &lines[carrier].points;
    let (a, b) = (pts[seg], pts[seg + 1]);
    let len = dist(v(a), v(b));
    let corner = f64::from(CORNER_CLEAR_PX);
    if lo < corner || hi > len - corner {
        return false;
    }
    let marks = shapes(style, a, b, lo, hi);
    let core = (v(at_along(a, b, lo)), v(at_along(a, b, hi)));
    let mut all = marks.clone();
    all.push(core);
    let clear = f64::from(CLEAR_PX) + 1.0;
    // Every mark lies in `zone`: an obstacle whose box keeps `clear` (and
    // a margin) away from it is clear of every mark, as the full test
    // would find, so the full test runs only on the near ones.
    // DECISION: a box cut before the exact distance tests, not a spatial
    // index: every candidate place tested every segment, box, mark and dot
    // of the view with four distance tests each (23-45 ms a scene with
    // 150-230 crossings, measured); the cut leaves the answer and so every
    // byte of the scene as it was, and takes it to about 2.7 ms.
    let zone = Bbox::of_segments(&all);
    // The passing lines: the segments crossed keep clear of the marks
    // (a hop's top crosses them), their other segments of the gap too.
    // DECISION: a line whose box is apart from `zone` has every segment
    // apart too: skipped whole (the same answer, CORE-24).
    for &(p, _) in passing {
        if zone.apart(&obstacles.line_boxes[p], clear) {
            continue;
        }
        for (k, w) in lines[p].points.windows(2).enumerate() {
            let s = (v(w[0]), v(w[1]));
            if zone.apart(&Bbox::of_segment(s), clear) {
                continue;
            }
            let crossed = passing.contains(&(p, k));
            let hit = all.iter().enumerate().any(|(n, &m)| {
                let top = style == CrossingStyle::Jump && n == 1;
                let gap = n == all.len() - 1;
                !(crossed && (top || gap)) && seg_seg(m, s) < clear
            });
            if hit {
                return false;
            }
        }
    }
    // Every other line keeps clear.
    for (i, l) in lines.iter().enumerate() {
        if i == carrier || passing.iter().any(|&(p, _)| p == i) || zone.apart(&obstacles.line_boxes[i], clear) {
            continue;
        }
        for w in l.points.windows(2) {
            let s = (v(w[0]), v(w[1]));
            if zone.apart(&Bbox::of_segment(s), clear) {
                continue;
            }
            if all.iter().any(|&m| seg_seg(m, s) < clear) {
                return false;
            }
        }
    }
    for r in &obstacles.rects {
        if zone.apart(&Bbox::of_rect(r), clear) {
            continue;
        }
        if all.iter().any(|&m| seg_rect(m, r) < clear) {
            return false;
        }
    }
    for &(_, p, q) in &obstacles.marks {
        if zone.apart(&Bbox::of_segment((p, q)), clear) {
            continue;
        }
        if all.iter().any(|&m| seg_seg(m, (p, q)) < clear) {
            return false;
        }
    }
    // P4-516: the bridge wins over the chevrons of its own lines
    // (`clear_chevrons` slides them away); other lines' chevrons stay.
    for &(i, p, q) in &obstacles.chevrons {
        if i == carrier || passing.iter().any(|&(l, _)| l == i) {
            continue;
        }
        if zone.apart(&Bbox::of_segment((p, q)), clear) {
            continue;
        }
        if all.iter().any(|&m| seg_seg(m, (p, q)) < clear) {
            return false;
        }
    }
    for &(c, r) in &obstacles.dots {
        if zone.apart(&Bbox::of_segment((c, c)), r + clear) {
            continue;
        }
        if all.iter().any(|&m| seg_point(m, c) < r + clear) {
            return false;
        }
    }
    true
}

/// An axis-aligned box round some px geometry, for cheap distance cuts.
#[derive(Debug, Clone, Copy)]
struct Bbox {
    min: V,
    max: V,
}

/// The margin, px, by which `Bbox::apart` overshoots its distance: far
/// more than any rounding in `seg_seg`, `seg_rect` and `seg_point`, so a
/// pair it calls apart is one those tests also find at `d` or more.
const APART_MARGIN_PX: f64 = 0.5;

impl Bbox {
    fn of_segment(s: (V, V)) -> Bbox {
        Bbox {
            min: (s.0.0.min(s.1.0), s.0.1.min(s.1.1)),
            max: (s.0.0.max(s.1.0), s.0.1.max(s.1.1)),
        }
    }

    fn of_segments(all: &[(V, V)]) -> Bbox {
        let mut b = Bbox {
            min: (f64::INFINITY, f64::INFINITY),
            max: (f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
        for &s in all {
            b = b.union(&Bbox::of_segment(s));
        }
        b
    }

    fn of_rect(r: &Rect) -> Bbox {
        Bbox {
            min: (f64::from(r.min.x), f64::from(r.min.y)),
            max: (f64::from(r.max.x), f64::from(r.max.y)),
        }
    }

    fn union(&self, o: &Bbox) -> Bbox {
        Bbox {
            min: (self.min.0.min(o.min.0), self.min.1.min(o.min.1)),
            max: (self.max.0.max(o.max.0), self.max.1.max(o.max.1)),
        }
    }

    /// True when everything in `self` is at least `d` (plus the margin)
    /// from everything in `o`: the boxes are that far apart on one axis.
    fn apart(&self, o: &Bbox, d: f64) -> bool {
        let gap = (o.min.0 - self.max.0).max(self.min.0 - o.max.0).max(o.min.1 - self.max.1).max(self.min.1 - o.max.1);
        gap >= d + APART_MARGIN_PX
    }
}

/// Least distance, px, from a slid chevron's point to either end of its
/// segment.
const CHEVRON_END_PX: f64 = 16.0;

/// Least distance, px, from a chevron to a label (CORE-10) and to a
/// bridge (P4-516).
const CHEVRON_LABEL_PX: f64 = 8.0;

/// Slides every chevron that a drawn bridge would touch along its own
/// segment to the nearest place clear of every bridge, at least
/// `CHEVRON_END_PX` from the segment's ends and `CHEVRON_LABEL_PX` from
/// labels and callout dots, 1px steps, forward first; removes it when
/// there is none (P4-516).
// DECISION: slide on the chevron's own segment only, so it still points
// the way that segment runs; nearest first keeps the CORE-10 spacing as
// close to the quarters as the bridge allows.
fn clear_chevrons(style: CrossingStyle, around: &Around<'_>, lines: &mut [SceneLine], clusters: &[Cluster]) {
    let mut zones: Vec<(V, V)> = Vec::new();
    for c in clusters {
        let pts = &lines[c.carrier].points;
        let (a, b) = (pts[c.seg], pts[c.seg + 1]);
        let (lo, hi) = (c.lo + c.shift, c.hi + c.shift);
        zones.extend(shapes(style, a, b, lo, hi));
        zones.push((v(at_along(a, b, lo)), v(at_along(a, b, hi))));
    }
    for l in lines.iter() {
        zones.extend(l.bars.iter().map(|bar| (v(bar[0]), v(bar[1]))));
    }
    if zones.is_empty() {
        return;
    }
    let clear = CHEVRON_LABEL_PX;
    let boxes: Vec<Bbox> = zones.iter().map(|&z| Bbox::of_segment(z)).collect();
    // A chevron slid along its segment stays within its line's box grown
    // by the chevron's reach from its point: the zones apart from that
    // are clear of every place it may take, and are left out per line.
    let blocked = |near: &[usize], pts: &[Point; 3]| {
        let segs = [(v(pts[0]), v(pts[1])), (v(pts[1]), v(pts[2]))];
        let b = Bbox::of_segments(&segs);
        near.iter().any(|&k| !boxes[k].apart(&b, clear) && segs.iter().any(|&s| seg_seg(s, zones[k]) < clear))
    };
    let near_label = |pts: &[Point; 3]| {
        around.labels.iter().any(|l| pts.iter().any(|&p| seg_rect((v(p), v(p)), &l.rect) < CHEVRON_LABEL_PX))
            || around.labels.iter().filter_map(|l| l.callout.as_ref()).any(|c| pts.iter().any(|&p| dist(v(p), v(c.dot)) < CHEVRON_LABEL_PX + f64::from(c.dot_radius_px)))
    };
    for l in lines.iter_mut() {
        if l.chevrons.is_empty() {
            continue;
        }
        let route = l.points.clone();
        let reach = l
            .chevrons
            .iter()
            .flat_map(|ch| ch.points.iter().map(move |p| (p.x - ch.at.x).abs().max((p.y - ch.at.y).abs())))
            .max()
            .unwrap_or(0);
        // The route's box, and the chevrons where they stand now (one off
        // the route would be tested in place before it is dropped).
        let route_box = route
            .windows(2)
            .map(|w| Bbox::of_segment((v(w[0]), v(w[1]))))
            .chain(l.chevrons.iter().flat_map(|ch| ch.points.iter().map(|&p| Bbox::of_segment((v(p), v(p))))))
            .fold(Bbox::of_segment((v(route[0]), v(route[0]))), |b, o| b.union(&o));
        let reach = f64::from(reach);
        let grown = Bbox {
            min: (route_box.min.0 - reach, route_box.min.1 - reach),
            max: (route_box.max.0 + reach, route_box.max.1 + reach),
        };
        let near: Vec<usize> = (0..zones.len()).filter(|&k| !boxes[k].apart(&grown, clear)).collect();
        let mut kept = Vec::with_capacity(l.chevrons.len());
        for ch in std::mem::take(&mut l.chevrons) {
            if !blocked(&near, &ch.points) {
                kept.push(ch);
                continue;
            }
            let Some(w) = route.windows(2).find(|w| on_segment(w[0], w[1], ch.at)) else { continue };
            let (a, b) = (w[0], w[1]);
            let (sx, sy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
            let len = dist(v(a), v(b));
            let step = if sx != 0 && sy != 0 { SQRT_2 } else { 1.0 };
            let from_a = dist(v(a), v(ch.at));
            #[allow(clippy::cast_possible_truncation)]
            let max = (len / step).ceil() as i32;
            let moved = (1..=max).flat_map(|k| [k, -k]).find_map(|k| {
                let along = from_a + f64::from(k) * step;
                if along < CHEVRON_END_PX || along > len - CHEVRON_END_PX {
                    return None;
                }
                let pts = ch.points.map(|p| Point::new(p.x + sx * k, p.y + sy * k));
                (!blocked(&near, &pts) && !near_label(&pts)).then_some((k, pts))
            });
            if let Some((k, pts)) = moved {
                let mut ch = ch;
                ch.at = Point::new(ch.at.x + sx * k, ch.at.y + sy * k);
                ch.points = pts;
                kept.push(ch);
            }
        }
        l.chevrons = kept;
    }
}

/// True when `p` lies on the closed segment `a..b`.
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross = i64::from(b.x - a.x) * i64::from(p.y - a.y) - i64::from(b.y - a.y) * i64::from(p.x - a.x);
    cross == 0 && p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// The drawn marks of a bridge over `lo..hi` of `a..b`: the two bars
/// (gap) or the hop's three sides (jump), px.
fn shapes(style: CrossingStyle, a: Point, b: Point, lo: f64, hi: f64) -> Vec<(V, V)> {
    let (p, q) = (at_along(a, b, lo), at_along(a, b, hi));
    if style == CrossingStyle::Jump {
        let (hx, hy) = hop_offset(a, b);
        let (p1, q1) = (Point::new(p.x + hx, p.y + hy), Point::new(q.x + hx, q.y + hy));
        vec![(v(p), v(p1)), (v(p1), v(q1)), (v(q1), v(q))]
    } else {
        let [x, y] = bar(a, b, p);
        let [z, w] = bar(a, b, q);
        vec![(v(x), v(y)), (v(z), v(w))]
    }
}

/// Writes the runs and bars of every carrier (P4-506..P4-513), then
/// fuses the bars of one bus (P4-510).
fn draw(style: CrossingStyle, lines: &mut [SceneLine], clusters: &[Cluster], group: &dyn Fn(EdgeId) -> Group) {
    let mut by_line: BTreeMap<usize, Vec<&Cluster>> = BTreeMap::new();
    for c in clusters {
        by_line.entry(c.carrier).or_default().push(c);
    }
    // Bars with their line, for fusing.
    let mut bars: Vec<(usize, Point, [Point; 2])> = Vec::new();
    for (&i, cs) in &by_line {
        let pts = lines[i].points.clone();
        let mut runs: Vec<Vec<Point>> = vec![vec![pts[0]]];
        for (s, w) in pts.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let mut here: Vec<&&Cluster> = cs.iter().filter(|c| c.seg == s).collect();
            here.sort_by(|x, y| x.lo.total_cmp(&y.lo));
            for c in here {
                let (p, q) = (at_along(a, b, c.lo + c.shift), at_along(a, b, c.hi + c.shift));
                let run = runs.last_mut().expect("one run");
                run.push(p);
                if style == CrossingStyle::Jump {
                    let (hx, hy) = hop_offset(a, b);
                    run.push(Point::new(p.x + hx, p.y + hy));
                    run.push(Point::new(q.x + hx, q.y + hy));
                    run.push(q);
                } else {
                    runs.push(vec![q]);
                    bars.push((i, p, bar(a, b, p)));
                    bars.push((i, q, bar(a, b, q)));
                }
            }
            runs.last_mut().expect("one run").push(b);
        }
        lines[i].runs = runs;
    }
    // P4-510: neighbouring bars of one bus meet halfway.
    let n = bars.len();
    let mut ends: Vec<[Point; 2]> = bars.iter().map(|b| b.2).collect();
    for x in 0..n {
        for y in 0..n {
            let (bx, by) = (&bars[x], &bars[y]);
            if x == y || !matches!(group(lines[bx.0].edge), Group::Bus(_)) || group(lines[bx.0].edge) != group(lines[by.0].edge) {
                continue;
            }
            let dir = unit_of(v(bx.2[0]), v(bx.2[1]));
            let d = (f64::from(by.1.x - bx.1.x), f64::from(by.1.y - bx.1.y));
            let along = dot(d, dir);
            if cross(d, dir).abs() > 1.5 || along.abs() > FUSE_REACH_PX || along.abs() < 1.0 {
                continue;
            }
            let mid = Point::new((bx.1.x + by.1.x).div_euclid(2), (bx.1.y + by.1.y).div_euclid(2));
            // End 1 points along `dir`.
            let k = usize::from(along > 0.0);
            ends[x][k] = mid;
        }
    }
    for (b, e) in bars.iter().zip(ends) {
        lines[b.0].bars.push(e);
    }
}

// ------------------------------------------------------------ geometry

fn v(p: Point) -> V {
    (f64::from(p.x), f64::from(p.y))
}

#[allow(clippy::cast_possible_truncation)]
fn round(p: V) -> Point {
    Point::new(p.0.round() as i32, p.1.round() as i32)
}

fn dist(a: V, b: V) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn dot(a: V, b: V) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn cross(a: V, b: V) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn unit_of(a: V, b: V) -> V {
    let l = dist(a, b).max(1e-9);
    ((b.0 - a.0) / l, (b.1 - a.1) / l)
}

fn unit(s: Segment) -> V {
    unit_of(v(s.a), v(s.b))
}

/// The crossing point of two segments that cross.
fn intersect(s: Segment, t: Segment) -> Option<V> {
    let (p, r) = (v(s.a), (f64::from(s.b.x - s.a.x), f64::from(s.b.y - s.a.y)));
    let (q, w) = (v(t.a), (f64::from(t.b.x - t.a.x), f64::from(t.b.y - t.a.y)));
    let den = cross(r, w);
    if den.abs() < 1e-12 {
        return None;
    }
    let k = cross((q.0 - p.0, q.1 - p.1), w) / den;
    Some((p.0 + r.0 * k, p.1 + r.1 * k))
}

/// The whole-px point `t` px from `a` towards `b`: per axis step on a
/// diagonal, so it lies exactly on the segment.
#[allow(clippy::cast_possible_truncation)]
fn at_along(a: Point, b: Point, t: f64) -> Point {
    let (sx, sy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    let k = if sx != 0 && sy != 0 { (t / SQRT_2).round() } else { t.round() } as i32;
    Point::new(a.x + sx * k, a.y + sy * k)
}

/// `len` px square to `a..b`, whole px (per axis step on a diagonal),
/// turned left of the direction (y down).
#[allow(clippy::cast_possible_truncation)]
fn normal(a: Point, b: Point, len: i32) -> (i32, i32) {
    let (sx, sy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    let k = if sx != 0 && sy != 0 { (f64::from(len) / SQRT_2).round() as i32 } else { len };
    (sy * k, -sx * k)
}

/// The T bar at `p` on `a..b` (P4-506).
fn bar(a: Point, b: Point, p: Point) -> [Point; 2] {
    let (nx, ny) = normal(a, b, BAR_HALF_PX);
    [Point::new(p.x - nx, p.y - ny), Point::new(p.x + nx, p.y + ny)]
}

/// The hop's offset from `a..b` (P4-508): `HOP_PX` square to it, on the
/// upper side, or the left one for a vertical carrier.
// DECISION: up, else left: the side a reader expects a jump on, the
// same for every carrier whatever its direction, so neighbouring hops
// and a bus's tracks all stand out the same way (P4-512).
fn hop_offset(a: Point, b: Point) -> (i32, i32) {
    let (nx, ny) = normal(a, b, HOP_PX);
    if ny < 0 || (ny == 0 && nx < 0) { (nx, ny) } else { (-nx, -ny) }
}

fn seg_point(s: (V, V), p: V) -> f64 {
    let (a, b) = s;
    let d = (b.0 - a.0, b.1 - a.1);
    let l2 = dot(d, d);
    if l2 < 1e-12 {
        return dist(a, p);
    }
    let k = (dot((p.0 - a.0, p.1 - a.1), d) / l2).clamp(0.0, 1.0);
    dist((a.0 + d.0 * k, a.1 + d.1 * k), p)
}

fn seg_seg(s: (V, V), t: (V, V)) -> f64 {
    let o = |p: V, q: V, r: V| cross((q.0 - p.0, q.1 - p.1), (r.0 - p.0, r.1 - p.1));
    let (d1, d2) = (o(t.0, t.1, s.0), o(t.0, t.1, s.1));
    let (d3, d4) = (o(s.0, s.1, t.0), o(s.0, s.1, t.1));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    seg_point(t, s.0).min(seg_point(t, s.1)).min(seg_point(s, t.0)).min(seg_point(s, t.1))
}

fn seg_rect(s: (V, V), r: &Rect) -> f64 {
    let inside = |p: V| p.0 >= f64::from(r.min.x) && p.0 <= f64::from(r.max.x) && p.1 >= f64::from(r.min.y) && p.1 <= f64::from(r.max.y);
    if inside(s.0) || inside(s.1) {
        return 0.0;
    }
    let (x0, y0, x1, y1) = (f64::from(r.min.x), f64::from(r.min.y), f64::from(r.max.x), f64::from(r.max.y));
    [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))]
        .into_iter()
        .map(|e| seg_seg(s, e))
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Colour, RelationshipId};
    use crate::scene::Stroke;
    use crate::view::BoxId;

    /// A routed 2px line through `points`, no chevrons, a dot at its end.
    fn hand_line(edge: u32, points: &[(i32, i32)]) -> SceneLine {
        let points: Vec<Point> = points.iter().map(|&(x, y)| Point::new(x, y)).collect();
        SceneLine {
            edge: EdgeId(edge),
            relationship: RelationshipId(edge),
            relationships: vec![RelationshipId(edge)],
            source: BoxId(0),
            destination: BoxId(1),
            tip: Tip::Dot {
                at: *points.last().expect("points"),
                radius_px: 0,
            },
            points,
            stroke: Stroke {
                colour: Colour { r: 0, g: 0, b: 0 },
                width_px: 2,
                dash_px: None,
            },
            chevrons: Vec::new(),
            joins: None,
            routed: true,
            opacity: 100,
            jumps: Vec::new(),
            runs: Vec::new(),
            bars: Vec::new(),
            thickness_written: false,
        }
    }

    fn run(lines: &mut [SceneLine]) -> Vec<SceneBridge> {
        let around = Around {
            written: &[],
            bundles: &[],
            boxes: &[],
            labels: &[],
        };
        add(CrossingStyle::Gap, &around, lines, 1)
    }

    #[test]
    fn p4_515_close_crossings_not_parallel_are_plain() {
        // A vertical at x 200 and a short diagonal through x 215.
        let mut lines = [
            hand_line(0, &[(0, 0), (400, 0)]),
            hand_line(1, &[(200, -100), (200, 100)]),
            hand_line(2, &[(205, -10), (225, 10)]),
        ];
        let b = run(&mut lines);
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|x| !x.drawn), "{b:?}");
        assert!(lines[0].runs.is_empty());
        // Parallel ones merge into one gap.
        let mut lines = [
            hand_line(0, &[(0, 0), (400, 0)]),
            hand_line(1, &[(200, -100), (200, 100)]),
            hand_line(2, &[(220, -100), (220, 100)]),
        ];
        let b = run(&mut lines);
        assert!(b.iter().all(|x| x.drawn));
        assert_eq!(lines[0].runs, vec![vec![Point::new(0, 0), Point::new(192, 0)], vec![Point::new(228, 0), Point::new(400, 0)]]);
    }

    #[test]
    fn p4_516_moved_off_a_corner_or_plain() {
        // 18px from a bend: the gap would end 10px from it; moved back 2px.
        let mut lines = [hand_line(0, &[(0, 0), (100, 0), (100, 200)]), hand_line(1, &[(82, -100), (82, 100)])];
        let b = run(&mut lines);
        assert!(b[0].drawn);
        assert_eq!(lines[0].runs[0].last(), Some(&Point::new(72, 0)));
        assert_eq!(lines[0].runs[1][0], Point::new(88, 0));
        // 5px from the bend: no legal place within 4px.
        let mut lines = [hand_line(0, &[(0, 0), (100, 0), (100, 200)]), hand_line(1, &[(95, -100), (95, 100)])];
        let b = run(&mut lines);
        assert!(!b[0].drawn);
        assert!(lines[0].runs.is_empty());
        // A third line ending 3px beside a bar: moved 1px off it.
        let mut lines = [
            hand_line(0, &[(0, 0), (400, 0)]),
            hand_line(1, &[(200, -100), (200, 100)]),
            hand_line(2, &[(189, -50), (189, -6)]),
        ];
        let b = run(&mut lines);
        assert!(b[0].drawn, "{b:?}");
        assert_eq!(lines[0].runs[0].last(), Some(&Point::new(193, 0)));
        // One on each side: no legal place, plain.
        let mut lines = [
            hand_line(0, &[(0, 0), (400, 0)]),
            hand_line(1, &[(200, -100), (200, 100)]),
            hand_line(2, &[(189, -50), (189, -6)]),
            hand_line(3, &[(211, -50), (211, -6)]),
        ];
        let b = run(&mut lines);
        assert!(!b[0].drawn, "{b:?}");
    }

    #[test]
    fn p4_516_box_cut_never_skips_a_near_obstacle() {
        // `Bbox::apart` only skips what the full tests also find clear:
        // over many segments, rects and points, whole px and on the
        // diagonals as the scene draws them, at and around the distances
        // `legal` and `clear_chevrons` use, a pair called apart is never
        // nearer than the distance by `seg_seg`, `seg_rect` or `seg_point`.
        let mut x = 0x5EED_u64;
        let mut next = |n: i32| {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            i32::try_from((z ^ (z >> 31)) % u64::try_from(2 * n + 1).expect("n")).expect("fits") - n
        };
        let mut checked = 0;
        for _ in 0..20_000 {
            let a = (f64::from(next(40)), f64::from(next(40)));
            let k = f64::from(next(12));
            let m = match next(2) {
                -1 => (a, (a.0 + k, a.1)),
                0 => (a, (a.0, a.1 + k)),
                _ => (a, (a.0 + k, a.1 + k)),
            };
            let b = (f64::from(next(40)), f64::from(next(40)));
            let s = (b, (b.0 + f64::from(next(15)), b.1 + f64::from(next(15))));
            let r = Rect::from_corners(Point::new(next(40), next(40)), Point::new(next(40), next(40)));
            for d in [f64::from(CLEAR_PX) + 1.0, CHEVRON_LABEL_PX, 4.5, 11.0] {
                let zone = Bbox::of_segments(&[m]);
                if zone.apart(&Bbox::of_segment(s), d) {
                    assert!(seg_seg(m, s) >= d, "{m:?} {s:?} {d}");
                    checked += 1;
                }
                if zone.apart(&Bbox::of_rect(&r), d) {
                    assert!(seg_rect(m, &r) >= d, "{m:?} {r:?} {d}");
                }
                if zone.apart(&Bbox::of_segment((b, b)), d) {
                    assert!(seg_point(m, b) >= d, "{m:?} {b:?} {d}");
                }
            }
        }
        assert!(checked > 10_000, "enough pairs called apart ({checked})");
    }

    #[test]
    fn hop_side_is_up_or_left() {
        let o = Point::new(0, 0);
        assert_eq!(hop_offset(o, Point::new(100, 0)), (0, -8));
        assert_eq!(hop_offset(o, Point::new(-100, 0)), (0, -8));
        assert_eq!(hop_offset(o, Point::new(0, 100)), (-8, 0));
        assert_eq!(hop_offset(o, Point::new(0, -100)), (-8, 0));
        assert_eq!(hop_offset(o, Point::new(100, 100)), (6, -6));
    }

    #[test]
    fn diagonal_points_stay_on_the_line() {
        let p = at_along(Point::new(0, 0), Point::new(100, -100), 14.0);
        assert_eq!(p, Point::new(10, -10));
        assert_eq!(bar(Point::new(0, 0), Point::new(100, 100), Point::new(10, 10)), [Point::new(6, 14), Point::new(14, 6)]);
    }
}
