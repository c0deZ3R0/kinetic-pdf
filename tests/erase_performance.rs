//! Run explicitly with --release -- --ignored --nocapture. The `bench` feature
//! enables the new selection measurements; without it this also builds at the
//! pre-erasure-fix revision for an apples-to-apples baseline.
mod common;
use std::{fmt::Write as _, hint::black_box, time::{Duration, Instant}};
use kinetic_pdf::{annots, domain::{Changes, Erasure}, protocol::{Request, Reply}, worker};
use pdf_content::lopdf::{dictionary, Dictionary, Document, Object, Stream};

fn fixture(rows: usize, blocks: usize, uncommon: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let font = if uncommon {
        let mut procedures = Dictionary::new();
        let mut names = vec![65.into()];
        for ch in b'A'..=b'Z' {
            let name = vec![ch];
            let proc = doc.add_object(Stream::new(dictionary! {}, b"500 0 0 0 450 700 d1 0 0 450 700 re f".to_vec()));
            procedures.set(name.clone(), proc);
            names.push(Object::Name(name));
        }
        dictionary! {
            "Type" => "Font", "Subtype" => "Type3", "FontBBox" => vec![0.into(),0.into(),450.into(),700.into()],
            "FontMatrix" => vec![0.001.into(),0.into(),0.into(),0.001.into(),0.into(),0.into()],
            "FirstChar" => 65, "LastChar" => 90, "Widths" => vec![500.into();26],
            "Encoding" => dictionary! { "Differences" => names }, "CharProcs" => procedures, "Resources" => dictionary! {}
        }
    } else { dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" } };
    let mut content = String::new();
    let line: String = (0..80).map(|i| char::from(b'A' + (i % 26) as u8)).collect();
    for column in 0..blocks {
        for row in 0..rows {
            writeln!(content, "BT /F 5.5 Tf {} {} Td ({line}) Tj ET", 20+column*460, 1660-row*12).unwrap();
        }
    }
    let content = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(),0.into(),2384.into(),1684.into()],
        "Contents" => content, "Resources" => dictionary! { "Font" => dictionary! { "F" => font } }
    });
    doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", root);
    let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap(); bytes
}

fn timed(mut f: impl FnMut(), runs: usize) -> (f64, f64) {
    f(); // Exclude DLL/font initialisation and allocator warm-up.
    let mut times = Vec::new();
    for _ in 0..runs { let t = Instant::now(); f(); times.push(t.elapsed().as_secs_f64()*1000.0); }
    times.sort_by(f64::total_cmp);
    (times[times.len()/2], times[(times.len()*95/100).min(times.len()-1)])
}

fn row(label: &str, fixture: &str, operation: &str, times: (f64,f64)) {
    println!("MEASURE,{label},{fixture},{operation},{:.6},{:.6}", times.0, times.1);
}

fn erasures(count: usize) -> Vec<Erasure> {
    (0..count).map(|i| {
        let y = 1659.0 - (i % 50) as f32 * 12.0;
        Erasure { page: 0, layer: None, region: vec![[19.0,y],[51.0,y],[51.0,y+8.0],[19.0,y+8.0]] }
    }).collect()
}

#[test]
#[ignore = "manual performance measurement"]
fn erase_costs() {
    let label = std::env::var("ERASE_BENCH_LABEL").unwrap_or_else(|_| "working".into());
    let pdfium = worker::bind().unwrap();
    let fixtures = [("text-4000",fixture(50,1,false)), ("text-52000",fixture(130,5,false)), ("type3-4000",fixture(50,1,true))];
    println!("MEASURE,label,fixture,operation,median_ms,p95_ms");
    println!("CHAR_BYTES,{label},{}",std::mem::size_of::<kinetic_pdf::domain::TextChar>());
    for (name, bytes) in &fixtures {
        let doc = pdfium.load_pdf_from_byte_slice(bytes,None).unwrap();
        row(&label,name,"load-page-and-extract",timed(|| {
            let page = doc.pages().first().unwrap(); black_box(annots::chars_of(&page).unwrap());
        },21));
        let page = doc.pages().first().unwrap();
        row(&label,name,"extract-loaded-page",timed(|| { black_box(annots::chars_of(&page).unwrap()); },21));
        let parsed = Document::load_mem(bytes).unwrap();
        if !name.starts_with("type3") {
            row(&label,name,"prepare-render-shapes",timed(|| { black_box(gpu_lines::page_shapes(&parsed,1,0.25,1.0).unwrap()); },7));
        }
        #[cfg(feature = "bench")]
        {
            let chars = annots::chars_of(&page).unwrap();
            for count in [0,1,8,32] {
                let erased = erasures(count);
                row(&label,name,&format!("selection-filter-{count}"),timed(|| {
                    for _ in 0..20 { black_box(kinetic_pdf::selection::without_erasures(black_box(&chars),0,black_box(&erased))); }
                },21).map_each(|v| v/20.0));
                let cache = kinetic_pdf::selection::TextCache::default();
                row(&label,name,&format!("selection-cached-{count}"),timed(|| {
                    for _ in 0..100 { black_box(cache.get(black_box(&chars),0,black_box(&erased))); }
                },21).map_each(|v| v/100.0));
            }
        }
    }
    // The real save path, including preparation, file commit and reloading.
    let (tx,rx,_) = common::start_worker();
    let dir = tempfile::tempdir().unwrap();
    let mut generation = 0;
    for (name, bytes) in &fixtures {
        for count in [0,1,8] {
            let mut times = Vec::new();
            for run in 0..8 {
                generation += 1;
                let path = dir.path().join("bench.pdf"); std::fs::write(&path,bytes).unwrap();
                tx.send(Request::Open { generation, path }).unwrap();
                loop { match common::next_reply(&rx) { Reply::Opened { .. } => break, Reply::OpenFailed { error,.. } => panic!("{error}"), _=>{} } }
                let start = Instant::now();
                tx.send(Request::Save { generation, changes: Changes { erasures: erasures(count), ..Default::default() }, arrangement:None,new_pages:vec![] }).unwrap();
                loop { match common::next_reply(&rx) { Reply::Saved { .. } => break, Reply::SaveFailed { error,.. } => panic!("{error}"), _=>{} } }
                if run > 0 { times.push(start.elapsed()); }
            }
            times.sort();
            let ms = |d: Duration| d.as_secs_f64()*1000.0;
            row(&label,name,&format!("save-{count}-erasures"),(ms(times[times.len()/2]),ms(*times.last().unwrap())));
        }
    }
}

#[cfg(feature = "bench")]
trait MapPair { fn map_each(self, f: impl Fn(f64)->f64) -> Self; }
#[cfg(feature = "bench")]
impl MapPair for (f64,f64) { fn map_each(self, f: impl Fn(f64)->f64) -> Self { (f(self.0),f(self.1)) } }
