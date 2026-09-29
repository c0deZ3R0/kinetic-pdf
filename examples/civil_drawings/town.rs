//! The township the drawings are of: its roads and the blocks between them,
//! the lots, the ground and the levels designed over it, and everything
//! built in the roads -- drainage, sewer, water, power, lights, trees,
//! signs -- all worked out once, so every sheet draws the same thing.
//!
//! x runs east and y north in metres from the site's south-west corner.

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::geom::*;

/// The site: what the drawings cover. Its height is set by the blocks.
pub const SITE_W: f64 = 1200.0;
/// How far the model runs past the site, for the roads beyond it.
pub const BEYOND: f64 = 30.0;
/// Lots stop short of the creek corridor.
pub const LOTS_EAST: f64 = 1050.0;
/// The map grid the coordinates are given in: the site's corner.
pub const EASTING: f64 = 312_000.0;
pub const NORTHING: f64 = 6_245_000.0;
pub const CROSSFALL: f64 = 0.03;
pub const BATTER: f64 = 4.0;

/// The natural surface: falling east to the creek, rolling a little.
pub fn ground(x: f64, y: f64) -> f64 {
    70.0 - 0.011 * x + 2.2 * (y / 160.0 + 0.5).sin() * (x / 310.0).cos() + 0.9 * (x / 57.0 + 1.3).sin() * (y / 71.0 - 0.7).cos()
        + 0.4 * ((x + y) / 33.0).sin()
        - 3.0 * (-((x - creek_x(y)) / 28.0).powi(2)).exp()
}

/// The finished surface the earthworks shape the land to, before the lots
/// are benched: the ground with its small rolls taken out.
pub fn shaped(x: f64, y: f64) -> f64 {
    70.0 - 0.011 * x + 2.2 * (y / 160.0 + 0.5).sin() * (x / 310.0).cos() + 0.45 * (x / 57.0 + 1.3).sin() * (y / 71.0 - 0.7).cos()
}

pub fn creek_x(y: f64) -> f64 {
    1175.0 + 18.0 * (y / 90.0).sin()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Kind {
    Local,
    Collector,
    Boulevard,
}

impl Kind {
    /// From the centreline to the lip of the kerb.
    pub fn half(self) -> f64 {
        match self {
            Kind::Boulevard => 8.5,
            Kind::Collector => 5.5,
            Kind::Local => 3.5,
        }
    }

    pub fn verge(self) -> f64 {
        match self {
            Kind::Boulevard => 4.0,
            Kind::Collector => 3.5,
            Kind::Local => 4.25,
        }
    }

    /// From the centreline to the road reserve's boundary.
    pub fn reserve(self) -> f64 {
        self.half() + self.verge()
    }

    /// The radius of a kerb return into it.
    pub fn corner(self) -> f64 {
        match self {
            Kind::Boulevard => 9.0,
            Kind::Collector => 7.5,
            Kind::Local => 6.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Boulevard => "Boulevard, 25.0 reserve",
            Kind::Collector => "Collector, 18.0 reserve",
            Kind::Local => "Local street, 15.5 reserve",
        }
    }

    /// Half the median, for the one road that has one.
    pub fn median(self) -> f64 {
        if self == Kind::Boulevard { 1.5 } else { 0.0 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    EastWest,
    NorthSouth,
}

pub struct Road {
    pub id: usize,
    pub name: &'static str,
    pub code: String,
    pub kind: Kind,
    pub axis: Axis,
    /// Where its centreline is across the other axis.
    pub at: f64,
    /// Where it runs along its axis, beyond the site too.
    pub ext: (f64, f64),
    /// The part inside the site, which its chainages are along.
    pub from: f64,
    pub to: f64,
    pub levels: Profile,
    /// Where the roads crossing or meeting it do, along it, and which.
    pub crossings: Vec<(f64, usize)>,
}

impl Road {
    pub fn length(&self) -> f64 {
        self.to - self.from
    }

    pub fn dir(&self) -> P {
        match self.axis {
            Axis::EastWest => (1.0, 0.0),
            Axis::NorthSouth => (0.0, 1.0),
        }
    }

    /// The centreline at a chainage, offset `off` to its left.
    pub fn point(&self, ch: f64, off: f64) -> P {
        match self.axis {
            Axis::EastWest => (self.from + ch, self.at + off),
            Axis::NorthSouth => (self.at - off, self.from + ch),
        }
    }

    pub fn ch_off(&self, p: P) -> (f64, f64) {
        match self.axis {
            Axis::EastWest => (p.0 - self.from, p.1 - self.at),
            Axis::NorthSouth => (p.1 - self.from, self.at - p.0),
        }
    }

    pub fn centreline(&self) -> Vec<P> {
        vec![self.point(0.0, 0.0), self.point(self.length(), 0.0)]
    }

    /// Its reserve, beyond the site too.
    pub fn reserve_box(&self) -> Bx {
        let r = self.kind.reserve();
        match self.axis {
            Axis::EastWest => (self.ext.0, self.at - r, self.ext.1, self.at + r),
            Axis::NorthSouth => (self.at - r, self.ext.0, self.at + r, self.ext.1),
        }
    }

    /// The finished surface across it at a chainage: the median raised, the
    /// carriageway falling to the kerbs, the verges rising to the boundary.
    pub fn surface(&self, ch: f64, off: f64) -> f64 {
        let crown = self.levels.level(ch.clamp(0.0, self.length()));
        let (half, median) = (self.kind.half(), self.kind.median());
        let a = off.abs();
        if a < median {
            return crown + 0.15;
        }
        let lip = crown - CROSSFALL * (half - median);
        if a <= half {
            crown - CROSSFALL * (a - median)
        } else if a <= half + 0.45 {
            lip + 0.125
        } else {
            lip + 0.125 + 0.025 * (a - half - 0.45)
        }
    }

    /// The level of its kerb's lip at a chainage.
    pub fn lip(&self, ch: f64) -> f64 {
        self.surface(ch, self.kind.half())
    }

    /// The name and chainage of a point on it, for a label or a table.
    pub fn place(&self, ch: f64) -> String {
        format!("{} CH {ch:.1}", self.short())
    }

    pub fn short(&self) -> String {
        self.name.replace(" Street", " St").replace(" Road", " Rd").replace(" Boulevard", " Bvd").replace(" Parade", " Pde").replace(" Way", " Wy")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Use {
    Lots,
    Park,
    School,
    Centre,
    /// The open space along the creek, and what's east of the lots.
    Corridor,
    Median,
}

impl Use {
    pub fn label(self) -> &'static str {
        match self {
            Use::Lots => "",
            Use::Park => "PUBLIC OPEN SPACE",
            Use::School => "SCHOOL SITE",
            Use::Centre => "LOCAL CENTRE",
            Use::Corridor => "CREEK CORRIDOR",
            Use::Median => "",
        }
    }
}

/// A block of land inside the kerbs, as the lip goes round it anticlockwise:
/// bottom, right, top and left, and the road along each, if any.
pub struct Island {
    pub id: usize,
    pub corners: Vec<Corner>,
    pub verges: Vec<f64>,
    pub sides: [Option<usize>; 4],
    pub kerb: Bx,
    pub property: Bx,
    pub use_: Use,
    pub lip: Vec<P>,
    pub back: Vec<P>,
    pub boundary: Vec<P>,
}

impl Island {
    /// The outline `f(verge)` in from the kerb along each road.
    pub fn ring(&self, f: impl Fn(f64) -> f64) -> Vec<P> {
        let d: Vec<f64> = self.verges.iter().map(|&v| if v > 0.0 { f(v) } else { 0.0 }).collect();
        rounded(&offset(&self.corners, &d, true), true)
    }

    pub fn has_verge(&self) -> bool {
        self.verges.iter().any(|&v| v > 0.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    South,
    East,
    North,
    West,
}

impl Side {
    /// Out of the lot, toward the road.
    pub fn out(self) -> P {
        match self {
            Side::South => (0.0, -1.0),
            Side::East => (1.0, 0.0),
            Side::North => (0.0, 1.0),
            Side::West => (-1.0, 0.0),
        }
    }
}

pub struct Lot {
    pub number: u32,
    pub bx: Bx,
    pub area: f64,
    /// The finished surface level of its pad.
    pub fsl: f64,
    pub island: usize,
    pub front: Side,
    pub stage: u32,
    /// The middle of its driveway, on the boundary.
    pub driveway: P,
}

impl Lot {
    pub fn frontage(&self) -> f64 {
        match self.front {
            Side::South | Side::North => self.bx.2 - self.bx.0,
            _ => self.bx.3 - self.bx.1,
        }
    }

    pub fn depth(&self) -> f64 {
        match self.front {
            Side::South | Side::North => self.bx.3 - self.bx.1,
            _ => self.bx.2 - self.bx.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PitKind {
    /// A kerb inlet with a lintel this long.
    Kerb(f64),
    Junction,
    Headwall,
    Outlet,
}

impl PitKind {
    pub fn name(self) -> String {
        match self {
            PitKind::Kerb(l) => format!("Kerb inlet, {l:.1} lintel"),
            PitKind::Junction => "Junction pit, 1200 sq".into(),
            PitKind::Headwall => "Headwall".into(),
            PitKind::Outlet => "Basin outlet".into(),
        }
    }
}

pub struct Pit {
    pub name: String,
    pub kind: PitKind,
    pub at: P,
    /// The way the road runs past it.
    pub dir: P,
    /// Away from the road, for its label.
    pub out: P,
    pub road: Option<usize>,
    pub ch: f64,
    pub surface: f64,
    pub invert: f64,
    /// Hectares draining to it.
    pub catchment: f64,
    pub line: usize,
}

pub struct Pipe {
    pub from: usize,
    pub to: usize,
    pub dia: f64,
    pub length: f64,
    pub grade: f64,
    pub up: f64,
    pub down: f64,
    /// Litres a second in the 20% and 1% chance storms.
    pub q5: f64,
    pub q100: f64,
    pub capacity: f64,
}

/// A run of pits one after another, top to bottom.
pub struct Line {
    pub name: String,
    pub road: usize,
    pub pits: Vec<usize>,
}

pub struct Manhole {
    pub name: String,
    pub at: P,
    pub surface: f64,
    pub invert: f64,
}

pub struct Sewer {
    pub name: String,
    pub island: usize,
    pub manholes: Vec<Manhole>,
    /// The road its bottom manhole connects to the sewer in.
    pub outlet: Option<usize>,
}

pub struct Wall {
    pub a: P,
    pub b: P,
    pub height: f64,
    pub top: f64,
}

pub struct Basin {
    pub name: String,
    pub corners: Vec<Corner>,
    pub top: f64,
    pub floor: f64,
    pub twl: f64,
    pub spillway: f64,
}

impl Basin {
    pub fn reach(&self) -> f64 {
        (self.top - self.floor) * BATTER
    }

    pub fn ring(&self, down: f64) -> Vec<P> {
        let d = down * BATTER;
        rounded(&offset(&self.corners, &[d; 4], true), true)
    }

    pub fn bx(&self) -> Bx {
        bounds(&rounded(&self.corners, true))
    }

    /// The design surface in it: the floor, the batters, or the bank.
    pub fn surface(&self, p: P) -> Option<f64> {
        let (x0, y0, x1, y1) = (self.corners[0].0, self.corners[0].1, self.corners[2].0, self.corners[2].1);
        if !in_box(p, (x0 - 2.0, y0 - 2.0, x1 + 2.0, y1 + 2.0)) {
            return None;
        }
        // Distance in from the top of bank, near enough for a rounded box.
        let d = (p.0 - x0).min(x1 - p.0).min(p.1 - y0).min(y1 - p.1);
        if d < 0.0 {
            return Some(self.top);
        }
        Some((self.top - d / BATTER).max(self.floor))
    }
}

pub enum SignKind {
    Stop,
    GiveWay,
    StreetName,
    Crossing,
    Hump,
    BusZone,
}

impl SignKind {
    pub fn name(&self) -> &'static str {
        match self {
            SignKind::Stop => "Stop",
            SignKind::GiveWay => "Give way",
            SignKind::StreetName => "Street name blades",
            SignKind::Crossing => "Pedestrian crossing",
            SignKind::Hump => "Road hump ahead",
            SignKind::BusZone => "Bus zone",
        }
    }
}

pub struct Sign {
    pub id: String,
    pub kind: SignKind,
    pub at: P,
    pub road: usize,
    pub ch: f64,
}

/// A painted line: points, and whether it's broken.
pub struct Marking {
    pub pts: Vec<P>,
    pub broken: bool,
    pub width: f64,
}

pub struct Town {
    pub height: f64,
    pub roads: Vec<Road>,
    pub islands: Vec<Island>,
    pub lots: Vec<Lot>,
    pub pits: Vec<Pit>,
    pub pipes: Vec<Pipe>,
    pub lines: Vec<Line>,
    pub sewers: Vec<Sewer>,
    pub walls: Vec<Wall>,
    pub basins: Vec<Basin>,
    pub lights: Vec<(P, P)>,
    pub trees: Vec<(P, usize)>,
    pub hydrants: Vec<P>,
    pub valves: Vec<P>,
    pub kiosks: Vec<P>,
    pub signs: Vec<Sign>,
    pub markings: Vec<Marking>,
    pub humps: Vec<(P, P)>,
    lot_index: HashMap<(i32, i32), Vec<usize>>,
}

/// Runoff coefficient, and rainfall intensities in millimetres an hour for
/// a five-minute storm of 20% and 1% chance a year.
pub const RUNOFF: f64 = 0.75;
pub const I5: f64 = 118.0;
pub const I100: f64 = 226.0;
const PIPE_SIZES: [f64; 13] = [0.375, 0.45, 0.525, 0.6, 0.675, 0.75, 0.825, 0.9, 1.05, 1.2, 1.35, 1.5, 1.8];

/// Full-bore capacity in litres a second, by Manning's equation.
pub fn capacity(dia: f64, grade: f64) -> f64 {
    let area = PI * dia * dia / 4.0;
    area * (dia / 4.0).powf(2.0 / 3.0) * grade.sqrt() / 0.013 * 1000.0
}

/// The street trees, by street: what's planted along which.
pub const SPECIES: [(&str, &str, [f64; 3]); 5] = [
    ("WG", "Water gum, 45 L", [0.15, 0.55, 0.3]),
    ("BB", "Brush box, 100 L", [0.35, 0.6, 0.15]),
    ("CM", "Crepe myrtle, 45 L", [0.75, 0.35, 0.6]),
    ("OP", "Ornamental pear, 45 L", [0.3, 0.65, 0.6]),
    ("JC", "Jacaranda, 100 L", [0.5, 0.4, 0.8]),
];

impl Town {
    pub fn new() -> Town {
        // The east-west streets, south to north: each a lot depth either
        // side of the one before.
        let ew: [(&str, Kind, bool, bool); 11] = [
            ("Egret Street", Kind::Local, true, true),
            ("Ibis Street", Kind::Local, false, false),
            ("Curlew Road", Kind::Local, true, false),
            ("Tern Street", Kind::Local, false, false),
            ("Wren Street", Kind::Local, false, false),
            ("Heron Ridge Boulevard", Kind::Boulevard, true, true),
            ("Finch Street", Kind::Local, false, false),
            ("Robin Street", Kind::Local, false, false),
            ("Plover Road", Kind::Local, true, false),
            ("Swift Street", Kind::Local, false, false),
            ("Lark Street", Kind::Local, true, true),
        ];
        let depths = [30.0, 28.0, 31.0, 30.0, 29.0, 32.0, 30.0, 28.0, 31.0, 30.0];
        let mut ys = vec![32.0 + ew[0].1.reserve()];
        for k in 1..ew.len() {
            let y = ys[k - 1] + ew[k - 1].1.reserve() + 2.0 * depths[k - 1] + ew[k].1.reserve();
            ys.push(y);
        }
        let height = ys[10] + ew[10].1.reserve() + 32.0;
        let (south, north) = (-BEYOND, height + BEYOND);

        let mut roads: Vec<Road> = Vec::new();
        for (k, &(name, kind, west, east)) in ew.iter().enumerate() {
            let ext = (if west { -BEYOND } else { 40.0 }, if east { SITE_W + 60.0 } else { 1060.0 });
            roads.push(Road {
                id: k,
                name,
                code: format!("E{k}"),
                kind,
                axis: Axis::EastWest,
                at: ys[k],
                ext,
                from: ext.0.max(0.0),
                to: ext.1.min(SITE_W),
                levels: Profile { ips: Vec::new() },
                crossings: Vec::new(),
            });
        }
        // North-south streets, and which east-west ones each runs between.
        let ns: [(&str, Kind, f64, Option<(usize, usize)>); 9] = [
            ("Sandpiper Way", Kind::Local, 40.0, Some((0, 10))),
            ("Osprey Street", Kind::Local, 160.0, Some((0, 5))),
            ("Magpie Street", Kind::Local, 300.0, Some((5, 10))),
            ("Falcon Street", Kind::Local, 430.0, Some((0, 3))),
            ("Owl Street", Kind::Local, 430.0, Some((7, 10))),
            ("Pelican Road", Kind::Collector, 600.0, None),
            ("Lorikeet Street", Kind::Local, 770.0, Some((0, 5))),
            ("Harrier Street", Kind::Local, 900.0, Some((3, 8))),
            ("Kingfisher Parade", Kind::Local, 1060.0, Some((0, 10))),
        ];
        for (j, &(name, kind, x, span)) in ns.iter().enumerate() {
            let ext = match span {
                Some((a, b)) => (ys[a], ys[b]),
                None => (south, north),
            };
            roads.push(Road {
                id: roads.len(),
                name,
                code: format!("N{j}"),
                kind,
                axis: Axis::NorthSouth,
                at: x,
                ext,
                from: ext.0.max(0.0),
                to: ext.1.min(height),
                levels: Profile { ips: Vec::new() },
                crossings: Vec::new(),
            });
        }
        // Where they meet.
        for a in 0..roads.len() {
            for b in 0..roads.len() {
                if roads[a].axis == Axis::EastWest && roads[b].axis == Axis::NorthSouth {
                    let (x, y) = (roads[b].at, roads[a].at);
                    let eps = 0.01;
                    if roads[a].ext.0 <= x + eps && x - eps <= roads[a].ext.1 && roads[b].ext.0 <= y + eps && y - eps <= roads[b].ext.1 {
                        roads[a].crossings.push((x, b));
                        roads[b].crossings.push((y, a));
                    }
                }
            }
        }
        for road in &mut roads {
            road.crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
            road.levels = profile(road);
        }

        let islands = islands(&roads, &ys, height);
        let mut town = Town {
            height,
            roads,
            islands,
            lots: Vec::new(),
            pits: Vec::new(),
            pipes: Vec::new(),
            lines: Vec::new(),
            sewers: Vec::new(),
            walls: Vec::new(),
            basins: Vec::new(),
            lights: Vec::new(),
            trees: Vec::new(),
            hydrants: Vec::new(),
            valves: Vec::new(),
            kiosks: Vec::new(),
            signs: Vec::new(),
            markings: Vec::new(),
            humps: Vec::new(),
            lot_index: HashMap::new(),
        };
        town.subdivide();
        town.basins();
        town.drainage();
        town.sewers();
        town.walls();
        town.services();
        town.signs();
        town
    }

    pub fn site(&self) -> Bx {
        (0.0, 0.0, SITE_W, self.height)
    }

    // -----------------------------------------------------------------------
    // Lots

    fn subdivide(&mut self) {
        let widths = [15.0, 12.5, 16.0, 14.0, 18.0, 15.0, 13.5];
        let mut number = 101;
        let site = (0.0, 0.0, LOTS_EAST, self.height);
        for isl in &self.islands {
            if isl.use_ != Use::Lots {
                continue;
            }
            let p = isl.property;
            let pb = (p.0.max(site.0), p.1.max(site.1), p.2.min(site.2), p.3.min(site.3));
            let (w, h) = (pb.2 - pb.0, pb.3 - pb.1);
            if w < 10.0 || h < 10.0 {
                continue;
            }
            let [bottom, right, top, left] = isl.sides.map(|s| s.is_some());
            // The rows: the box each is in and the side it faces.
            let mut rows: Vec<(Bx, Side)> = Vec::new();
            if w >= h {
                let mid = (pb.1 + pb.3) / 2.0;
                match (bottom, top) {
                    (true, true) if h >= 48.0 => {
                        rows.push(((pb.0, pb.1, pb.2, mid), Side::South));
                        rows.push(((pb.0, mid, pb.2, pb.3), Side::North));
                    }
                    (true, _) => rows.push((pb, Side::South)),
                    (_, true) => rows.push((pb, Side::North)),
                    _ => {}
                }
            } else {
                let mid = (pb.0 + pb.2) / 2.0;
                match (left, right) {
                    (true, true) if w >= 48.0 => {
                        rows.push(((pb.0, pb.1, mid, pb.3), Side::West));
                        rows.push(((mid, pb.1, pb.2, pb.3), Side::East));
                    }
                    (_, true) => rows.push((pb, Side::East)),
                    (true, _) => rows.push((pb, Side::West)),
                    _ => {}
                }
            }
            let f = widths[isl.id % widths.len()];
            let ring = &isl.boundary;
            for (r, side) in rows {
                let along_x = matches!(side, Side::South | Side::North);
                let len = if along_x { r.2 - r.0 } else { r.3 - r.1 };
                let n = ((len / f).floor() as usize).max(1);
                let step = len / n as f64;
                for i in 0..n {
                    let bx = if along_x {
                        (r.0 + step * i as f64, r.1, r.0 + step * (i + 1) as f64, r.3)
                    } else {
                        (r.0, r.1 + step * i as f64, r.2, r.1 + step * (i + 1) as f64)
                    };
                    // The corner lots lose the rounding of the boundary.
                    let s = 0.01;
                    let whole = rect_pts((bx.0 + s, bx.1 + s, bx.2 - s, bx.3 - s)).iter().all(|&q| inside(ring, q));
                    let mut area = (bx.2 - bx.0) * (bx.3 - bx.1);
                    if !whole {
                        let r = isl.corners.iter().map(|c| c.2).fold(0.0, f64::max);
                        area -= r * r * (1.0 - PI / 4.0);
                    }
                    let c = centre(bx);
                    let fsl = (shaped(c.0, c.1) / 0.05).round() * 0.05 + 0.1;
                    let front = mul(add(side.out(), (1.0, 1.0)), 0.5);
                    let edge = (bx.0 + (bx.2 - bx.0) * front.0, bx.1 + (bx.3 - bx.1) * front.1);
                    let along = if along_x { ((bx.2 - bx.0) * 0.25, 0.0) } else { (0.0, (bx.3 - bx.1) * 0.25) };
                    let driveway = if along_x { (bx.0 + along.0, edge.1) } else { (edge.0, bx.1 + along.1) };
                    let stage = 1 + (c.0 > 600.0) as u32 + 2 * ((c.1 > self.height / 3.0) as u32 + (c.1 > self.height * 2.0 / 3.0) as u32);
                    self.lots.push(Lot { number, bx, area, fsl, island: isl.id, front: side, stage, driveway });
                    number += 1;
                }
            }
        }
        for (i, lot) in self.lots.iter().enumerate() {
            let c = centre(lot.bx);
            self.lot_index.entry(((c.0 / 50.0) as i32, (c.1 / 50.0) as i32)).or_default().push(i);
        }
    }

    pub fn lot_at(&self, p: P) -> Option<&Lot> {
        let (i, j) = ((p.0 / 50.0) as i32, (p.1 / 50.0) as i32);
        for di in -1..=1 {
            for dj in -1..=1 {
                if let Some(list) = self.lot_index.get(&(i + di, j + dj)) {
                    if let Some(&k) = list.iter().find(|&&k| in_box(p, self.lots[k].bx)) {
                        return Some(&self.lots[k]);
                    }
                }
            }
        }
        None
    }

    pub fn road_at(&self, p: P) -> Option<&Road> {
        self.roads.iter().filter(|r| in_box(p, r.reserve_box())).min_by(|a, b| a.ch_off(p).1.abs().total_cmp(&b.ch_off(p).1.abs()))
    }

    /// The finished surface: the lot pads, the roads, the basins, and the
    /// shaped ground elsewhere. None where nothing is done: the corridor.
    pub fn design(&self, p: P) -> Option<f64> {
        if let Some(level) = self.basins.iter().find_map(|b| b.surface(p)) {
            return Some(level);
        }
        if let Some(lot) = self.lot_at(p) {
            return Some(lot.fsl);
        }
        if let Some(road) = self.road_at(p) {
            let (ch, off) = road.ch_off(p);
            return Some(road.surface(ch, off));
        }
        let isl = self.islands.iter().find(|i| in_box(p, i.property))?;
        match isl.use_ {
            Use::Corridor => None,
            _ if p.0 > LOTS_EAST => None,
            _ => Some(shaped(p.0, p.1)),
        }
    }

    // -----------------------------------------------------------------------
    // Basins, drainage

    fn basins(&mut self) {
        let spans = [(70.0, 190.0), (250.0, 370.0), (470.0, 590.0), (650.0, 770.0)];
        for (i, &(y0, y1)) in spans.iter().enumerate() {
            let (x0, x1) = (1084.0, 1138.0);
            let top = ground((x0 + x1) / 2.0, (y0 + y1) / 2.0).min(ground(x0, y0)).min(ground(x0, y1)) + 0.2;
            let floor = top - 1.5;
            self.basins.push(Basin {
                name: format!("B{}", i + 1),
                corners: vec![(x0, y0, 10.0), (x1, y0, 10.0), (x1, y1, 10.0), (x0, y1, 10.0)],
                top,
                floor,
                twl: floor + 1.0,
                spillway: floor + 1.2,
            });
        }
    }

    fn drainage(&mut self) {
        let mut pits: Vec<Pit> = Vec::new();
        let mut lines: Vec<Line> = Vec::new();
        let mut links: Vec<(usize, usize)> = Vec::new();

        let pit = |pits: &mut Vec<Pit>, name: String, kind: PitKind, road: &Road, ch: f64, off: f64, line: usize, area: f64| -> usize {
            let at = road.point(ch, off);
            let out = mul(left(road.dir()), off.signum());
            pits.push(Pit { name, kind, at, dir: road.dir(), out, road: Some(road.id), ch, surface: road.lip(ch), invert: 0.0, catchment: area, line });
            pits.len() - 1
        };

        // East-west trunks along the south kerb, the north kerb's pits
        // crossing to them, flowing east to the corridor.
        let mut trunk_pits: Vec<Vec<(f64, usize)>> = vec![Vec::new(); self.roads.len()];
        for road in self.roads.iter().filter(|r| r.axis == Axis::EastWest) {
            let x_end = road.to.min(1060.0) - 12.0;
            let x_start = road.from + 14.0;
            let mut xs: Vec<f64> = road
                .crossings
                .iter()
                .map(|&(x, j)| x - self.roads[j].kind.reserve() - self.roads[j].kind.corner() - 1.0)
                .filter(|&x| x > x_start && x < x_end)
                .collect();
            xs.push(x_start);
            xs.push(x_end);
            xs.sort_by(f64::total_cmp);
            let mut filled = vec![xs[0]];
            for w in xs.windows(2) {
                let n = ((w[1] - w[0]) / 72.0).ceil().max(1.0) as usize;
                for k in 1..=n {
                    filled.push(w[0] + (w[1] - w[0]) * k as f64 / n as f64);
                }
            }
            filled.dedup_by(|a, b| (*a - *b).abs() < 8.0);
            let line = lines.len();
            let mut trunk = Vec::new();
            let off = road.kind.half() + 0.6;
            for (n, &x) in filled.iter().enumerate() {
                let ch = x - road.from;
                let spacing = if n == 0 { 40.0 } else { x - filled[n - 1] };
                let area = spacing * (road.kind.reserve() + 30.0) / 10_000.0;
                let lintel = if n + 1 == filled.len() { 3.0 } else if road.kind == Kind::Boulevard { 2.4 } else { 1.8 };
                let north = pit(&mut pits, format!("{}/{}A", road.code, n + 1), PitKind::Kerb(lintel), road, ch, off, line, area);
                let south = pit(&mut pits, format!("{}/{}", road.code, n + 1), PitKind::Kerb(lintel), road, ch, -off, line, area);
                links.push((north, south));
                if let Some(&last) = trunk.last() {
                    links.push((last, south));
                }
                trunk.push(south);
                trunk_pits[road.id].push((x, south));
            }
            // Out to the corridor, a headwall into the swale.
            let hw_at = (1074.0, road.at - road.kind.reserve() - 7.0);
            pits.push(Pit {
                name: format!("{}/HW", road.code),
                kind: PitKind::Headwall,
                at: hw_at,
                dir: (0.0, 1.0),
                out: (1.0, 0.0),
                road: None,
                ch: 0.0,
                surface: ground(hw_at.0, hw_at.1),
                invert: 0.0,
                catchment: 0.0,
                line,
            });
            let hw = pits.len() - 1;
            links.push((*trunk.last().expect("a pit"), hw));
            trunk.push(hw);
            lines.push(Line { name: road.code.clone(), road: road.id, pits: trunk });
        }

        // North-south lines, each stretch between crossings flowing to the
        // lower of its ends that meets a trunk.
        let mut ns_order = Vec::new();
        for road in self.roads.iter().filter(|r| r.axis == Axis::NorthSouth && r.at < 1060.0) {
            let mut stops: Vec<f64> = road.crossings.iter().map(|c| c.0).filter(|&y| y > road.from && y < road.to).collect();
            stops.insert(0, road.from);
            stops.push(road.to);
            stops.dedup_by(|a, b| (*a - *b).abs() < 1.0);
            for (s, w) in stops.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                let meets = |y: f64| road.crossings.iter().find(|c| (c.0 - y).abs() < 1.0).map(|c| c.1);
                let (lo, hi) = (road.levels.level(a - road.from), road.levels.level(b - road.from));
                let down_at_b = match (meets(a), meets(b)) {
                    (Some(_), Some(_)) => hi <= lo,
                    (None, Some(_)) => true,
                    (Some(_), None) => false,
                    (None, None) => continue,
                };
                let (end, ew) = if down_at_b { (b, meets(b).expect("a trunk")) } else { (a, meets(a).expect("a trunk")) };
                let clear = self.roads[ew].kind.reserve() + road.kind.corner() + 1.0;
                let (y0, y1) = if down_at_b { (a + clear, b - clear) } else { (b - clear, a + clear) };
                if (y1 - y0).abs() < 20.0 {
                    continue;
                }
                let n = ((y1 - y0).abs() / 70.0).ceil().max(1.0) as usize;
                let line = lines.len();
                let code = format!("{}{}", road.code, (b'a' + s as u8) as char);
                let mut run = Vec::new();
                let off = road.kind.half() + 0.6;
                for k in 0..=n {
                    let y = y0 + (y1 - y0) * k as f64 / n as f64;
                    let ch = y - road.from;
                    let area = (y1 - y0).abs() / n as f64 * (road.kind.reserve() + 30.0) / 10_000.0;
                    let west = pit(&mut pits, format!("{code}/{}A", k + 1), PitKind::Kerb(1.8), road, ch, off, line, area);
                    let east = pit(&mut pits, format!("{code}/{}", k + 1), PitKind::Kerb(1.8), road, ch, -off, line, area);
                    ns_order.push(west);
                    ns_order.push(east);
                    links.push((west, east));
                    if let Some(&last) = run.last() {
                        links.push((last, east));
                    }
                    run.push(east);
                }
                // Into the trunk at the pit nearest the junction.
                let target = trunk_pits[ew].iter().min_by(|p, q| (p.0 - road.at).abs().total_cmp(&(q.0 - road.at).abs())).map(|p| p.1).expect("a trunk pit");
                links.push((*run.last().expect("a pit"), target));
                let _ = end;
                lines.push(Line { name: code, road: road.id, pits: run });
            }
        }

        // Each basin's outlet to the creek.
        for basin in &self.basins {
            let (x1, yc) = (basin.corners[1].0, (basin.corners[0].1 + basin.corners[2].1) / 2.0);
            let os = (x1 - basin.reach() - 0.5, yc);
            pits.push(Pit { name: format!("{}/OS", basin.name), kind: PitKind::Outlet, at: os, dir: (0.0, 1.0), out: (1.0, 0.0), road: None, ch: 0.0, surface: basin.floor, invert: basin.floor - 0.15, catchment: 0.0, line: lines.len() });
            let hw_at = (creek_x(yc) - 11.0, yc - 4.0);
            pits.push(Pit { name: format!("{}/HW", basin.name), kind: PitKind::Headwall, at: hw_at, dir: (0.0, 1.0), out: (1.0, 0.0), road: None, ch: 0.0, surface: ground(hw_at.0, hw_at.1), invert: 0.0, catchment: 0.0, line: lines.len() });
            links.push((pits.len() - 2, pits.len() - 1));
            lines.push(Line { name: format!("{} outlet", basin.name), road: 0, pits: vec![pits.len() - 2, pits.len() - 1] });
        }

        let mut pipes: Vec<Pipe> = links
            .iter()
            .map(|&(from, to)| Pipe { from, to, dia: 0.0, length: dist(pits[from].at, pits[to].at), grade: 0.0, up: 0.0, down: 0.0, q5: 0.0, q100: 0.0, capacity: 0.0 })
            .collect();

        // Every pit after all that drain to it: the north-south lines, then
        // the trunks in the order their pits were made, the outlets last.
        let mut order: Vec<usize> = ns_order.clone();
        let in_ns: std::collections::HashSet<usize> = ns_order.into_iter().collect();
        order.extend((0..pits.len()).filter(|i| !in_ns.contains(i)));
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); pits.len()];
        let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); pits.len()];
        for (k, p) in pipes.iter().enumerate() {
            outgoing[p.from].push(k);
            incoming[p.to].push(k);
        }
        let q = |ha: f64, i: f64| RUNOFF * i * ha / 0.36;
        let mut total = vec![0.0; pits.len()];
        for &i in &order {
            let lowest_in = incoming[i].iter().map(|&k| pipes[k].down).fold(f64::INFINITY, f64::min);
            total[i] = pits[i].catchment + incoming[i].iter().map(|&k| total[pipes[k].from]).sum::<f64>();
            let biggest_in = incoming[i].iter().map(|&k| pipes[k].dia).fold(0.0, f64::max);
            pits[i].invert = match pits[i].kind {
                PitKind::Headwall => lowest_in,
                PitKind::Outlet => pits[i].invert,
                _ => (pits[i].surface - 1.0 - biggest_in).min(lowest_in - 0.03),
            };
            for &k in &outgoing[i] {
                let to = pipes[k].to;
                let fall = pits[i].surface - pits[to].surface;
                let pipe = &mut pipes[k];
                pipe.grade = if pits[to].kind == PitKind::Headwall { 0.005 } else { (fall / pipe.length).clamp(0.005, 0.05) };
                pipe.up = pits[i].invert;
                pipe.down = pipe.up - pipe.length * pipe.grade;
                pipe.q5 = q(total[i], I5);
                pipe.q100 = q(total[i], I100);
                pipe.dia = PIPE_SIZES.iter().copied().find(|&d| capacity(d, pipe.grade) >= pipe.q5).unwrap_or(1.8);
                pipe.dia = pipe.dia.max(biggest_in);
                pipe.capacity = capacity(pipe.dia, pipe.grade);
            }
        }
        self.pits = pits;
        self.pipes = pipes;
        self.lines = lines;
    }

    pub fn pipe_from(&self, pit: usize) -> Option<&Pipe> {
        self.pipes.iter().find(|p| p.from == pit)
    }

    // -----------------------------------------------------------------------
    // Sewer, walls, services, signs

    fn sewers(&mut self) {
        let mut n = 1;
        for isl in self.islands.iter().filter(|i| i.use_ == Use::Lots) {
            let lots: Vec<&Lot> = self.lots.iter().filter(|l| l.island == isl.id).collect();
            if lots.is_empty() {
                continue;
            }
            let b = lots.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, l| (b.0.min(l.bx.0), b.1.min(l.bx.1), b.2.max(l.bx.2), b.3.max(l.bx.3)));
            let fronts: Vec<Side> = lots.iter().map(|l| l.front).collect();
            let has = |s: Side| fronts.contains(&s);
            // Along the backs of the lots.
            let (a, z) = if has(Side::South) && has(Side::North) {
                let y = lots.iter().filter(|l| l.front == Side::South).map(|l| l.bx.3).fold(f64::MIN, f64::max) - 1.5;
                ((b.0 + 2.0, y), (b.2 - 2.0, y))
            } else if has(Side::South) {
                ((b.0 + 2.0, b.3 - 1.5), (b.2 - 2.0, b.3 - 1.5))
            } else if has(Side::North) {
                ((b.0 + 2.0, b.1 + 1.5), (b.2 - 2.0, b.1 + 1.5))
            } else if has(Side::West) && has(Side::East) {
                let x = lots.iter().filter(|l| l.front == Side::West).map(|l| l.bx.2).fold(f64::MIN, f64::max) - 1.5;
                ((x, b.1 + 2.0), (x, b.3 - 2.0))
            } else if has(Side::East) {
                ((b.0 + 1.5, b.1 + 2.0), (b.0 + 1.5, b.3 - 2.0))
            } else {
                ((b.2 - 1.5, b.1 + 2.0), (b.2 - 1.5, b.3 - 2.0))
            };
            let (up, down) = if shaped(a.0, a.1) >= shaped(z.0, z.1) { (a, z) } else { (z, a) };
            let length = dist(up, down);
            let count = (length / 60.0).ceil().max(1.0) as usize;
            let fall = shaped(up.0, up.1) - shaped(down.0, down.1);
            let grade = (fall / length).max(1.0 / 150.0);
            let mut manholes = Vec::new();
            let mut invert = shaped(up.0, up.1) + 0.1 - 1.2;
            for k in 0..=count {
                let at = add(up, mul(sub(down, up), k as f64 / count as f64));
                if k > 0 {
                    invert -= grade * length / count as f64 + 0.02;
                }
                let surface = self.lot_at(at).map_or(shaped(at.0, at.1), |l| l.fsl);
                manholes.push(Manhole { name: format!("S{n}/{}", k + 1), at, surface, invert });
            }
            let outlet = isl.sides.iter().flatten().copied().min_by(|&p, &q| {
                let d = |r: usize| {
                    let road = &self.roads[r];
                    let (_, off) = road.ch_off(down);
                    off.abs()
                };
                d(p).total_cmp(&d(q))
            });
            self.sewers.push(Sewer { name: format!("S{n}"), island: isl.id, manholes, outlet });
            n += 1;
        }
    }

    fn walls(&mut self) {
        let lots = &self.lots;
        for i in 0..lots.len() {
            for j in i + 1..lots.len() {
                let (a, b) = (&lots[i].bx, &lots[j].bx);
                let dh = (lots[i].fsl - lots[j].fsl).abs();
                if dh < 0.25 {
                    continue;
                }
                let shared = if (a.2 - b.0).abs() < 0.01 || (b.2 - a.0).abs() < 0.01 {
                    let x = if (a.2 - b.0).abs() < 0.01 { a.2 } else { a.0 };
                    let (y0, y1) = (a.1.max(b.1), a.3.min(b.3));
                    (y1 - y0 > 1.0).then_some(((x, y0), (x, y1)))
                } else if (a.3 - b.1).abs() < 0.01 || (b.3 - a.1).abs() < 0.01 {
                    let y = if (a.3 - b.1).abs() < 0.01 { a.3 } else { a.1 };
                    let (x0, x1) = (a.0.max(b.0), a.2.min(b.2));
                    (x1 - x0 > 1.0).then_some(((x0, y), (x1, y)))
                } else {
                    None
                };
                if let Some((p, q)) = shared {
                    self.walls.push(Wall { a: p, b: q, height: dh, top: lots[i].fsl.max(lots[j].fsl) });
                }
            }
        }
    }

    fn services(&mut self) {
        let site = grow(self.site(), -1.0);
        let lots_east = (1.0, 1.0, LOTS_EAST, self.height - 1.0);
        let mut n_species = 0;
        for isl in self.islands.iter().filter(|i| i.has_verge() && i.use_ != Use::Median) {
            for (p, u) in stations(&closed(&isl.ring(|_| 0.8)), 8.0, 34.0) {
                if in_box(p, site) {
                    self.lights.push((p, mul(left(u), -1.0)));
                }
            }
            for (p, _) in stations(&closed(&isl.ring(|v| v - 2.3)), 25.0, 80.0) {
                if in_box(p, site) {
                    self.hydrants.push(p);
                }
            }
            for (p, _) in stations(&closed(&isl.ring(|v| v - 2.3)), 5.0, 160.0) {
                if in_box(p, site) {
                    self.valves.push(p);
                }
            }
            for (p, _) in stations(&closed(&isl.ring(|v| v - 0.3)), 60.0, 190.0) {
                if in_box(p, lots_east) && isl.use_ == Use::Lots {
                    self.kiosks.push(p);
                }
            }
        }
        let driveways: Vec<P> = self.lots.iter().map(|l| l.driveway).collect();
        for isl in self.islands.iter().filter(|i| i.has_verge() && i.use_ != Use::Median) {
            for (p, _) in stations(&closed(&isl.ring(|_| 1.7)), 3.0, 11.0) {
                let clear = self.lights.iter().all(|(l, _)| dist(*l, p) > 4.0)
                    && self.pits.iter().all(|k| dist(k.at, p) > 3.0)
                    && driveways.iter().all(|d| dist(*d, p) > 5.5);
                if in_box(p, site) && clear {
                    let road = self.road_at(p).map_or(0, |r| r.id);
                    self.trees.push((p, road % SPECIES.len()));
                    n_species += 1;
                }
            }
        }
        let _ = n_species;
    }

    fn signs(&mut self) {
        let mut n = 1;
        let mut put = |signs: &mut Vec<Sign>, kind: SignKind, at: P, road: &Road| {
            let (ch, _) = road.ch_off(at);
            signs.push(Sign { id: format!("SN{n:03}"), kind, at, road: road.id, ch });
            n += 1;
        };
        let mut signs = Vec::new();
        for ew in self.roads.iter().filter(|r| r.axis == Axis::EastWest) {
            for &(x, j) in &ew.crossings {
                let ns = &self.roads[j];
                let (y, minor_is_ns) = (ew.at, ns.kind <= ew.kind);
                let (major, minor) = if minor_is_ns { (ew, ns) } else { (ns, ew) };
                let centre_pt = (x, y);
                if !in_box(centre_pt, self.site()) {
                    continue;
                }
                let kind = |major: &Road| if major.kind == Kind::Local { SignKind::GiveWay } else { SignKind::Stop };
                // Each approach along the minor road: hold line across its
                // left lane, and the sign on the left kerb.
                for dirn in [-1.0f64, 1.0] {
                    let (mch, _) = minor.ch_off(centre_pt);
                    let ch = mch + dirn * (major.kind.reserve() - major.kind.verge() + 1.2);
                    if ch < 0.5 || ch > minor.length() - 0.5 {
                        continue;
                    }
                    // Driving toward the junction, on the left.
                    let heading = -dirn;
                    let lane = heading * minor.kind.half();
                    let (a, b) = (minor.point(ch, 0.0), minor.point(ch, lane));
                    self.markings.push(Marking { pts: vec![a, b], broken: minor.kind > Kind::Local || major.kind == Kind::Local, width: 0.3 });
                    let sign_at = minor.point(ch + dirn * 2.0, heading * (minor.kind.half() + 0.9));
                    put(&mut signs, kind(major), sign_at, minor);
                }
                put(&mut signs, SignKind::StreetName, (x + ns.kind.reserve() - 1.0, y + ew.kind.reserve() - 1.0), ew);
            }
            // Humps midway along long local stretches.
            if ew.kind == Kind::Local {
                let mut xs: Vec<f64> = ew.crossings.iter().map(|c| c.0).collect();
                xs.insert(0, ew.from);
                xs.push(ew.to.min(1060.0));
                for w in xs.windows(2) {
                    if w[1] - w[0] > 160.0 {
                        let mid = (w[0] + w[1]) / 2.0;
                        self.humps.push(((mid, ew.at), (1.0, 0.0)));
                        put(&mut signs, SignKind::Hump, (mid - 40.0, ew.at - ew.kind.half() - 0.9), ew);
                    }
                }
            }
        }
        // The boulevard's lanes, the collector's centreline, crossings and
        // bus zones.
        for road in &self.roads {
            let (c0, c1) = (road.point(0.0, 0.0), road.point(road.length(), 0.0));
            if road.kind == Kind::Boulevard {
                for off in [-5.0, 5.0] {
                    self.markings.push(Marking { pts: vec![add(c0, mul(left(road.dir()), off)), add(c1, mul(left(road.dir()), off))], broken: true, width: 0.1 });
                }
                for (x, name) in [(365.0, "park"), (515.0, "park")] {
                    let _ = name;
                    put(&mut signs, SignKind::Crossing, (x, road.at - road.kind.half() - 0.9), road);
                }
                for x in [250.0, 820.0] {
                    put(&mut signs, SignKind::BusZone, (x, road.at + road.kind.half() + 0.9), road);
                }
            }
            if road.kind == Kind::Collector {
                self.markings.push(Marking { pts: vec![c0, c1], broken: true, width: 0.1 });
            }
        }
        self.signs = signs;
    }

    // -----------------------------------------------------------------------
    // Lookups for the sheets

    /// Kerb setout points: each rounded corner's tangent points and centre.
    pub fn kerb_points(&self) -> Vec<(String, P)> {
        let mut out = Vec::new();
        for isl in &self.islands {
            for (k, &(x, y, r)) in isl.corners.iter().enumerate() {
                if r <= 0.0 {
                    continue;
                }
                let n = isl.corners.len();
                let (prev, next) = (isl.corners[(k + n - 1) % n], isl.corners[(k + 1) % n]);
                let u1 = unit(sub((x, y), (prev.0, prev.1)));
                let u2 = unit(sub((next.0, next.1), (x, y)));
                let turn = cross(u1, u2).atan2(u1.0 * u2.0 + u1.1 * u2.1);
                let t = r * (turn.abs() / 2.0).tan();
                let t1 = sub((x, y), mul(u1, t));
                let t2 = add((x, y), mul(u2, t));
                let c = add(t1, mul(left(u1), r * turn.signum()));
                let base = format!("K{}.{}", isl.id, k + 1);
                out.push((format!("{base}a"), t1));
                out.push((format!("{base}b"), t2));
                out.push((format!("{base}c"), c));
            }
        }
        out
    }
}

/// A road's levels: an intersection point wherever another road meets it,
/// matching there with no curve, and between them every 70 metres or so,
/// each with a vertical curve.
fn profile(road: &Road) -> Profile {
    let length = road.length();
    let at = |ch: f64| {
        let p = road.point(ch, 0.0);
        shaped(p.0, p.1)
    };
    let mut fixed: Vec<f64> = road.crossings.iter().map(|c| c.0 - road.from).filter(|&c| c > 1.0 && c < length - 1.0).collect();
    fixed.insert(0, 0.0);
    fixed.push(length);
    let mut ips = Vec::new();
    for w in fixed.windows(2) {
        ips.push((w[0], at(w[0]), 0.0));
        let n = ((w[1] - w[0]) / 70.0).floor() as usize;
        for k in 1..n {
            let c = w[0] + (w[1] - w[0]) * k as f64 / n as f64;
            let gap = (w[1] - w[0]) / n as f64;
            ips.push((c, at(c) + 0.08 * ((c / 37.0).sin()), (gap * 0.6).min(40.0)));
        }
    }
    ips.push((length, at(length), 0.0));
    Profile { ips }
}

/// The blocks inside the kerbs: the grid of cells between road lines,
/// merged across any line with no road on it. Every block has to come out
/// a rectangle.
fn islands(roads: &[Road], ys: &[f64], height: f64) -> Vec<Island> {
    let mut xl: Vec<f64> = roads.iter().filter(|r| r.axis == Axis::NorthSouth).map(|r| r.at).collect();
    xl.push(-BEYOND);
    xl.push(SITE_W + 60.0);
    xl.sort_by(f64::total_cmp);
    xl.dedup();
    let mut yl: Vec<f64> = ys.to_vec();
    yl.push(-BEYOND);
    yl.push(height + BEYOND);
    yl.sort_by(f64::total_cmp);
    let (nx, ny) = (xl.len() - 1, yl.len() - 1);
    let eps = 0.01;
    let ns_on = |x: f64, y0: f64, y1: f64| roads.iter().find(|r| r.axis == Axis::NorthSouth && (r.at - x).abs() < eps && r.ext.0 <= y0 + eps && r.ext.1 >= y1 - eps).map(|r| r.id);
    let ew_on = |y: f64, x0: f64, x1: f64| roads.iter().find(|r| r.axis == Axis::EastWest && (r.at - y).abs() < eps && r.ext.0 <= x0 + eps && r.ext.1 >= x1 - eps).map(|r| r.id);

    let mut parent: Vec<usize> = (0..nx * ny).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    let cell = |i: usize, j: usize| j * nx + i;
    for i in 0..nx {
        for j in 0..ny {
            if i + 1 < nx && ns_on(xl[i + 1], yl[j], yl[j + 1]).is_none() {
                let (a, b) = (find(&mut parent, cell(i, j)), find(&mut parent, cell(i + 1, j)));
                parent[a] = b;
            }
            if j + 1 < ny && ew_on(yl[j + 1], xl[i], xl[i + 1]).is_none() {
                let (a, b) = (find(&mut parent, cell(i, j)), find(&mut parent, cell(i, j + 1)));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for i in 0..nx {
        for j in 0..ny {
            let root = find(&mut parent, cell(i, j));
            groups.entry(root).or_default().push((i, j));
        }
    }
    let mut groups: Vec<Vec<(usize, usize)>> = groups.into_values().collect();
    groups.sort_by_key(|g| g.iter().map(|&(i, j)| (j, i)).min());

    let mut islands = Vec::new();
    for g in groups {
        let (i0, i1) = (g.iter().map(|c| c.0).min().unwrap_or(0), g.iter().map(|c| c.0).max().unwrap_or(0) + 1);
        let (j0, j1) = (g.iter().map(|c| c.1).min().unwrap_or(0), g.iter().map(|c| c.1).max().unwrap_or(0) + 1);
        assert_eq!(g.len(), (i1 - i0) * (j1 - j0), "a block between x {}..{} and y {}..{} isn't a rectangle", xl[i0], xl[i1], yl[j0], yl[j1]);
        let (x0, x1, y0, y1) = (xl[i0], xl[i1], yl[j0], yl[j1]);
        let sides = [ew_on(y0, x0, x1), ns_on(x1, y0, y1), ew_on(y1, x0, x1), ns_on(x0, y0, y1)];
        let half = |s: Option<usize>| s.map_or(0.0, |r| roads[r].kind.half());
        let verge = |s: Option<usize>| s.map_or(0.0, |r| roads[r].kind.verge());
        let kerb = (x0 + half(sides[3]), y0 + half(sides[0]), x1 - half(sides[1]), y1 - half(sides[2]));
        let radius = |a: Option<usize>, b: Option<usize>| match (a, b) {
            (Some(p), Some(q)) => roads[p].kind.corner().max(roads[q].kind.corner()),
            _ => 0.0,
        };
        let corners = vec![
            (kerb.0, kerb.1, radius(sides[3], sides[0])),
            (kerb.2, kerb.1, radius(sides[0], sides[1])),
            (kerb.2, kerb.3, radius(sides[1], sides[2])),
            (kerb.0, kerb.3, radius(sides[2], sides[3])),
        ];
        let verges: Vec<f64> = sides.iter().map(|&s| verge(s)).collect();
        let property = (kerb.0 + verges[3], kerb.1 + verges[0], kerb.2 - verges[1], kerb.3 - verges[2]);
        let c = centre(kerb);
        let use_ = if c.0 > 1060.0 {
            Use::Corridor
        } else if in_box(c, (300.0, ys[5], 600.0, ys[6])) {
            Use::Park
        } else if in_box(c, (430.0, ys[1], 600.0, ys[2])) {
            Use::School
        } else if in_box(c, (600.0, ys[4], 770.0, ys[5])) {
            Use::Centre
        } else {
            Use::Lots
        };
        islands.push(make_island(islands.len(), corners, verges, sides, kerb, property, use_));
    }
    // The boulevard's median, between the junctions.
    for road in roads.iter().filter(|r| r.kind == Kind::Boulevard) {
        let mut xs: Vec<(f64, f64)> = road.crossings.iter().map(|&(x, j)| (x, roads[j].kind.reserve() + 6.0)).collect();
        xs.insert(0, (road.ext.0 - 20.0, 0.0));
        xs.push((road.ext.1 + 20.0, 0.0));
        for w in xs.windows(2) {
            let (a, b) = (w[0].0 + w[0].1, w[1].0 - w[1].1);
            if b - a < 10.0 {
                continue;
            }
            let m = road.kind.median();
            let kerb = (a, road.at - m, b, road.at + m);
            let corners = rect_pts(kerb).into_iter().map(|(x, y)| (x, y, m * 0.999)).collect();
            islands.push(make_island(islands.len(), corners, vec![0.0; 4], [None; 4], kerb, kerb, Use::Median));
        }
    }
    islands
}

fn make_island(id: usize, corners: Vec<Corner>, verges: Vec<f64>, sides: [Option<usize>; 4], kerb: Bx, property: Bx, use_: Use) -> Island {
    let mut isl = Island { id, corners, verges, sides, kerb, property, use_, lip: Vec::new(), back: Vec::new(), boundary: Vec::new() };
    isl.lip = isl.ring(|_| 0.0);
    isl.back = isl.ring(|_| 0.45);
    isl.boundary = isl.ring(|v| v);
    if use_ == Use::Median {
        isl.back = isl.lip.clone();
    }
    isl
}
