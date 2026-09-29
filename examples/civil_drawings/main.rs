//! Writes a large drawing set for the Microsoft Store screenshots: the civil
//! engineering drawings for a township that doesn't exist -- some 1,350 lots
//! on a boulevard, a collector and local streets, with their earthworks,
//! drainage, basins, water, sewer, power, signs and trees -- drawn for the
//! purpose, so nobody's real drawings are shown, and big enough to show the
//! app opening a set of several hundred A1 sheets.
//!
//!     cargo run --release --example civil_drawings -- out.pdf [--png]
//!
//! One model of the town (`town`) is drawn by every sheet, so the plans,
//! sections and schedules agree with each other, and each sheet's
//! references to the others are to sheets that exist. Everything is drawn in
//! millimetres on the paper; a `View` turns the site's metres into paper at
//! a drawing's scale. The take-off, highlights and notes are written by the
//! app's own save, as a user's would be, and read back to check.

#[path = "../sample_ink/mod.rs"]
mod sample_ink;

mod details;
mod draw;
mod geom;
mod plans;
mod profiles;
mod tables;
mod town;

use std::time::Instant;

use kinetic_pdf::model::{Changes, Markup as Markup_, MarkupKind as DrawKind, MeasureChanges, NewHighlight, PdfBox, ScaleChanges};
use kinetic_pdf::{annots, selection, worker};
use markup_model::viewport::Viewport;
use markup_model::{Geometry, Markup, MarkupKind, Pt, Rect, Scale, ScaleId, ScaleStore, ViewportId};
use pdf_content::lopdf::{dictionary, Document, Object, Stream, StringFormat};

use geom::*;
use plans::Overall;
use profiles::{Long, PANEL};
use sample_ink::{Ink, View, PT};
use town::*;

/// A1 landscape.
pub const SHEET: (f64, f64) = (841.0, 594.0);
/// Whose notes and measurements they are.
const AUTHOR: &str = "S. Taylor";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disc {
    Ga,
    Setout,
    CutFill,
    Grading,
    Drainage,
    WaterSewer,
    Electrical,
    Signage,
    Landscape,
}

impl Disc {
    pub const ALL: [Disc; 9] = [Disc::Ga, Disc::Setout, Disc::CutFill, Disc::Grading, Disc::Drainage, Disc::WaterSewer, Disc::Electrical, Disc::Signage, Disc::Landscape];

    /// The number of its first sector's sheet.
    fn base(self) -> usize {
        match self {
            Disc::Ga => 101,
            Disc::Setout => 121,
            Disc::CutFill => 201,
            Disc::Grading => 221,
            Disc::Drainage => 401,
            Disc::WaterSewer => 501,
            Disc::Electrical => 601,
            Disc::Signage => 701,
            Disc::Landscape => 721,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Disc::Ga => "GENERAL ARRANGEMENT",
            Disc::Setout => "SETOUT PLAN",
            Disc::CutFill => "BULK EARTHWORKS, CUT AND FILL",
            Disc::Grading => "FINISHED LEVELS AND RETAINING",
            Disc::Drainage => "STORMWATER DRAINAGE PLAN",
            Disc::WaterSewer => "WATER AND SEWER RETICULATION",
            Disc::Electrical => "ELECTRICAL, COMMUNICATIONS AND GAS",
            Disc::Signage => "SIGNAGE AND LINEMARKING PLAN",
            Disc::Landscape => "STREET TREES AND LANDSCAPE",
        }
    }
}

pub struct Sector {
    pub id: usize,
    pub col: usize,
    pub row: usize,
    pub bx: Bx,
}

pub enum Job {
    Cover,
    Index(usize),
    Notes,
    Overall(Overall),
    Sector(Disc, usize),
    LotSetout(usize),
    Typical(usize),
    RoadLong(Vec<(usize, f64, f64)>),
    Cross(Vec<(usize, f64)>),
    DrainLong(Vec<Vec<Vec<usize>>>),
    SewerLong(Vec<Vec<(usize, usize, usize)>>),
    Pits(Vec<usize>),
    Pipes(Vec<usize>),
    Manholes(Vec<(usize, usize)>),
    Basin(usize),
    Detail(usize),
}

pub struct Sheet {
    pub number: String,
    pub title: Vec<String>,
    pub scale: String,
    pub job: Job,
}

/// Rows of the drawing index a sheet holds, and a column of it.
pub type SheetsPer = (usize, usize);
const INDEX: SheetsPer = (196, 98);

pub struct Set {
    pub sheets: Vec<Sheet>,
    pub sectors: Vec<Sector>,
}

impl Set {
    pub fn sector_sheet(&self, d: Disc, id: usize) -> String {
        format!("C-{}", d.base() + id)
    }

    pub fn lot_setout_sheet(&self, id: usize) -> String {
        format!("C-{}", 141 + id)
    }

    pub fn sector_of(&self, p: P) -> Option<usize> {
        self.sectors.iter().find(|s| in_box(p, s.bx)).map(|s| s.id)
    }

    /// The sheets whose titles have `key` in them: one, or first to last.
    pub fn range_of(&self, key: &str) -> String {
        let hits: Vec<&Sheet> = self.sheets.iter().filter(|s| s.title.join(" ").contains(key)).collect();
        match hits.as_slice() {
            [] => "–".into(),
            [one] => one.number.clone(),
            [first, .., last] => format!("{} to {}", first.number, last.number),
        }
    }

    /// The plans a stretch of road is on.
    pub fn plans_for(&self, d: Disc, _town: &Town, road: &Road, ch0: f64, ch1: f64) -> String {
        let mut ids: Vec<usize> = Vec::new();
        let mut ch = ch0;
        while ch <= ch1 {
            if let Some(s) = self.sector_of(road.point(ch, 0.0)) {
                if !ids.contains(&s) {
                    ids.push(s);
                }
            }
            ch += 10.0;
        }
        ids.iter().map(|&s| self.sector_sheet(d, s)).collect::<Vec<_>>().join(", ")
    }

    fn page_of(&self, number: &str) -> usize {
        self.sheets.iter().position(|s| s.number == number).unwrap_or_else(|| panic!("no sheet {number}"))
    }
}

/// Every sheet in the set, numbered, in order.
fn plan_set(town: &Town) -> Set {
    let h = town.height / 4.0;
    let mut sectors = Vec::new();
    for row in (0..4).rev() {
        for col in 0..4 {
            let id = sectors.len();
            sectors.push(Sector { id, col, row, bx: (col as f64 * 300.0, row as f64 * h, (col + 1) as f64 * 300.0, (row + 1) as f64 * h) });
        }
    }
    let mut body: Vec<Sheet> = Vec::new();
    let mut push = |number: usize, title: Vec<String>, scale: &str, job: Job| {
        body.push(Sheet { number: format!("C-{number:03}"), title, scale: scale.into(), job });
    };
    let s = |x: &str| x.to_owned();
    push(10, vec![s("GENERAL NOTES")], "–", Job::Notes);
    let overall = [
        (Overall::Layout, "OVERALL LAYOUT AND SECTOR KEY"),
        (Overall::Staging, "STAGING PLAN"),
        (Overall::Lots, "OVERALL SUBDIVISION PLAN"),
    ];
    for (k, (o, t)) in overall.iter().enumerate() {
        push(11 + k, vec![s(t)], "1:2500 @ A1", Job::Overall(*o));
    }
    let sector_sheets = |push: &mut dyn FnMut(usize, Vec<String>, &str, Job), d: Disc| {
        for sec in &sectors {
            push(d.base() + sec.id, vec![d.title().into(), format!("SECTOR {}", sec.id + 1)], "1:500 @ A1", Job::Sector(d, sec.id));
        }
    };
    sector_sheets(&mut push, Disc::Ga);
    sector_sheets(&mut push, Disc::Setout);
    for sec in &sectors {
        push(141 + sec.id, vec![s("LOT SETOUT TABLES"), format!("SECTOR {}", sec.id + 1)], "–", Job::LotSetout(sec.id));
    }
    push(200, vec![s("OVERALL EARTHWORKS, CUT AND FILL")], "1:2500 @ A1", Job::Overall(Overall::Earthworks));
    sector_sheets(&mut push, Disc::CutFill);
    sector_sheets(&mut push, Disc::Grading);
    push(301, vec![s("TYPICAL ROAD SECTIONS"), s("BOULEVARD, PAVEMENTS")], "1:50 @ A1", Job::Typical(0));
    push(302, vec![s("TYPICAL ROAD SECTIONS"), s("COLLECTOR AND LOCAL")], "1:50 @ A1", Job::Typical(1));

    // Each road's long section in panels, two to a sheet.
    let mut panels = Vec::new();
    for road in &town.roads {
        let n = (road.length() / PANEL).ceil().max(1.0) as usize;
        for k in 0..n {
            panels.push((road.id, road.length() * k as f64 / n as f64, road.length() * (k + 1) as f64 / n as f64));
        }
    }
    let mut number = 311;
    for pair in panels.chunks(2) {
        let names: Vec<String> = pair.iter().map(|&(r, a, b)| format!("{} {a:.0}–{b:.0}", town.roads[r].short())).collect();
        push(number, vec![s("ROAD LONG SECTIONS"), names.join(", ")], "H 1:500 V 1:100", Job::RoadLong(pair.to_vec()));
        number += 1;
    }
    // Cross sections every 20 metres, clear of the junctions.
    let mut list = Vec::new();
    for road in &town.roads {
        let mut ch = 10.0;
        while ch < road.length() - 5.0 {
            let clear = road.crossings.iter().all(|&(at, j)| (at - road.from - ch).abs() > town.roads[j].kind.reserve() + road.kind.corner() + 2.0);
            let p = road.point(ch, 0.0);
            if clear && p.0 < 1050.0 {
                list.push((road.id, ch));
            }
            ch += 20.0;
        }
    }
    let mut number = (number / 10 + 1) * 10 + 1;
    for chunk in list.chunks(18) {
        let (r, a, b) = (chunk[0].0, chunk[0].1, chunk[chunk.len() - 1].1);
        let span = if chunk.iter().all(|c| c.0 == r) { format!("{} CH {a:.0} TO {b:.0}", town.roads[r].short()) } else { format!("{} CH {a:.0} ON", town.roads[r].short()) };
        push(number, vec![s("ROAD CROSS SECTIONS"), span], "H 1:200 V 1:100", Job::Cross(chunk.to_vec()));
        number += 1;
    }

    push(400, vec![s("OVERALL STORMWATER")], "1:2500 @ A1", Job::Overall(Overall::Stormwater));
    sector_sheets(&mut push, Disc::Drainage);
    let mut panels: Vec<Vec<usize>> = Vec::new();
    for line in &town.lines {
        panels.extend(profiles::pit_panels(town, line));
    }
    let lengths: Vec<f64> = panels.iter().map(|p| p.windows(2).map(|w| dist(town.pits[w[0]].at, town.pits[w[1]].at)).sum()).collect();
    let mut number = 421;
    for sheet in profiles::pack(&lengths) {
        let rows: Vec<Vec<Vec<usize>>> = sheet.iter().map(|row| row.iter().map(|&i| panels[i].clone()).collect()).collect();
        let first = &town.pits[rows[0][0][0]].name;
        push(number, vec![s("STORMWATER LONG SECTIONS"), format!("FROM PIT {first}")], "H 1:500 V 1:100", Job::DrainLong(rows));
        number += 1;
    }
    // Every pit, by the line it drains down.
    let mut order: Vec<usize> = (0..town.pits.len()).collect();
    order.sort_by_key(|&i| (town.pits[i].line, i));
    let mut number = (number / 10 + 1) * 10 + 1;
    let per = tables::ROWS * 2;
    let pages = order.len().div_ceil(per);
    for (k, chunk) in order.chunks(per).enumerate() {
        push(number, vec![s("STORMWATER PIT SCHEDULE"), format!("SHEET {} OF {pages}", k + 1)], "–", Job::Pits(chunk.to_vec()));
        number += 1;
    }
    let pipes: Vec<usize> = (0..town.pipes.len()).collect();
    let pages = pipes.len().div_ceil(per);
    for (k, chunk) in pipes.chunks(per).enumerate() {
        push(number, vec![s("STORMWATER PIPE SCHEDULE"), format!("SHEET {} OF {pages}", k + 1)], "–", Job::Pipes(chunk.to_vec()));
        number += 1;
    }
    let mut number = (number / 10 + 1) * 10 + 1;
    for (k, _) in town.basins.iter().enumerate() {
        push(number, vec![format!("DETENTION BASIN {}", k + 1), s("PLAN AND SECTIONS")], "AS SHOWN", Job::Basin(k));
        number += 1;
    }

    push(500, vec![s("OVERALL WATER AND SEWER")], "1:2500 @ A1", Job::Overall(Overall::WaterSewer));
    sector_sheets(&mut push, Disc::WaterSewer);
    let items: Vec<(usize, usize, usize)> = town.sewers.iter().enumerate().map(|(k, s)| (k, 0, s.manholes.len() - 1)).collect();
    let lengths: Vec<f64> = town.sewers.iter().map(|s| dist(s.manholes[0].at, s.manholes[s.manholes.len() - 1].at)).collect();
    let mut number = 521;
    for sheet in profiles::pack(&lengths) {
        let rows: Vec<Vec<(usize, usize, usize)>> = sheet.iter().map(|row| row.iter().map(|&i| items[i]).collect()).collect();
        let names: Vec<String> = rows.iter().flatten().map(|&(k, _, _)| town.sewers[k].name.clone()).collect();
        push(number, vec![s("SEWER LONG SECTIONS"), format!("LINES {}", names.join(", "))], "H 1:500 V 1:100", Job::SewerLong(rows));
        number += 1;
    }
    let mhs: Vec<(usize, usize)> = town.sewers.iter().enumerate().flat_map(|(k, s)| (0..s.manholes.len()).map(move |m| (k, m))).collect();
    let mut number = (number / 10 + 1) * 10 + 1;
    let pages = mhs.len().div_ceil(per);
    for (k, chunk) in mhs.chunks(per).enumerate() {
        push(number, vec![s("SEWER MANHOLE SCHEDULE"), format!("SHEET {} OF {pages}", k + 1)], "–", Job::Manholes(chunk.to_vec()));
        number += 1;
    }
    push(600, vec![s("OVERALL ELECTRICAL AND LIGHTING")], "1:2500 @ A1", Job::Overall(Overall::Electrical));
    sector_sheets(&mut push, Disc::Electrical);
    sector_sheets(&mut push, Disc::Signage);
    push(720, vec![s("OVERALL STREET TREES")], "1:2500 @ A1", Job::Overall(Overall::Landscape));
    sector_sheets(&mut push, Disc::Landscape);
    for (k, (name, scale, _)) in details::DETAILS.iter().enumerate() {
        push(801 + k, vec![s("DETAILS"), s(name)], scale, Job::Detail(k));
    }

    // The cover and the index at the front, the index long enough for all.
    let mut parts = 1;
    while (body.len() + 1 + parts).div_ceil(INDEX.0) > parts {
        parts += 1;
    }
    let mut sheets = vec![Sheet { number: "C-000".into(), title: vec![s("COVER SHEET"), s("AND LOCALITY PLAN")], scale: "NTS".into(), job: Job::Cover }];
    for k in 0..parts {
        sheets.push(Sheet { number: format!("C-{:03}", 1 + k), title: vec![s("DRAWING INDEX"), format!("SHEET {} OF {parts}", k + 1)], scale: "–".into(), job: Job::Index(k) });
    }
    sheets.extend(body);
    let set = Set { sheets, sectors };
    let mut seen = std::collections::HashSet::new();
    for sheet in &set.sheets {
        assert!(seen.insert(sheet.number.clone()), "{} is numbered twice", sheet.number);
    }
    set
}

/// Sections through a basin, along and across, at H 1:500 V 1:100.
fn basin_sections(ink: &mut Ink, b: &Basin) -> [Long; 2] {
    let bx = b.bx();
    let c = centre(bx);
    let along = ((c.0, bx.1 - 15.0), (c.0, bx.3 + 15.0));
    let across = ((bx.0 - 15.0, c.1), (bx.2 + 15.0, c.1));
    let mut out = Vec::new();
    for (k, (name, (a, z))) in [("A", along), ("B", across)].into_iter().enumerate() {
        let len = dist(a, z);
        let u = unit(sub(z, a));
        let pt = |s: f64| add(a, mul(u, s));
        let n = (len / 0.5) as usize;
        let ss: Vec<f64> = (0..=n).map(|i| i as f64 * 0.5).collect();
        let lo = b.floor.min(ss.iter().map(|&s| ground(pt(s).0, pt(s).1)).fold(f64::MAX, f64::min));
        let l = Long { ox: 470.0, oy: 400.0 - k as f64 * 190.0, kh: 2.0, kv: 10.0, datum: (lo - 1.0).floor(), ch0: 0.0 };
        let existing: Vec<(f64, f64)> = ss.iter().map(|&s| l.p(s, ground(pt(s).0, pt(s).1))).collect();
        let design: Vec<(f64, f64)> = ss.iter().map(|&s| l.p(s, b.surface(pt(s)).unwrap_or(ground(pt(s).0, pt(s).1)))).collect();
        for (i, &s) in ss.iter().enumerate() {
            if i % 2 == 0 {
                if let Some(d) = b.surface(pt(s)) {
                    let e = ground(pt(s).0, pt(s).1);
                    ink.pen(0.12, if d < e { [0.85, 0.2, 0.2] } else { [0.2, 0.4, 0.85] });
                    ink.line(l.p(s, e), l.p(s, d));
                }
            }
        }
        ink.pen(0.25, [0.45, 0.3, 0.15]);
        ink.dash(&[1.5, 0.8]);
        ink.poly(&existing, false, "S");
        ink.dash(&[]);
        ink.pen(0.5, sample_ink::BLACK);
        ink.poly(&design, false, "S");
        ink.pen(0.3, draw::STORM);
        ink.dash(&[3.0, 1.0]);
        let (ta, tb) = (bx.1 + (b.top - b.twl) * BATTER, bx.3 - (b.top - b.twl) * BATTER);
        let (ta, tb) = if k == 0 { (ta - bx.1 + 15.0, tb - bx.1 + 15.0) } else { (bx.0 - 15.0 + (b.top - b.twl) * BATTER - (bx.0 - 15.0) + 15.0, len - 15.0 - (b.top - b.twl) * BATTER) };
        ink.line(l.p(ta, b.twl), l.p(tb, b.twl));
        ink.dash(&[]);
        let mut chs: Vec<f64> = (0..).map(|i| i as f64 * 10.0).take_while(|&c| c < len).collect();
        chs.push(len);
        let xs: Vec<f64> = chs.iter().map(|&c| l.p(c, 0.0).0).collect();
        let rows = vec![
            ("DESIGN", chs.iter().map(|&c| format!("{:.2}", b.surface(pt(c)).unwrap_or(ground(pt(c).0, pt(c).1)))).collect::<Vec<_>>()),
            ("NATURAL", chs.iter().map(|&c| format!("{:.2}", ground(pt(c).0, pt(c).1))).collect()),
            ("CHAINAGE", chs.iter().map(|c| format!("{c:.1}")).collect()),
        ];
        ink.fill(sample_ink::BLACK);
        ink.text((l.ox - 58.0, l.oy + 2.0), 2.1, sample_ink::Font::Bold, sample_ink::Align::Left, &format!("DATUM RL {:.2}", l.datum));
        let bottom = draw::bands(ink, l.ox - 60.0, l.oy, xs[xs.len() - 1] + 4.0, &xs, &rows, 9.0);
        draw::title(ink, (l.ox - 60.0, bottom - 12.0), name, &format!("SECTION {name}"), "H 1:500  V 1:100");
        out.push(l);
    }
    [out[0], out[1]]
}

/// What a sheet gives back for the take-off to be measured on.
#[derive(Default)]
struct Drawn {
    plan: Option<View>,
    longs: Vec<Long>,
    viewports: Vec<(&'static str, Bx, f64)>,
}

fn render(ink: &mut Ink, set: &Set, town: &Town, sheet: &Sheet) -> Drawn {
    let mut out = Drawn::default();
    match &sheet.job {
        Job::Cover => tables::cover_sheet(ink, set, town),
        Job::Index(k) => tables::index_sheet(ink, set, *k, INDEX),
        Job::Notes => tables::notes_sheet(ink),
        Job::Overall(o) => plans::overall_sheet(ink, set, town, *o),
        Job::Sector(d, id) => out.plan = Some(plans::sector_sheet(ink, set, town, *d, &set.sectors[*id])),
        Job::LotSetout(id) => tables::lot_setout(ink, town, set.sectors[*id].bx),
        Job::Typical(k) => {
            details::typical_sheet(ink, *k);
        }
        Job::RoadLong(panels) => {
            for (k, &(r, a, b)) in panels.iter().enumerate() {
                out.longs.push(profiles::road_panel(ink, set, town, &town.roads[r], a, b, 440.0 - k as f64 * 250.0));
            }
        }
        Job::Cross(list) => out.longs = profiles::cross_sheet(ink, town, list),
        Job::DrainLong(rows) => {
            for (k, row) in rows.iter().enumerate() {
                let mut x = 30.0;
                for path in row {
                    let (stations, runs) = profiles::drain_stations(town, path);
                    let line = &town.lines[town.pits[path[0]].line];
                    let road = &town.roads[line.road];
                    let sub = if line.name.contains("outlet") { "CORRIDOR".to_owned() } else { road.name.to_uppercase() };
                    let l = profiles::pipe_panel(ink, x, 420.0 - k as f64 * 250.0, &stations, &runs, (&format!("LINE {}  ·  {sub}", line.name), &format!("H 1:500  V 1:100  ·  PITS {} TO {}", stations[0].name, stations[stations.len() - 1].name)), false);
                    x = l.p(stations.iter().zip(stations.iter().skip(1)).map(|(a, b)| dist(a.at, b.at)).sum(), 0.0).0 + 30.0;
                    out.longs.push(l);
                }
            }
        }
        Job::SewerLong(rows) => {
            for (k, row) in rows.iter().enumerate() {
                let mut x = 30.0;
                for &(s, a, b) in row {
                    let sewer = &town.sewers[s];
                    let (stations, runs) = profiles::sewer_stations(sewer, a, b);
                    let to = sewer.outlet.map_or("THE TRUNK SEWER".into(), |r| town.roads[r].name.to_uppercase());
                    let l = profiles::pipe_panel(ink, x, 420.0 - k as f64 * 250.0, &stations, &runs, (&format!("SEWER {}", sewer.name), &format!("H 1:500  V 1:100  ·  TO THE MAIN IN {to}")), true);
                    x = l.p(stations.windows(2).map(|w| dist(w[0].at, w[1].at)).sum(), 0.0).0 + 30.0;
                    out.longs.push(l);
                }
            }
        }
        Job::Pits(list) => tables::pit_schedule(ink, set, town, list),
        Job::Pipes(list) => tables::pipe_schedule(ink, town, list),
        Job::Manholes(list) => tables::manhole_schedule(ink, set, town, list),
        Job::Basin(k) => {
            out.plan = Some(plans::basin_plan(ink, town, &town.basins[*k]));
            out.longs.extend(basin_sections(ink, &town.basins[*k]));
        }
        Job::Detail(k) => out.viewports = details::detail_sheet(ink, *k),
    }
    out
}

// ---------------------------------------------------------------------------
// The take-off

fn area(page: usize, pts: Vec<Pt>, name: &str, desc: &str, colour: [f64; 3], depth: Option<f64>) -> Markup {
    let kind = if depth.is_some() { MarkupKind::Volume } else { MarkupKind::Area };
    let mut m = Markup::new(page as u32, kind, Geometry::Polygon { pts, holes: Vec::new() });
    m.style.stroke = colour.map(|c| c as f32);
    m.style.fill = Some(colour.map(|c| c as f32));
    m.style.fill_opacity = 0.28;
    m.style.width = 1.5;
    m.extras.depth_m = depth;
    m.meta.name = name.into();
    m.meta.label = desc.into();
    m.meta.author = AUTHOR.into();
    m
}

fn measured(page: usize, kind: MarkupKind, geometry: Geometry, name: &str, desc: &str, colour: [f64; 3]) -> Markup {
    let mut m = Markup::new(page as u32, kind, geometry);
    m.style.stroke = colour.map(|c| c as f32);
    m.style.width = 2.0;
    m.meta.name = name.into();
    m.meta.label = desc.into();
    m.meta.author = AUTHOR.into();
    m
}

fn pipe_colour(dia: f64) -> [f64; 3] {
    let t = ((dia - 0.375) / 1.1).clamp(0.0, 1.0);
    [0.13 + 0.6 * t, 0.45 - 0.25 * t, 0.85 - 0.3 * t]
}

/// Each tool used: group, description, key, colour, depth.
type Tool = (&'static str, String, &'static str, [f64; 3], Option<f64>);

struct TakeOff {
    markups: Vec<Markup>,
    tools: Vec<Tool>,
}

impl TakeOff {
    fn tool(&mut self, group: &'static str, desc: &str, key: &'static str, colour: [f64; 3], depth: Option<f64>) {
        if !self.tools.iter().any(|t| t.1 == desc) {
            self.tools.push((group, desc.to_owned(), key, colour, depth));
        }
    }

    fn count(&mut self, page: usize, pts: Vec<Pt>, name: &str, group: &'static str, desc: &str, colour: [f64; 3]) {
        if pts.is_empty() {
            return;
        }
        self.tool(group, desc, "measure.count", colour, None);
        self.markups.push(measured(page, MarkupKind::Count, Geometry::Points { pts }, name, desc, colour));
    }

    fn polylength(&mut self, page: usize, pts: Vec<Pt>, name: &str, group: &'static str, desc: &str, colour: [f64; 3]) {
        self.tool(group, desc, "measure.polylength", colour, None);
        self.markups.push(measured(page, MarkupKind::Polylength, Geometry::Polyline { pts }, name, desc, colour));
    }
}

const ASPHALT: [f64; 3] = [0.25, 0.3, 0.4];
const KERB: [f64; 3] = [0.0, 0.58, 0.6];
const FOOTPATH: [f64; 3] = [0.93, 0.55, 0.13];
const DRIVEWAYS: [f64; 3] = [0.45, 0.45, 0.2];
const EARTH: [f64; 3] = [0.55, 0.35, 0.75];
const PITS: [f64; 3] = [0.86, 0.15, 0.15];
const SEWERS: [f64; 3] = [0.55, 0.3, 0.1];
const HYDRANTS: [f64; 3] = [0.0, 0.55, 0.85];
const LIGHTS: [f64; 3] = [0.92, 0.65, 0.05];
const KIOSKS: [f64; 3] = [0.9, 0.3, 0.1];
const SIGNS: [f64; 3] = [0.7, 0.1, 0.1];
const LEVELS: [f64; 3] = [0.8, 0.2, 0.6];
const WIDTHS: [f64; 3] = [0.1, 0.45, 0.85];

/// The sector the take-off is measured in: the park, the boulevard and the
/// streets round them.
fn take_off_sector(set: &Set) -> usize {
    set.sector_of((450.0, 450.0)).expect("a sector there")
}

fn take_off(set: &Set, town: &Town, drawn: &[Drawn]) -> TakeOff {
    let mut t = TakeOff { markups: Vec::new(), tools: Vec::new() };
    let sid = take_off_sector(set);
    let w = set.sectors[sid].bx;
    let page = |d: Disc| set.page_of(&set.sector_sheet(d, sid));
    let v = plans::sector_view(&set.sectors[sid]);
    let pts = |list: &[P]| -> Vec<Pt> { list.iter().map(|&(x, y)| v.pt(x, y)).collect() };

    // Roads: asphalt, kerbs, paths, driveways.
    let ga = page(Disc::Ga);
    for road in town.roads.iter().filter(|r| overlaps(r.reserve_box(), w)) {
        let (k, m) = (road.kind.half(), road.kind.median());
        let strips: Vec<(f64, f64)> = if m > 0.0 { vec![(-k, -m), (m, k)] } else { vec![(-k, k)] };
        for (a, b) in strips {
            let quad = [road.point(0.0, a), road.point(road.length(), a), road.point(road.length(), b), road.point(0.0, b)];
            let poly = clip_polygon(&quad, w);
            if poly.len() > 2 && polygon_area(&poly) > 10.0 {
                t.tool("Roadworks", "Asphalt", "measure.area", ASPHALT, None);
                t.markups.push(area(ga, pts(&poly), &road.short(), "Asphalt", ASPHALT, None));
            }
        }
    }
    for isl in town.islands.iter().filter(|i| overlaps(bounds(&i.lip), w)) {
        for run in clip_polyline(&closed(&isl.lip), w) {
            t.polylength(ga, pts(&run), &format!("Block {}", isl.id), "Roadworks", "Kerb and gutter", KERB);
        }
        if isl.has_verge() && isl.use_ != Use::Median {
            for run in clip_polyline(&closed(&isl.ring(|g| g - 0.9)), w) {
                t.polylength(ga, pts(&run), &format!("Block {}", isl.id), "Roadworks", "Concrete footpath", FOOTPATH);
            }
        }
    }
    let driveways: Vec<Pt> = town.lots.iter().filter(|l| in_box(l.driveway, w)).map(|l| v.pt(l.driveway.0, l.driveway.1)).collect();
    t.count(ga, driveways, &format!("Sector {}", sid + 1), "Roadworks", "Driveway crossings", DRIVEWAYS);

    // Earthworks on the lots that move most.
    let cf = page(Disc::CutFill);
    let mut lots: Vec<(&Lot, f64)> = town
        .lots
        .iter()
        .filter(|l| in_box(centre(l.bx), w))
        .map(|l| {
            let c = centre(l.bx);
            let mean = (0..9).map(|i| plans::cut_fill(town, (c.0 + ((i % 3) as f64 - 1.0) * (l.bx.2 - l.bx.0) / 3.0, c.1 + ((i / 3) as f64 - 1.0) * (l.bx.3 - l.bx.1) / 3.0)).unwrap_or(0.0)).sum::<f64>() / 9.0;
            (l, mean)
        })
        .collect();
    lots.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    for (lot, mean) in lots.iter().take(8) {
        let desc = if *mean < 0.0 { "Lot cut" } else { "Lot fill" };
        let depth = (mean.abs() * 100.0).round() / 100.0;
        t.tool("Earthworks", desc, "measure.area", EARTH, Some(depth));
        t.markups.push(area(cf, pts(&rect_pts(lot.bx)), &format!("Lot {}", lot.number), desc, EARTH, Some(depth.max(0.01))));
    }

    // Drainage.
    let dr = page(Disc::Drainage);
    let pits: Vec<Pt> = town.pits.iter().filter(|p| matches!(p.kind, PitKind::Kerb(_)) && in_box(p.at, w)).map(|p| v.pt(p.at.0, p.at.1)).collect();
    t.count(dr, pits, &format!("Sector {}", sid + 1), "Stormwater", "Kerb inlet pits", PITS);
    for pipe in town.pipes.iter().filter(|p| in_box(town.pits[p.from].at, w) && in_box(town.pits[p.to].at, w)) {
        let desc = format!("RCP {:.0}", pipe.dia * 1000.0);
        let (a, b) = (town.pits[pipe.from].at, town.pits[pipe.to].at);
        t.polylength(dr, pts(&[a, b]), &format!("{} – {}", town.pits[pipe.from].name, town.pits[pipe.to].name), "Stormwater", &desc, pipe_colour(pipe.dia));
    }

    // Water and sewer.
    let ws = page(Disc::WaterSewer);
    for sewer in &town.sewers {
        let line: Vec<P> = sewer.manholes.iter().map(|m| m.at).collect();
        for run in clip_polyline(&line, w) {
            t.polylength(ws, pts(&run), &sewer.name, "Sewer", "Sewer main, 150 PVC", SEWERS);
        }
    }
    let mhs: Vec<Pt> = town.sewers.iter().flat_map(|s| &s.manholes).filter(|m| in_box(m.at, w)).map(|m| v.pt(m.at.0, m.at.1)).collect();
    t.count(ws, mhs, &format!("Sector {}", sid + 1), "Sewer", "Sewer manholes", SEWERS);
    let hyd: Vec<Pt> = town.hydrants.iter().filter(|p| in_box(**p, w)).map(|p| v.pt(p.0, p.1)).collect();
    t.count(ws, hyd, &format!("Sector {}", sid + 1), "Water", "Fire hydrants", HYDRANTS);

    // Lights, kiosks, signs, trees.
    let el = page(Disc::Electrical);
    let lights: Vec<Pt> = town.lights.iter().filter(|(p, _)| in_box(*p, w)).map(|(p, _)| v.pt(p.0, p.1)).collect();
    t.count(el, lights, &format!("Sector {}", sid + 1), "Electrical", "Street lights", LIGHTS);
    let kiosks: Vec<Pt> = town.kiosks.iter().filter(|p| in_box(**p, w)).map(|p| v.pt(p.0, p.1)).collect();
    t.count(el, kiosks, &format!("Sector {}", sid + 1), "Electrical", "Kiosks", KIOSKS);
    let sg = page(Disc::Signage);
    let signs: Vec<Pt> = town.signs.iter().filter(|s| in_box(s.at, w)).map(|s| v.pt(s.at.0, s.at.1)).collect();
    t.count(sg, signs, &format!("Sector {}", sid + 1), "Signage", "Signs", SIGNS);
    let ls = page(Disc::Landscape);
    for (k, sp) in SPECIES.iter().enumerate() {
        let trees: Vec<Pt> = town.trees.iter().filter(|(p, s)| *s == k && in_box(*p, w)).map(|(p, _)| v.pt(p.0, p.1)).collect();
        t.count(ls, trees, &format!("Sector {}", sid + 1), "Landscape", &format!("Street trees, {}", sp.1.split(',').next().unwrap_or("")), sp.2);
    }

    // On the boulevard's first long section, read at its vertical scale.
    let boulevard = town.roads.iter().find(|r| r.kind == Kind::Boulevard).expect("a boulevard");
    let (lp, ld) = set
        .sheets
        .iter()
        .enumerate()
        .find_map(|(i, s)| match &s.job {
            Job::RoadLong(p) if p[0].0 == boulevard.id => Some((i, &drawn[i].longs[0])),
            _ => None,
        })
        .expect("the boulevard's long section");
    let existing = |ch: f64| {
        let p = boulevard.point(ch, 0.0);
        ground(p.0, p.1)
    };
    let end = boulevard.length() / (boulevard.length() / PANEL).ceil();
    let ch = (0..end as usize).map(|c| c as f64).max_by(|a, b| (boulevard.levels.level(*a) - existing(*a)).abs().total_cmp(&(boulevard.levels.level(*b) - existing(*b)).abs())).expect("chainages");
    t.tool("Levels", "Cut and fill depth", "measure.length", LEVELS, None);
    t.markups.push(measured(lp, MarkupKind::Length, Geometry::Line { a: ld.pt(ch, existing(ch)), b: ld.pt(ch, boulevard.levels.level(ch)) }, &format!("Boulevard CH {ch:.0}"), "Cut and fill depth", LEVELS));

    // On the first cross sections of the boulevard: its width, and a cut.
    let (cp, list) = set
        .sheets
        .iter()
        .enumerate()
        .find_map(|(i, s)| match &s.job {
            Job::Cross(list) if list[0].0 == boulevard.id => Some((i, list)),
            _ => None,
        })
        .expect("the boulevard's cross sections");
    for (k, &(_, ch)) in list.iter().enumerate().take(3) {
        let l = drawn[cp].longs[k];
        let lip = boulevard.lip(ch);
        t.tool("Levels", "Carriageway width", "measure.length", WIDTHS, None);
        t.markups.push(measured(cp, MarkupKind::Length, Geometry::Line { a: l.pt(-8.5, lip + 0.6), b: l.pt(8.5, lip + 0.6) }, &format!("Boulevard CH {ch:.0}"), "Carriageway width", WIDTHS));
    }

    // The first basin, dug out.
    let bp = set.page_of(&set.range_of("DETENTION BASIN").split(' ').next().unwrap_or("").to_owned());
    let basin = &town.basins[0];
    let half = basin.ring((basin.top - basin.floor) / 2.0);
    let bv = drawn[bp].plan.expect("the basin's plan");
    t.tool("Earthworks", "Basin excavation", "measure.area", EARTH, Some(basin.top - basin.floor));
    t.markups.push(area(bp, half.iter().map(|&(x, y)| bv.pt(x, y)).collect(), "Basin 1", "Basin excavation", EARTH, Some(basin.top - basin.floor)));
    t
}

/// The tools file the app reads (`tools.json` beside its page cache), holding
/// the take-off's tools.
fn tools_json(tools: &[Tool]) -> String {
    let saved: Vec<serde_json::Value> = tools
        .iter()
        .map(|(group, name, key, colour, depth)| {
            let area = *key == "measure.area";
            serde_json::json!({
                "name": name,
                "group": group,
                "key": key,
                "settings": {
                    "style": {
                        "stroke": colour,
                        "fill": if area { Some(colour) } else { None },
                        "opacity": 1.0,
                        "fill_opacity": if area { 0.28 } else { 1.0 },
                        "pattern": "Solid",
                        "pattern_colour": null,
                        "pattern_opacity": 1.0,
                        "pattern_size": 6.0,
                        "width": if area { 1.5 } else { 2.0 },
                        "width_unit": "Points",
                        "dash": [],
                        "label_size": 10.0,
                        "label_colour": colour,
                        "label_font": "Sans",
                    },
                    "defaults": { "name": "", "description": name, "item_code": "", "layer": "", "status": "" },
                    "depth_m": depth,
                    "slope": null,
                },
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "changed": {}, "saved": saved, "collapsed": [] })).expect("JSON writes")
}

fn main() {
    let started = Instant::now();
    let out = std::env::args().nth(1).filter(|a| !a.starts_with("--")).unwrap_or_else(|| "target/store-images/civil/heron-ridge.pdf".to_owned());
    let town = Town::new();
    let set = plan_set(&town);
    println!(
        "{} sheets: {} lots, {} roads, {} pits, {} pipes, {} sewers, {} walls, {} trees ({:.1} s)",
        set.sheets.len(),
        town.lots.len(),
        town.roads.len(),
        town.pits.len(),
        town.pipes.len(),
        town.sewers.len(),
        town.walls.len(),
        town.trees.len(),
        started.elapsed().as_secs_f64()
    );

    let mut inks = Vec::new();
    let mut drawn = Vec::new();
    for (i, sheet) in set.sheets.iter().enumerate() {
        let mut ink = Ink::new();
        drawn.push(render(&mut ink, &set, &town, sheet));
        draw::frame(&mut ink, sheet, i, set.sheets.len());
        inks.push(ink);
    }
    let raw: usize = inks.iter().map(|i| i.0.len()).sum();
    println!("drawn: {:.1} MB of content ({:.1} s)", raw as f64 / 1e6, started.elapsed().as_secs_f64());

    // The PDF, with each sheet's number as its page label.
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = |doc: &mut Document, name: &str| {
        doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => name.to_owned(), "Encoding" => "WinAnsiEncoding" })
    };
    let regular = font(&mut doc, "Helvetica");
    let bold = font(&mut doc, "Helvetica-Bold");
    let resources = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => regular, "F2" => bold },
        "ExtGState" => dictionary! {
            "Tint" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(0.3) },
            "Fade" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(0.55) },
        },
    });
    let mut kids = Vec::new();
    for ink in inks {
        let content = doc.add_object(Stream::new(dictionary! {}, ink.0));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real((SHEET.0 * PT) as f32), Object::Real((SHEET.1 * PT) as f32)],
            "Contents" => content,
            "Resources" => resources,
        });
        kids.push(Object::from(page));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let mut nums = Vec::new();
    for (i, sheet) in set.sheets.iter().enumerate() {
        nums.push(Object::Integer(i as i64));
        nums.push(Object::Dictionary(dictionary! { "P" => Object::String(sheet.number.as_bytes().to_vec(), StringFormat::Literal) }));
    }
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "PageLabels" => dictionary! { "Nums" => nums } });
    doc.trailer.set("Root", catalog);
    let info = doc.add_object(dictionary! {
        "Title" => Object::String(b"Heron Ridge Estate - sample civil drawing set".to_vec(), StringFormat::Literal),
        "Author" => Object::String(b"Sample Civil (fictional)".to_vec(), StringFormat::Literal),
    });
    doc.trailer.set("Info", info);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("a document in memory saves");
    println!("written: {:.1} MB ({:.1} s)", bytes.len() as f64 / 1e6, started.elapsed().as_secs_f64());

    // Every page's scale: the plans at 1:500 or 1:2500, the sections with
    // their vertical exaggeration, the details at theirs.
    let mut scales = ScaleStore::default();
    let page_box = Rect::from_corners(Pt::new(0.0, 0.0), Pt::new(SHEET.0 * PT, SHEET.1 * PT));
    let keep = |scales: &mut ScaleStore, scale: Scale| {
        scales.find_same(&scale).unwrap_or_else(|| {
            let id = scale.id;
            scales.set_scale(scale);
            id
        })
    };
    let ratio = |r: f64| Scale::from_ratio(ScaleId::new(), r).expect("a positive ratio");
    let exaggerated = |along: f64, up: f64| ratio(along).with_vertical(1000.0 / up * PT, 1.0, 0).expect("a vertical scale");
    let mut pages = Vec::new();
    for (i, sheet) in set.sheets.iter().enumerate() {
        let scale = match &sheet.job {
            Job::Sector(..) | Job::Basin(_) => Some(ratio(500.0)),
            Job::Overall(_) => Some(ratio(2500.0)),
            Job::Typical(_) => Some(ratio(50.0)),
            Job::RoadLong(_) | Job::DrainLong(_) | Job::SewerLong(_) => Some(exaggerated(500.0, 100.0)),
            Job::Cross(_) => Some(exaggerated(200.0, 100.0)),
            Job::Detail(k) => Some(ratio(details::DETAILS[*k].2)),
            _ => None,
        };
        if let Some(scale) = scale {
            let id = keep(&mut scales, scale);
            scales.set_page_scale(i as u32, page_box, id);
            pages.push(i);
        }
        let mut regions: Vec<(String, Bx, Scale)> = drawn[i].viewports.iter().map(|(n, b, r)| (n.to_string(), *b, ratio(*r))).collect();
        if let Job::Basin(_) = sheet.job {
            regions.push(("Sections".into(), (400.0, 62.0, 830.0, 580.0), exaggerated(500.0, 100.0)));
        }
        for (name, (x0, y0, x1, y1), scale) in regions {
            let id = keep(&mut scales, scale);
            let bbox = Rect::from_corners(Pt::new(x0 * PT, y0 * PT), Pt::new(x1 * PT, y1 * PT));
            scales.set_viewport(Viewport { id: ViewportId::new(), page: i as u32, bbox, name, scale: id, whole_page: false });
        }
    }

    let pdfium = worker::bind().expect("pdfium loads");
    let notes_page = set.page_of("C-010");
    let loaded = pdfium.load_pdf_from_byte_slice(&bytes, None).expect("pdfium reads what was written");
    let chars = annots::page_chars(&loaded, notes_page).expect("the notes have text");
    let highlights = [
        ("Services shown are indicative only", [1.0, 0.86, 0.1], "Service locating is booked for Monday. Add what they find to the C-500 series."),
        ("98% standard maximum dry density", [0.45, 0.85, 0.35], "The geotechnical report asks for 100% under the road pavements. Confirm with them."),
        ("class 2 unless noted", [1.0, 0.5, 0.7], "The pipes on the boulevard carry less cover. Check whether they need class 3."),
        ("before its catchment is paved", [0.35, 0.7, 1.0], "The programme has basin 2 after the first road pour. Raise at the site meeting."),
    ];
    let adds = highlights
        .iter()
        .map(|(phrase, color, comment)| {
            let range = selection::find(&chars, phrase).into_iter().next().unwrap_or_else(|| panic!("{phrase:?} is in the notes"));
            NewHighlight { page: notes_page, quads: selection::bands(&chars, range), color: color.map(|c| c as f32), comment: (*comment).into() }
        })
        .collect();
    drop(loaded);

    // On the take-off sector's plan: a box round a junction and an arrow at
    // the park, with a note on each.
    let sid = take_off_sector(&set);
    let ga = set.page_of(&set.sector_sheet(Disc::Ga, sid));
    let v = plans::sector_view(&set.sectors[sid]);
    let red = [0.86, 0.15, 0.15];
    let drawn_markup = |kind, points: Vec<[f32; 2]>, comment: &str| {
        let (xs, ys): (Vec<f32>, Vec<f32>) = points.iter().map(|p| (p[0], p[1])).unzip();
        let min = |q: &[f32]| q.iter().copied().fold(f32::MAX, f32::min) - 3.0;
        let max = |q: &[f32]| q.iter().copied().fold(f32::MIN, f32::max) + 3.0;
        Markup_ {
            key: None,
            page: ga,
            kind,
            bounds: PdfBox { left: min(&xs), bottom: min(&ys), right: max(&xs), top: max(&ys) },
            points,
            color: red,
            width: 2.0,
            style: Default::default(),
            name: String::new(),
            comment: comment.into(),
            author: AUTHOR.into(),
        }
    };
    let corner = |x: f64, y: f64| {
        let p = v.pt(x, y);
        [p.x as f32, p.y as f32]
    };
    // The junction nearest the middle of the sector.
    let middle = centre(set.sectors[sid].bx);
    let (jx, jy) = town
        .roads
        .iter()
        .filter(|r| r.axis == Axis::EastWest)
        .flat_map(|r| r.crossings.iter().map(move |c| (c.0, r.at)))
        .min_by(|a, b| dist(*a, middle).total_cmp(&dist(*b, middle)))
        .expect("a junction");
    let boxed = drawn_markup(DrawKind::Rectangle, vec![corner(jx - 22.0, jy - 22.0), corner(jx + 22.0, jy + 22.0)], "Council wants 8 m kerb returns at this junction, not 6 m. Revise before the next issue.");
    let arrow = drawn_markup(DrawKind::Arrow, vec![corner(middle.0 + 40.0, middle.1 - 50.0), corner(middle.0 + 10.0, middle.1 - 30.0)], "Playground to move north, clear of the boulevard. Landscape architect to confirm.");

    let take = take_off(&set, &town, &drawn);
    let written = take.markups.len();
    let changes = Changes {
        adds,
        markups: vec![boxed, arrow],
        author: AUTHOR.into(),
        scales: Some(ScaleChanges { scales, pages }),
        measures: MeasureChanges { written: take.markups, removed: Vec::new() },
        ..Default::default()
    };
    let saved = annots::save(&pdfium, &bytes, &changes).expect("saves");
    println!("saved with the take-off: {:.1} MB ({:.1} s)", saved.bytes.len() as f64 / 1e6, started.elapsed().as_secs_f64());

    // Read back as the app reads a file: every page there, every
    // measurement a measurement.
    let reread = Document::load_mem(&saved.bytes).expect("lopdf reads what was saved");
    assert_eq!(reread.get_pages().len(), set.sheets.len(), "every sheet is a page");
    let back = pdf_io::read(&reread);
    for (page, why) in &back.skipped {
        eprintln!("page {}: {why}", page + 1);
    }
    assert_eq!(back.markups.len(), written, "every measurement reads back as one");
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("makes the folder");
    }
    std::fs::write(&out, &saved.bytes).expect("writes the file");
    let tools = format!("{}-tools.json", out.trim_end_matches(".pdf"));
    std::fs::write(&tools, tools_json(&take.tools)).expect("writes the tools");
    println!("wrote {out}: {} pages, {:.1} MB, {written} measurements ({:.1} s)", set.sheets.len(), saved.bytes.len() as f64 / 1e6, started.elapsed().as_secs_f64());
    println!("wrote {tools}");

    // `--png` draws a sheet of each kind to a picture beside it, to look
    // over; `--png=all` draws every one.
    let every = std::env::args().any(|a| a == "--png=all");
    if every || std::env::args().any(|a| a == "--png") {
        let doc = pdfium.load_pdf_from_byte_slice(&saved.bytes, None).expect("pdfium reads what was saved");
        let mut kinds = std::collections::HashSet::new();
        for (i, sheet) in set.sheets.iter().enumerate() {
            let kind = match &sheet.job {
                Job::Sector(d, _) => format!("{d:?}"),
                Job::Overall(_) => format!("overall{}", sheet.number),
                Job::Detail(_) | Job::Typical(_) => sheet.number.clone(),
                other => format!("{}", std::mem::discriminant(other).hash_key()),
            };
            let pick = every || i == ga || kinds.insert(kind);
            if !pick {
                continue;
            }
            let t = Instant::now();
            let ([w, h], rgba) = annots::render_page(&doc, i, 1.5).expect("draws");
            let path = format!("{}-{}.png", out.trim_end_matches(".pdf"), sheet.number);
            let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("creates the picture"));
            let mut png = png::Encoder::new(file, w as u32, h as u32);
            png.set_color(png::ColorType::Rgba);
            png.write_header().and_then(|mut wr| wr.write_image_data(&rgba)).expect("writes the picture");
            println!("drew {path} in {:.2} s", t.elapsed().as_secs_f64());
        }
    }
}

/// A key for a job's kind, to draw one of each.
trait HashKey {
    fn hash_key(&self) -> u64;
}

impl HashKey for std::mem::Discriminant<Job> {
    fn hash_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}
