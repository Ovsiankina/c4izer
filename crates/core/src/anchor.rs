//! Line anchors: a named point on a box outline that one end of a drawn
//! line is pinned to (SRS 4.8 P4-200..P4-212). The point is stored
//! relative to its box (a corner, or a side and a percent along it), so
//! it follows the box when the box moves or is resized; this module turns
//! it into a grid point on the box rectangle and lists a box's snap
//! points for the editor.
//!
//! Depends on: `geom`.

use std::fmt;

use crate::geom::{GRID_PX, Point, Rect};

/// One side of a box, as an anchor names it (`top`, `right`, `bottom`,
/// `left`). Serves P4-201.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnchorSide {
    /// The top side (-y).
    Top,
    /// The right side (+x).
    Right,
    /// The bottom side (+y).
    Bottom,
    /// The left side (-x).
    Left,
}

impl AnchorSide {
    /// The keyword as written in an `anchor` line.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Left => "left",
        }
    }
}

/// A corner of a box, as an anchor names it. Serves P4-201, P4-207.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnchorCorner {
    /// `topLeft`.
    TopLeft,
    /// `topRight`.
    TopRight,
    /// `bottomRight`.
    BottomRight,
    /// `bottomLeft`.
    BottomLeft,
}

impl AnchorCorner {
    /// The keyword as written in an `anchor` line.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::TopLeft => "topLeft",
            Self::TopRight => "topRight",
            Self::BottomRight => "bottomRight",
            Self::BottomLeft => "bottomLeft",
        }
    }

    /// The two sides meeting at the corner, the top or bottom one first
    /// (P4-207).
    #[must_use]
    pub const fn sides(self) -> [AnchorSide; 2] {
        match self {
            Self::TopLeft => [AnchorSide::Top, AnchorSide::Left],
            Self::TopRight => [AnchorSide::Top, AnchorSide::Right],
            Self::BottomRight => [AnchorSide::Bottom, AnchorSide::Right],
            Self::BottomLeft => [AnchorSide::Bottom, AnchorSide::Left],
        }
    }
}

/// A point on a box outline that a line end is pinned to: a corner, or
/// a side and a whole percent `0..=100` along it (from the left on `top`
/// and `bottom`, from the top on `left` and `right`). Serves P4-201,
/// P4-202.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Anchor {
    /// A corner of the box.
    Corner(AnchorCorner),
    /// A point on a side, `percent` along it (`0..=100`).
    Side {
        /// The side.
        side: AnchorSide,
        /// How far along the side, in whole percent.
        percent: u8,
    },
}

impl Anchor {
    /// The point `percent` along `side`; `percent` above 100 counts as
    /// 100.
    #[must_use]
    pub const fn side(side: AnchorSide, percent: u8) -> Self {
        Self::Side {
            side,
            percent: if percent > 100 { 100 } else { percent },
        }
    }

    /// The anchor written `word` (a corner or a side keyword, any case)
    /// with `percent` for a side, 50 when `None`. `None` for an unknown
    /// word, a percent above 100, or a percent after a corner. Serves
    /// P4-201.
    #[must_use]
    pub fn parse(word: &str, percent: Option<u32>) -> Option<Self> {
        let w = word.to_ascii_lowercase();
        let corner = match w.as_str() {
            "topleft" => Some(AnchorCorner::TopLeft),
            "topright" => Some(AnchorCorner::TopRight),
            "bottomright" => Some(AnchorCorner::BottomRight),
            "bottomleft" => Some(AnchorCorner::BottomLeft),
            _ => None,
        };
        if let Some(c) = corner {
            return percent.is_none().then_some(Self::Corner(c));
        }
        let side = match w.as_str() {
            "top" => AnchorSide::Top,
            "right" => AnchorSide::Right,
            "bottom" => AnchorSide::Bottom,
            "left" => AnchorSide::Left,
            _ => return None,
        };
        let percent = u8::try_from(percent.unwrap_or(50)).ok().filter(|p| *p <= 100)?;
        Some(Self::Side { side, percent })
    }

    /// The point on `rect` (grid units) this anchor names (P4-202): a
    /// corner is that corner; a side point lies `round(length * percent /
    /// 100)` units from the side's start (its left or top end).
    #[must_use]
    pub const fn point(self, rect: Rect) -> Point {
        match self {
            Self::Corner(AnchorCorner::TopLeft) => rect.min,
            Self::Corner(AnchorCorner::TopRight) => Point::new(rect.max.x, rect.min.y),
            Self::Corner(AnchorCorner::BottomRight) => rect.max,
            Self::Corner(AnchorCorner::BottomLeft) => Point::new(rect.min.x, rect.max.y),
            Self::Side { side, percent } => {
                let p = percent as i32;
                match side {
                    AnchorSide::Top => Point::new(rect.min.x + along(rect.width(), p), rect.min.y),
                    AnchorSide::Bottom => Point::new(rect.min.x + along(rect.width(), p), rect.max.y),
                    AnchorSide::Left => Point::new(rect.min.x, rect.min.y + along(rect.height(), p)),
                    AnchorSide::Right => Point::new(rect.max.x, rect.min.y + along(rect.height(), p)),
                }
            }
        }
    }

    /// The sides a line may leave this anchor square to: one for a side
    /// point, the two that meet at a corner (P4-206, P4-207).
    #[must_use]
    pub fn sides(self) -> Vec<AnchorSide> {
        match self {
            Self::Corner(c) => c.sides().to_vec(),
            Self::Side { side, .. } => vec![side],
        }
    }
}

/// `round(len * percent / 100)` for `len >= 0`, halves rounded up.
const fn along(len: i32, percent: i32) -> i32 {
    (len * percent + 50).div_euclid(100)
}

impl fmt::Display for Anchor {
    /// The point as written in an `anchor` line: `topLeft`, `top` (the
    /// middle), `top 25`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Corner(c) => f.write_str(c.keyword()),
            Self::Side { side, percent: 50 } => f.write_str(side.keyword()),
            Self::Side { side, percent } => write!(f, "{} {percent}", side.keyword()),
        }
    }
}

/// Snap points are at least this far apart along a side, px (P4-205).
pub const SNAP_SPACING_PX: i32 = 120;

/// The snap points of a box with rectangle `rect` (grid units), each with
/// its grid point (P4-205): the four corners, then per side (top, right,
/// bottom, left) `n - 1` points spaced `L / n` apart, with `n = 2 *
/// max(1, floor(L / 120))` for a side `L` px long, so the middle is always
/// one and points sit at least 60 px apart before rounding to the grid.
/// Each point's percent is the one resolving nearest to `k L / n` (`k =
/// 1..n-1`), ties to `round(100 k / n)`.
// DECISION: `n` is even so the middle is a snap point; one extra pair of
// points per 120 px keeps them 50 px or more apart once on the grid,
// more than the 30 px lane spacing the router avoids (CORE-12), so lines
// pinned to neighbouring points can run side by side.
// DECISION: the percent is searched (101 candidates) instead of taken as
// `round(100 k / n)`, because rounding the percent and then the grid
// point can pull neighbours 20 px closer on a long side.
#[must_use]
pub fn snap_points(rect: Rect) -> Vec<(Anchor, Point)> {
    let mut out: Vec<(Anchor, Point)> = [
        AnchorCorner::TopLeft,
        AnchorCorner::TopRight,
        AnchorCorner::BottomRight,
        AnchorCorner::BottomLeft,
    ]
    .into_iter()
    .map(|c| (Anchor::Corner(c), Anchor::Corner(c).point(rect)))
    .collect();
    for side in [AnchorSide::Top, AnchorSide::Right, AnchorSide::Bottom, AnchorSide::Left] {
        let len = match side {
            AnchorSide::Top | AnchorSide::Bottom => rect.width(),
            AnchorSide::Left | AnchorSide::Right => rect.height(),
        };
        let n = 2 * (len * GRID_PX / SNAP_SPACING_PX).max(1);
        for k in 1..n {
            let ideal = (2 * len * k + n).div_euclid(2 * n);
            let plain = (200 * k + n).div_euclid(2 * n);
            let percent = (0..=100)
                .min_by_key(|&p| ((along(len, p) - ideal).abs(), (p - plain).abs()))
                .unwrap_or(plain);
            let a = Anchor::side(side, u8::try_from(percent).unwrap_or(50));
            out.push((a, a.point(rect)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w_px: i32, h_px: i32) -> Rect {
        Rect::from_centre(Point::new(0, 0), w_px / 20, h_px / 20)
    }

    #[test]
    fn parse_and_display_round_trip() {
        for text in ["topLeft", "topRight", "bottomRight", "bottomLeft", "top", "right 0", "bottom 100", "left 25"] {
            let mut words = text.split(' ');
            let word = words.next().expect("word");
            let percent = words.next().map(|p| p.parse::<u32>().expect("number"));
            let a = Anchor::parse(word, percent).expect("parses");
            assert_eq!(a.to_string(), text);
        }
        assert_eq!(Anchor::parse("TOP", Some(50)).map(|a| a.to_string()).as_deref(), Some("top"));
        assert_eq!(Anchor::parse("top", Some(101)), None);
        assert_eq!(Anchor::parse("topLeft", Some(10)), None);
        assert_eq!(Anchor::parse("middle", None), None);
    }

    #[test]
    fn point_on_rectangle() {
        let r = rect(240, 140);
        assert_eq!(Anchor::parse("topLeft", None).map(|a| a.point(r)), Some(r.min));
        assert_eq!(Anchor::parse("bottom", None).map(|a| a.point(r)), Some(Point::new(0, 7)));
        assert_eq!(Anchor::parse("top", Some(25)).map(|a| a.point(r)), Some(Point::new(-6, -7)));
        assert_eq!(Anchor::parse("right", Some(0)).map(|a| a.point(r)), Some(Point::new(12, -7)));
    }

    #[test]
    fn snap_rule() {
        let count = |w, h| snap_points(rect(w, h)).len();
        // 240 x 140: corners, quarters on top and bottom, middles on the sides.
        assert_eq!(count(240, 140), 4 + 3 + 1 + 3 + 1);
        assert_eq!(count(200, 120), 4 + 4);
        assert_eq!(count(440, 140), 4 + 5 + 1 + 5 + 1);
        for (w, h) in [(240, 140), (200, 180), (440, 140), (1000, 360), (40, 40)] {
            let pts = snap_points(rect(w, h));
            for (i, (_, p)) in pts.iter().enumerate() {
                for (_, q) in &pts[i + 1..] {
                    assert_ne!(p, q, "{w}x{h}: two snap points at one place");
                }
            }
        }
    }
}
