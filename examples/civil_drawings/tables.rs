//! The sheets that are mostly words and numbers: the cover, the drawing
//! index, the general notes, and the schedules.

use crate::draw::*;
use crate::geom::*;
use crate::sample_ink::{grey, north, wrap, Align, Font, Ink, BLACK};
use crate::town::*;
use crate::{Set, SheetsPer};

/// Rows of a schedule a sheet holds, in each of its two columns.
pub const ROWS: usize = 96;
pub const ROW_H: f64 = 5.0;

/// Two tables side by side, the rows running down the first then the
/// second.
fn two_up(ink: &mut Ink, top: f64, cols: &[(&str, f64)], rows: &[Vec<String>]) {
    let width: f64 = cols.iter().map(|c| c.1).sum();
    for (k, chunk) in rows.chunks(ROWS).enumerate().take(2) {
        table(ink, 30.0 + k as f64 * (width + 12.0), top, cols, chunk, ROW_H, 2.0);
    }
}

pub fn pit_schedule(ink: &mut Ink, set: &Set, town: &Town, pits: &[usize]) {
    let rows: Vec<Vec<String>> = pits
        .iter()
        .map(|&i| {
            let p = &town.pits[i];
            let out = town.pipe_from(i);
            vec![
                p.name.clone(),
                p.kind.name(),
                p.road.map_or("Corridor".into(), |r| town.roads[r].place(p.ch)),
                set.sector_of(p.at).map_or("–".into(), |s| set.sector_sheet(crate::Disc::Drainage, s)),
                format!("{:.3}", p.surface),
                format!("{:.3}", p.invert),
                format!("{:.2}", p.surface - p.invert),
                if p.catchment > 0.0 { format!("{:.3}", p.catchment) } else { "–".into() },
                out.map_or("–".into(), |q| format!("{:.0}", q.q100)),
                out.map_or("–".into(), |q| town.pits[q.to].name.clone()),
            ]
        })
        .collect();
    let cols = [("PIT", 22.0), ("TYPE", 42.0), ("LOCATION", 50.0), ("PLAN", 16.0), ("SURFACE", 20.0), ("INVERT", 20.0), ("DEPTH", 14.0), ("AREA ha", 16.0), ("Q1% L/s", 16.0), ("TO", 20.0)];
    two_up(ink, 560.0, &cols, &rows);
}

pub fn pipe_schedule(ink: &mut Ink, town: &Town, pipes: &[usize]) {
    let rows: Vec<Vec<String>> = pipes
        .iter()
        .map(|&k| {
            let p = &town.pipes[k];
            vec![
                format!("{} – {}", town.pits[p.from].name, town.pits[p.to].name),
                format!("{:.0}Ø RCP", p.dia * 1000.0),
                format!("{:.2}", p.length),
                format!("{:.2}", p.grade * 100.0),
                format!("{:.3}", p.up),
                format!("{:.3}", p.down),
                format!("{:.0}", p.q5),
                format!("{:.0}", p.q100),
                format!("{:.0}", p.capacity),
                format!("{:.2}", p.q5 / 1000.0 / (std::f64::consts::PI * p.dia * p.dia / 4.0)),
            ]
        })
        .collect();
    let cols = [("PIPE", 38.0), ("SIZE", 26.0), ("LENGTH", 18.0), ("GRADE %", 17.0), ("US IL", 20.0), ("DS IL", 20.0), ("Q20%", 16.0), ("Q1%", 16.0), ("FULL", 16.0), ("V m/s", 14.0)];
    two_up(ink, 560.0, &cols, &rows);
}

pub fn manhole_schedule(ink: &mut Ink, set: &Set, town: &Town, mhs: &[(usize, usize)]) {
    let rows: Vec<Vec<String>> = mhs
        .iter()
        .map(|&(s, m)| {
            let sewer = &town.sewers[s];
            let mh = &sewer.manholes[m];
            let depth = mh.surface - mh.invert;
            vec![
                mh.name.clone(),
                format!("{:.3}", EASTING + mh.at.0),
                format!("{:.3}", NORTHING + mh.at.1),
                set.sector_of(mh.at).map_or("–".into(), |x| set.sector_sheet(crate::Disc::WaterSewer, x)),
                format!("{:.3}", mh.surface),
                format!("{:.3}", mh.invert),
                format!("{depth:.2}"),
                if depth > 1.5 { "1050 dia, step irons".into() } else { "1050 dia".into() },
            ]
        })
        .collect();
    let cols = [("MH", 20.0), ("EASTING", 30.0), ("NORTHING", 32.0), ("PLAN", 16.0), ("SURFACE", 20.0), ("INVERT", 20.0), ("DEPTH", 16.0), ("TYPE", 42.0)];
    two_up(ink, 560.0, &cols, &rows);
}

/// Every corner of every lot in a sector, and the lots' own schedule.
pub fn lot_setout(ink: &mut Ink, town: &Town, sector: Bx) {
    let lots: Vec<&Lot> = town.lots.iter().filter(|l| in_box(centre(l.bx), sector)).collect();
    heading(ink, (30.0, 566.0), "LOT CORNERS");
    let mut points = Vec::new();
    for lot in &lots {
        for (k, p) in rect_pts(lot.bx).iter().enumerate() {
            points.push(vec![format!("{}{}", lot.number, ["a", "b", "c", "d"][k]), format!("{:.3}", EASTING + p.0), format!("{:.3}", NORTHING + p.1)]);
        }
    }
    let cols = [("POINT", 17.0), ("EASTING", 29.0), ("NORTHING", 31.0)];
    let per = 104;
    for (k, chunk) in points.chunks(per).enumerate().take(6) {
        table(ink, 30.0 + k as f64 * 80.0, 560.0, &cols, chunk, 4.7, 1.8);
    }
    let x = 30.0 + 6.0 * 80.0 + 6.0;
    heading(ink, (x, 566.0), "LOT SCHEDULE");
    let rows: Vec<Vec<String>> = lots
        .iter()
        .map(|l| vec![l.number.to_string(), format!("{:.0}", l.area), format!("{:.2}", l.frontage()), format!("{:.2}", l.depth()), format!("{:.2}", l.fsl), format!("{}", l.stage)])
        .collect();
    let cols = [("LOT", 14.0), ("m²", 14.0), ("FRONT", 16.0), ("DEPTH", 16.0), ("FSL", 18.0), ("ST", 10.0)];
    for (k, chunk) in rows.chunks(104).enumerate().take(3) {
        table(ink, x + k as f64 * 91.0, 560.0, &cols, chunk, 4.7, 1.8);
    }
}

pub fn index_sheet(ink: &mut Ink, set: &Set, part: usize, per: SheetsPer) {
    heading(ink, (30.0, 566.0), "DRAWING INDEX");
    let start = part * per.0;
    let rows: Vec<Vec<String>> = set.sheets.iter().skip(start).take(per.0).map(|s| vec![s.number.clone(), s.title.join(" "), s.scale.clone(), "C".into()]).collect();
    let cols = [("No.", 20.0), ("TITLE", 280.0), ("SCALE", 50.0), ("REV", 12.0)];
    for (k, chunk) in rows.chunks(per.1).enumerate().take(2) {
        table(ink, 30.0 + k as f64 * 400.0, 560.0, &cols, chunk, 4.9, 2.0);
    }
}

pub const NOTES: &[(&str, &[&str])] = &[
    ("GENERAL", &[
        "Read these drawings with the specification and the geotechnical report. Report any discrepancy to the engineer before going on.",
        "Do not scale from the drawings. Dimensions are in metres and levels in metres above the local datum unless noted.",
        "Locate and protect every existing service before digging. Services shown are indicative only.",
        "Set out from the survey marks on the cover. Have a registered surveyor confirm them before work starts.",
        "The works are staged as the staging plan. Each stage is to be complete and stable before the next begins.",
    ]),
    ("EARTHWORKS", &[
        "Strip topsoil 100 deep and stockpile it on site for the verges and the basins' batters.",
        "Place fill in layers no thicker than 200, compacted to 98% standard maximum dry density.",
        "Batters no steeper than 1 in 4 unless noted. Turf every batter as soon as it is finished.",
        "Lots are benched to the levels on the finished level plans, within 50 either way.",
        "Retaining walls where shown, built before the lots either side are filled.",
    ]),
    ("ROADWORKS", &[
        "Pavements as the typical sections. Proof roll the subgrade and replace soft spots as the engineer directs.",
        "Kerb and gutter to the kerb details, with kerb ramps at every intersection and crossing point.",
        "Subsoil drains under every kerb, discharging to the nearest pit.",
        "Driveway crossings to each lot as shown, 3.0 wide at the boundary.",
    ]),
    ("STORMWATER", &[
        "Pipes are reinforced concrete, class 2 unless noted, with rubber ring joints, bedded to HS2.",
        "Cover to pipes no less than 600 under roads and 450 elsewhere.",
        "Bench every pit to half the outlet's diameter, shaped to the flow.",
        "Build each basin's outlet and spillway before its catchment is paved.",
    ]),
    ("SERVICES", &[
        "Water and sewer to the water authority's approved design; the layout shown is for coordination.",
        "Electricity, communications and gas in the shared trench allocations on the typical sections.",
        "Conduits under the road at every lot corner, 600 below the finished surface.",
    ]),
];

pub fn notes_sheet(ink: &mut Ink) {
    heading(ink, (30.0, 566.0), "GENERAL NOTES");
    let size = 3.2;
    let leading = 4.6;
    let width = 240.0;
    let columns = [30.0, 300.0, 570.0];
    let top = 552.0;
    let (mut col, mut y, mut n) = (0, top, 1);
    for (head, notes) in NOTES {
        let needed = 7.0 + notes.iter().map(|s| wrap(s, size, Font::Regular, width - 8.0).len() as f64 * leading + 2.0).sum::<f64>();
        if y - needed < 90.0 && col + 1 < columns.len() {
            col += 1;
            y = top;
        }
        let x = columns[col];
        ink.fill(BLACK);
        ink.text((x, y), 3.8, Font::Bold, Align::Left, head);
        y -= 7.0;
        for note in *notes {
            ink.text((x, y), size, Font::Regular, Align::Left, &format!("{n}."));
            let lines = wrap(note, size, Font::Regular, width - 8.0);
            y = ink.lines((x + 8.0, y), size, leading, Font::Regular, &lines) - 2.0;
            n += 1;
        }
        y -= 4.0;
    }
}

pub fn cover_sheet(ink: &mut Ink, set: &Set, town: &Town) {
    ink.fill(BLACK);
    ink.text((32.0, 552.0), 16.0, Font::Bold, Align::Left, "HERON RIDGE ESTATE");
    ink.text((32.0, 536.0), 8.0, Font::Regular, Align::Left, "STAGES 1 TO 6  ·  SUBDIVISION WORKS");
    ink.fill(grey(0.35));
    ink.text((32.0, 526.0), 4.2, Font::Regular, Align::Left, "CIVIL ENGINEERING DRAWINGS  ·  ISSUE C  ·  SEPTEMBER 2026");
    ink.fill(BOUNDARY);
    ink.rect(32.0, 518.0, 60.0, 1.4, "f");
    locality(ink, (32.0, 96.0, 430.0, 500.0));
    let x = 470.0;
    heading(ink, (x, 552.0), "THIS SET");
    let rows = vec![
        vec!["Drawings".into(), format!("{}", set.sheets.len())],
        vec!["Drawing index".into(), set.range_of("DRAWING INDEX")],
        vec!["General notes".into(), set.range_of("GENERAL NOTES")],
        vec!["Lots".into(), format!("{}", town.lots.len())],
        vec!["Roads".into(), format!("{} ({:.2} km)", town.roads.len(), town.roads.iter().map(|r| r.length()).sum::<f64>() / 1000.0)],
        vec!["Stormwater pits".into(), format!("{}", town.pits.len())],
        vec!["Sewer manholes".into(), format!("{}", town.sewers.iter().map(|s| s.manholes.len()).sum::<usize>())],
        vec!["Basins".into(), format!("{}", town.basins.len())],
    ];
    let bottom = table(ink, x, 546.0, &[("", 90.0), ("", 110.0)], &rows, 6.0, 2.6);
    heading(ink, (x, bottom - 14.0), "SERIES");
    let series = [
        ("C-000", "General: cover, index, notes, overall plans"),
        ("C-100", "General arrangement and setout"),
        ("C-200", "Earthworks and finished levels"),
        ("C-300", "Roads: typical, long and cross sections"),
        ("C-400", "Stormwater and basins"),
        ("C-500", "Water and sewer"),
        ("C-600", "Electrical and lighting"),
        ("C-700", "Signs, linemarking and landscape"),
        ("C-800", "Details"),
    ];
    let rows: Vec<Vec<String>> = series.iter().map(|(a, b)| vec![(*a).into(), (*b).into()]).collect();
    let bottom = table(ink, x, bottom - 20.0, &[("SERIES", 30.0), ("", 250.0)], &rows, 6.0, 2.6);
    heading(ink, (x, bottom - 14.0), "SURVEY CONTROL");
    let rows = vec![
        vec!["PM 1".into(), format!("{:.3}", EASTING + 12.418), format!("{:.3}", NORTHING + 20.106), format!("{:.3}", ground(12.4, 20.1)), "Kerb, Sampleton Rd".into()],
        vec!["PM 2".into(), format!("{:.3}", EASTING + 588.905), format!("{:.3}", NORTHING + 431.772), format!("{:.3}", ground(588.9, 431.8)), "Star picket, park".into()],
        vec!["PM 3".into(), format!("{:.3}", EASTING + 1096.31), format!("{:.3}", NORTHING + 208.461), format!("{:.3}", ground(1096.3, 208.5)), "Concrete block, corridor".into()],
    ];
    table(ink, x, bottom - 20.0, &[("MARK", 20.0), ("EASTING", 36.0), ("NORTHING", 38.0), ("RL", 22.0), ("DESCRIPTION", 70.0)], &rows, 6.0, 2.4);
}

fn locality(ink: &mut Ink, b: Bx) {
    let (x0, y0, x1, y1) = b;
    let p = |u: f64, w: f64| (x0 + u * (x1 - x0), y0 + w * (y1 - y0));
    ink.fill([0.97, 0.96, 0.93]);
    ink.rect(x0, y0, x1 - x0, y1 - y0, "f");
    ink.clip(&rect_pts(b));
    let mut rng = Rng(0xa11e_4a1d);
    let mut streets: Vec<Vec<(f64, f64)>> = Vec::new();
    for i in 0..15 {
        let u = 0.03 + i as f64 * 0.07 + (rng.next() - 0.5) * 0.02;
        streets.push(vec![p(u, 0.0), p(u + (rng.next() - 0.5) * 0.04, 1.0)]);
    }
    for i in 0..15 {
        let w = 0.04 + i as f64 * 0.068 + (rng.next() - 0.5) * 0.02;
        streets.push(vec![p(0.0, w), p(1.0, w + (rng.next() - 0.5) * 0.05)]);
    }
    for (width, colour) in [(2.2, grey(0.78)), (1.5, [1.0, 1.0, 1.0])] {
        ink.pen(width, colour);
        for s in &streets {
            ink.poly(s, false, "S");
        }
    }
    ink.fill([0.82, 0.92, 0.78]);
    ink.poly(&[p(0.08, 0.62), p(0.3, 0.6), p(0.33, 0.8), p(0.1, 0.83)], true, "f");
    let creek: Vec<(f64, f64)> = (0..=60).map(|i| i as f64 / 60.0).map(|t| p(0.8 + 0.04 * (t * 9.0).sin() + 0.06 * t, 1.0 - t)).collect();
    ink.pen(4.0, [0.62, 0.8, 0.95]);
    ink.poly(&creek, false, "S");
    let rail = [p(0.0, 0.2), p(1.0, 0.33)];
    ink.pen(0.6, BLACK);
    ink.poly(&rail, false, "S");
    for i in 1..40 {
        let t = i as f64 / 40.0;
        let c = (rail[0].0 + (rail[1].0 - rail[0].0) * t, rail[0].1 + (rail[1].1 - rail[0].1) * t);
        ink.line((c.0 - 0.4, c.1 - 1.6), (c.0 + 0.4, c.1 + 1.6));
    }
    let main = [p(0.31, 0.0), p(0.33, 0.5), p(0.3, 1.0)];
    for (width, colour) in [(4.6, grey(0.5)), (3.6, [1.0, 0.9, 0.62])] {
        ink.pen(width, colour);
        ink.poly(&main, false, "S");
    }
    let site = [p(0.34, 0.38), p(0.78, 0.38), p(0.78, 0.66), p(0.34, 0.66)];
    ink.save();
    ink.op("/Tint gs");
    ink.fill(BOUNDARY);
    ink.poly(&site, true, "f");
    ink.restore();
    ink.pen(1.0, BOUNDARY);
    ink.poly(&site, true, "S");
    ink.restore();
    ink.pen(0.5, BLACK);
    ink.rect(x0, y0, x1 - x0, y1 - y0, "S");
    tag(ink, p(0.2, 0.71), 0.0, 3.0, Font::Bold, grey(0.25), "SAMPLETON PARK");
    tag(ink, p(0.33, 0.9), 1.55, 3.2, Font::Bold, grey(0.2), "SAMPLETON ROAD");
    tag(ink, p(0.2, 0.228), 0.12, 2.8, Font::Bold, grey(0.2), "RAILWAY");
    tag(ink, p(0.86, 0.25), 1.4, 3.0, Font::Bold, [0.1, 0.4, 0.7], "KINGFISHER CREEK");
    callout(ink, p(0.56, 0.52), p(0.6, 0.8), 3.4, &["HERON RIDGE ESTATE", "THE SITE"]);
    north(ink, p(0.92, 0.1), 8.0);
    ink.fill(BLACK);
    ink.text((x0, y0 - 9.0), 5.0, Font::Bold, Align::Left, "LOCALITY PLAN");
    ink.text((x0, y0 - 15.0), 2.8, Font::Regular, Align::Left, "NOT TO SCALE");
}
