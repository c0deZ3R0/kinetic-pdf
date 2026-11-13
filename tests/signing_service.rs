//! Real same-executable signing-service integration, using fresh synthetic identities.
//!
//! PATH: tests/signing_service.rs
//! CONSUMED BY: cargo test --test signing_service on Windows. No fixture keys.
//! Final multi-page blocks are stamps plus one signature widget under one P1
//! seal; native PDFium raster checks include inherited crops, rotations and
//! actual emitted font-size growth between small and large blocks.
//! Optional KINETIC_SIGNING_OPENSSL / KINETIC_SIGNING_PYTHON developer oracles
//! verify integrity only; they are never shipping-helper commands or trust proof.

#![cfg(windows)]
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use pdf_content::lopdf::{dictionary, Document, IncrementalDocument, Object};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

static ID: AtomicU64 = AtomicU64::new(1);
fn digest(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() }
fn invoke(raw: Vec<u8>) -> (std::process::ExitStatus, Vec<u8>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kinetic-pdf")).arg("--signature-service")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let raw = Zeroizing::new(raw);
    let writer = std::thread::spawn(move || { let _ = stdin.write_all(&raw); });
    let reader = std::thread::spawn(move || { let mut out = vec![]; stdout.by_ref().take(65537).read_to_end(&mut out).unwrap(); out });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() { break status; }
        if start.elapsed() > Duration::from_secs(120) { let _ = child.kill(); let _ = child.wait(); panic!("owned signing test child timed out"); }
        std::thread::sleep(Duration::from_millis(10));
    };
    child.wait().unwrap(); writer.join().unwrap();
    (status, reader.join().unwrap())
}
fn call(task: Value) -> Value {
    let id = ID.fetch_add(1, Ordering::Relaxed);
    let (status, bytes) = invoke(serde_json::to_vec(&json!({"version":kinetic_pdf::signing::PROTOCOL_VERSION,"id":id,"task":task})).unwrap());
    assert!(status.success(), "helper exited {status}");
    assert!(bytes.len() <= 65536);
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["version"], kinetic_pdf::signing::PROTOCOL_VERSION);
    assert_eq!(response["id"], id);
    response["result"].clone()
}
fn plain() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let content = doc.add_object(pdf_content::lopdf::Stream::new(dictionary! {}, b"BT /F1 12 Tf 50 700 Td (ORIGINAL synthetic text) Tj ET".to_vec()));
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0),Object::Integer(0),Object::Integer(612),Object::Integer(792)],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }, "Contents" => content });
    doc.objects.insert(pages,Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }));
    let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root",root);
    let mut out=vec![]; doc.save_to(&mut out).unwrap(); out
}
fn annotate(bytes: Vec<u8>) -> Vec<u8> {
    let doc=Document::load_mem(&bytes).unwrap();
    let page=*doc.get_pages().values().next().unwrap();
    let mut update=IncrementalDocument::create_from(bytes,doc);
    let note=update.new_document.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Rect" => vec![Object::Integer(20);4], "Contents" => Object::string_literal("UNSAVED? No, persisted current annotation") });
    update.opt_clone_object_to_new_document(page).unwrap();
    let mut annots=update.new_document.get_dictionary(page).unwrap().get(b"Annots").ok().and_then(|v|v.as_array().ok()).cloned().unwrap_or_default();
    annots.push(Object::Reference(note)); update.new_document.get_dictionary_mut(page).unwrap().set("Annots",annots);
    let mut out=vec![]; update.save_to(&mut out).unwrap(); out
}
fn cropped_second_page(rotation: u16, unit: f32) -> Vec<u8> {
    let mut doc=Document::load_mem(&plain()).unwrap();
    let pages=doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
    let first=*doc.get_pages().values().next().unwrap();
    doc.get_dictionary_mut(first).unwrap().set("CropBox",vec![Object::Integer(0),Object::Integer(0),Object::Integer(612),Object::Integer(792)]);
    doc.get_dictionary_mut(first).unwrap().set("Rotate",0);
    let second=doc.add_object(dictionary! { "Type"=>"Page", "Parent"=>pages, "Resources"=>dictionary! {}, "UserUnit"=>Object::Real(unit) });
    let parent=doc.get_dictionary_mut(pages).unwrap();
    parent.set("Kids",vec![Object::Reference(first),Object::Reference(second)]); parent.set("Count",2);
    parent.set("MediaBox",vec![Object::Integer(-40),Object::Integer(-20),Object::Integer(600),Object::Integer(800)]);
    parent.set("CropBox",vec![Object::Integer(20),Object::Integer(30),Object::Integer(650),Object::Integer(760)]);
    parent.set("Rotate",i64::from(rotation));
    let mut out=vec![]; doc.save_to(&mut out).unwrap(); out
}
fn multipage_source() -> Vec<u8> {
    let mut doc=Document::with_version("1.7"); let root_pages=doc.new_object_id(); let mut kids=vec![]; let mut notes=vec![];
    for rotation in [0,90,180,270] {
        let branch=doc.new_object_id(); let page=doc.new_object_id();
        let note=doc.add_object(dictionary! { "Type"=>"Annot", "Subtype"=>"Text", "P"=>page, "F"=>4,
            "Rect"=>[30,35,45,50].map(Object::Integer).to_vec(), "Contents"=>Object::string_literal("Preserve original source note"),
            "NM"=>Object::string_literal(format!("OriginalNote-{rotation}")), "Name"=>"Note" });
        notes.push(note);
        let annotations=Object::Array(vec![Object::Reference(note)]);
        let annotations=if rotation%180==0 { Object::Reference(doc.add_object(annotations)) } else { annotations };
        doc.objects.insert(page,Object::Dictionary(dictionary! { "Type"=>"Page", "Parent"=>branch, "Resources"=>dictionary! {}, "Annots"=>annotations }));
        doc.objects.insert(branch,Object::Dictionary(dictionary! { "Type"=>"Pages", "Parent"=>root_pages, "Count"=>1, "Kids"=>vec![Object::Reference(page)],
            "MediaBox"=>[-40,-20,600,800].map(Object::Integer).to_vec(), "CropBox"=>[20,30,650,760].map(Object::Integer).to_vec(), "Rotate"=>rotation, "UserUnit"=>1 }));
        kids.push(Object::Reference(branch));
    }
    doc.objects.insert(root_pages,Object::Dictionary(dictionary! { "Type"=>"Pages", "Count"=>4, "Kids"=>kids }));
    let root=doc.add_object(dictionary! { "Type"=>"Catalog", "Pages"=>root_pages }); doc.trailer.set("Root",root);
    // Deliberately collide with the first generated block's preferred name.
    let collision=format!("KineticSignatureBlock-{}-0-0",doc.max_id+1);
    doc.get_dictionary_mut(notes[0]).unwrap().set("NM",Object::string_literal(collision));
    let mut out=vec![]; doc.save_to(&mut out).unwrap(); out
}
fn annotation_ids(doc: &Document, page: pdf_content::lopdf::ObjectId) -> Vec<pdf_content::lopdf::ObjectId> {
    let value=doc.get_dictionary(page).unwrap().get(b"Annots").unwrap();
    let value=match value { Object::Reference(id)=>doc.get_object(*id).unwrap(), _=>value };
    value.as_array().unwrap().iter().map(|v|v.as_reference().unwrap()).collect()
}
fn raster(bytes: &[u8], index: usize) -> (usize,usize,Vec<u8>,kinetic_pdf::domain::PageGeometry) {
    use pdfium_render::prelude::*;
    let pdfium=kinetic_pdf::worker::bind().unwrap();
    let doc=pdfium.load_pdf_from_byte_slice(bytes,None).unwrap();
    let page=doc.pages().get(index as PdfPageIndex).unwrap();
    let geometry=kinetic_pdf::annots::page_geometry(&page);
    let bitmap=page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(1.0).render_annotations(true).render_form_data(true)).unwrap();
    (bitmap.width() as usize,bitmap.height() as usize,bitmap.as_rgba_bytes(),geometry)
}
fn appearance_text(doc: &Document, appearance: &pdf_content::lopdf::Stream) -> String {
    use std::collections::HashMap;
    let fonts=appearance.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap();
    let mut mappings=HashMap::new();
    for (key,value) in fonts.iter() {
        let font=doc.get_object(value.as_reference().unwrap()).unwrap().as_dict().unwrap();
        assert_eq!(font.get(b"Subtype").unwrap().as_name().unwrap(),b"Type0");
        let cmap=doc.get_object(font.get(b"ToUnicode").unwrap().as_reference().unwrap()).unwrap().as_stream().unwrap();
        let bytes=cmap.decompressed_content().unwrap_or_else(|_|cmap.content.clone());
        let mut codes=HashMap::new();
        for line in String::from_utf8(bytes).unwrap().lines() {
            let words=line.split_whitespace().collect::<Vec<_>>();
            if words.len()==2 && words[0].starts_with('<') && words[1].starts_with('<') {
                let cid=u16::from_str_radix(words[0].trim_matches(['<','>']),16).unwrap();
                let unicode=words[1].trim_matches(['<','>']);
                let units=unicode.as_bytes().chunks_exact(4).map(|p|u16::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect::<Vec<_>>();
                codes.insert(cid,String::from_utf16(&units).unwrap());
            }
        }
        mappings.insert(key.clone(),codes);
    }
    let operations=pdf_content::lopdf::content::Content::decode(&appearance.content).unwrap();
    let mut current=vec![]; let mut text=String::new();
    for op in operations.operations {
        if op.operator=="Tf" { current=op.operands[0].as_name().unwrap().to_vec(); }
        if op.operator=="Tj" {
            for code in op.operands[0].as_str().unwrap().chunks_exact(2) {
                text.push_str(&mappings[&current][&u16::from_be_bytes([code[0],code[1]])]);
            }
        }
    }
    text
}
fn multipage_round_trip(dir: &Path, pfx: &Path, password: &str, actual_subject: &str) {
    use std::collections::HashSet;
    let original=multipage_source(); let source=dir.join("multi-source.pdf"); std::fs::write(&source,&original).unwrap();
    let mut placements=vec![];
    for second in [false,true] { for page in 0..4 {
        let rotation=page as u16*90;
        let rect=match (rotation%180==0,second) {
            (true,false)=>[60.0f32,80.0,300.0,190.0],
            (false,false)=>[60.0,80.0,170.0,320.0],
            (_,true)=>[180.0,340.0,580.0,740.0],
        };
        placements.push((page,rect,rotation));
    } }
    let blocks=placements.iter().map(|(page,rect,rotation)|json!({"page":page,"rect":rect,"rotation":rotation})).collect::<Vec<_>>();
    let reason="Final blocks — résumé Δοκιμή"; let output=dir.join("multi-certified.pdf");
    let signed=call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":output,"pfx":pfx,"password":password,
        "options":{"mode":"certify_no_changes","reason":reason,"appearances":blocks}}));
    assert!(signed.get("Ok").is_some(),"multi-page P1: {signed}");
    assert_eq!(signed["Ok"]["report"]["signatures"].as_array().unwrap().len(),1);
    assert_eq!(signed["Ok"]["report"]["signatures"][0]["certification"],1);
    let bytes=std::fs::read(&output).unwrap(); assert!(bytes.starts_with(&original)); assert_eq!(std::fs::read(&source).unwrap(),original);
    let before=Document::load_mem(&original).unwrap(); let parsed=Document::load_mem(&bytes).unwrap();
    let signatures=parsed.objects.iter().filter_map(|(id,o)|o.as_dict().ok().filter(|d|d.get(b"Type").ok().and_then(|o|o.as_name().ok())==Some(b"Sig")).map(|d|(*id,d))).collect::<Vec<_>>();
    let widgets=parsed.objects.iter().filter_map(|(id,o)|o.as_dict().ok().filter(|d|d.get(b"FT").ok().and_then(|o|o.as_name().ok())==Some(b"Sig")).map(|d|(*id,d))).collect::<Vec<_>>();
    assert_eq!(signatures.len(),1,"one CMS dictionary, not one per visual block"); assert_eq!(widgets.len(),1);
    let (signature_id,signature)=signatures[0]; let (widget_id,widget)=widgets[0];
    assert_eq!(widget.get(b"V").unwrap().as_reference().unwrap(),signature_id);
    let form=parsed.get_object(parsed.catalog().unwrap().get(b"AcroForm").unwrap().as_reference().unwrap()).unwrap().as_dict().unwrap();
    assert_eq!(form.get(b"Fields").unwrap().as_array().unwrap(),&vec![Object::Reference(widget_id)]);
    assert_eq!(parsed.catalog().unwrap().get(b"Perms").unwrap().as_dict().unwrap().get(b"DocMDP").unwrap().as_reference().unwrap(),signature_id);
    let m=String::from_utf8(signature.get(b"M").unwrap().as_str().unwrap().to_vec()).unwrap();
    let date=chrono::NaiveDateTime::parse_from_str(m.trim_start_matches("D:"),"%Y%m%d%H%M%SZ").unwrap().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut aps=vec![]; let mut names=HashSet::new(); let mut fonts=HashSet::new(); let mut reference_ink:[Option<Vec<bool>>;2]=[None,None]; let mut emitted_sizes=[0.0f32;8];
    for page_index in 0..4 {
        let page=*parsed.get_pages().values().nth(page_index).unwrap();
        let annotations=annotation_ids(&parsed,page); let old=annotation_ids(&before,page);
        assert_eq!(&annotations[..old.len()],old.as_slice(),"preserve original direct/indirect Annots");
        assert_eq!(annotations.len(),old.len()+2,"both repeated-page blocks must survive");
        for old_id in old { assert!(names.insert(parsed.get_dictionary(old_id).unwrap().get(b"NM").unwrap().as_str().unwrap().to_vec())); }
        let mut rectangles=vec![];
        let (w,h,base,geometry)=raster(&original,page_index); let (sw,sh,image,_)=raster(&bytes,page_index); assert_eq!((w,h),(sw,sh));
        assert_eq!([geometry.bounds.left,geometry.bounds.bottom,geometry.bounds.right,geometry.bounds.top],[20.0,30.0,600.0,760.0]);
        assert_eq!(u16::from(geometry.rotation)*90,page_index as u16*90);
        for (which,annotation_id) in annotations.iter().skip(1).enumerate() {
            let index=page_index+which*4; let annotation=parsed.get_dictionary(*annotation_id).unwrap();
            assert!(names.insert(annotation.get(b"NM").unwrap().as_str().unwrap().to_vec()),"native annotation names must be unique, including source collisions");
            assert_eq!(annotation.get(b"P").unwrap().as_reference().unwrap(),page);
            assert_eq!(annotation.get(b"F").unwrap().as_i64().unwrap(),132);
            if index==7 {
                assert_eq!(*annotation_id,widget_id); assert_eq!(annotation.get(b"Subtype").unwrap().as_name().unwrap(),b"Widget");
            } else {
                assert_eq!(annotation.get(b"Subtype").unwrap().as_name().unwrap(),b"Stamp");
                assert!(!annotation.has(b"FT") && !annotation.has(b"V") && !annotation.has(b"T"),"ordinary final visuals are not fake signature fields");
                assert_eq!(annotation.get(b"Name").unwrap().as_name().unwrap(),annotation.get(b"NM").unwrap().as_str().unwrap());
            }
            let ap=annotation.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap(); aps.push(ap);
            let appearance=parsed.get_object(ap).unwrap().as_stream().unwrap(); let text=appearance_text(&parsed,appearance);
            assert!(text.contains(actual_subject) && text.contains(reason) && text.contains(&date),"every final AP has the same real signer, reason and signing instant");
            assert!(text.starts_with("Digitally certified"));
            let operations=pdf_content::lopdf::content::Content::decode(&appearance.content).unwrap();
            emitted_sizes[index]=operations.operations.iter().filter(|op|op.operator=="Tf").map(|op|match op.operands[1] {
                Object::Real(size)=>size, Object::Integer(size)=>size as f32, _=>panic!("numeric emitted font size"),
            }).fold(0.0,f32::max);
            assert!((6.0..=48.01).contains(&emitted_sizes[index]));
            for (_,font) in appearance.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().iter() { fonts.insert(font.as_reference().unwrap()); }
            let rect=placements[index].1; let q=kinetic_pdf::domain::PdfBox {left:rect[0],bottom:rect[1],right:rect[2],top:rect[3]};
            let (l,t,r,b)=geometry.box_to_view(&q);
            let roi=((l*w as f32).round() as usize,(t*h as f32).round() as usize,(r*w as f32).round() as usize,(b*h as f32).round() as usize);
            let (l,t,r,b)=roi; assert_eq!((r-l,b-t),if which==0 {(240,110)} else {(400,400)}); rectangles.push(roi);
            let mut ink=vec![]; let mut changed=0;
            for y in t..b { for x in l..r { let at=(y*w+x)*4;
                ink.push(image[at..at+3].iter().map(|v|u32::from(*v)).sum::<u32>()<3*170);
                if base[at..at+3].iter().zip(&image[at..at+3]).any(|(a,b)|a.abs_diff(*b)>3) { changed+=1; }
            } }
            assert!(changed>1000 && ink.iter().filter(|v|**v).count()>300,"each stamp and final widget must really render");
            if let Some(reference)=&reference_ink[which] {
                let overlap=reference.iter().zip(&ink).filter(|(a,b)|**a && **b).count();
                let total=reference.iter().filter(|v|**v).count()+ink.iter().filter(|v|**v).count();
                assert!(2.0*overlap as f64/total as f64>0.75,"all native stamp/widget text must remain upright: block {index}");
            } else { reference_ink[which]=Some(ink); }
            eprintln!("Native multi-block raster PASS: block {index}, page {}, rotation {}, changed pixels {changed}, emitted font {:.4} pt",page_index+1,u16::from(geometry.rotation)*90,emitted_sizes[index]);
        }
        for y in 0..h { for x in 0..w { let at=(y*w+x)*4;
            if base[at..at+3].iter().zip(&image[at..at+3]).any(|(a,b)|a.abs_diff(*b)>3) {
                assert!(rectangles.iter().any(|&(l,t,r,b)|x+1>=l && x<=r+1 && y+1>=t && y<=b+1),"multi-block AP escaped a selected rect on page {} at ({x},{y})",page_index+1);
            }
        } }
    }
    assert_eq!(aps.len(),8); assert_eq!(fonts.len(),1,"same final text shares embedded font resources across differently sized blocks within this signing operation");
    for page in 0..4 { assert!(emitted_sizes[page+4]>emitted_sizes[page]*1.5,"actual PDF detail text must grow on larger boxes at every rotation"); }
    oracle(&output,&bytes,dir);
    for (index,ap) in aps.into_iter().enumerate() {
        let offset=match parsed.reference_table.entries.get(&ap.0).unwrap() { pdf_content::lopdf::xref::XrefEntry::Normal {offset,..}=>*offset as usize,_=>panic!("normal AP object") };
        let relative=bytes[offset..].windows(5).position(|w|w==b"0.985").unwrap(); let mut corrupted=bytes.clone(); corrupted[offset+relative+3]=b'7';
        assert_eq!(verify(&dir.join(format!("corrupt-multi-{index}.pdf")),&corrupted)["Ok"]["report"]["signatures"][0]["integrity_valid"],false,"every stamp/widget AP is covered by the sole CMS");
    }
    let refused=dir.join("multi-reseal-refused.pdf");
    assert!(call(json!({"kind":"sign_pdf","input":output,"input_sha256":digest(&bytes),"output":refused,"pfx":pfx,"password":password,
        "options":{"mode":"certify_no_changes","reason":reason,"appearances":blocks}})).get("Err").is_some()); assert!(!refused.exists());
    for case in ["ambiguous","over-limit","bad-middle"] {
        let mut options=json!({"mode":"certify_no_changes","reason":reason,"appearances":blocks});
        match case {
            "ambiguous"=>options["appearance"]=blocks[7].clone(),
            "over-limit"=>options["appearances"]=json!(vec![blocks[0].clone();257]),
            _=>options["appearances"][3]["rotation"]=json!(1),
        }
        let refused=dir.join(format!("multi-{case}-refused.pdf"));
        assert!(call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":refused,"pfx":pfx,"password":password,"options":options})).get("Err").is_some(),"{case} must be rejected");
        assert!(!refused.exists()); assert_eq!(std::fs::read(&source).unwrap(),original);
    }
}
fn verify(path: &Path, bytes: &[u8]) -> Value {
    std::fs::write(path,bytes).unwrap();
    call(json!({"kind":"verify_pdf","input":path,"input_sha256":digest(bytes)}))
}
fn oracle(path: &Path, bytes: &[u8], dir: &Path) {
    let doc=Document::load_mem(bytes).unwrap();
    let sig=doc.objects.values().filter_map(|v|v.as_dict().ok()).find(|d|d.get(b"Type").ok().and_then(|v|v.as_name().ok())==Some(b"Sig")).unwrap();
    let range=sig.get(b"ByteRange").unwrap().as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as usize).collect::<Vec<_>>();
    let mut data=bytes[..range[1]].to_vec(); data.extend_from_slice(&bytes[range[2]..range[2]+range[3]]);
    let contents=sig.get(b"Contents").unwrap().as_str().unwrap();
    let (header,len)=if contents[1]<128 {(2,contents[1] as usize)} else {let n=(contents[1]&127) as usize; (2+n,contents[2..2+n].iter().fold(0usize,|n,b|n*256+*b as usize))};
    let cms=dir.join("oracle.der"); let content=dir.join("oracle-content.bin");
    std::fs::write(&cms,&contents[..header+len]).unwrap(); std::fs::write(&content,&data).unwrap();
    if let Some(exe)=std::env::var_os("KINETIC_SIGNING_OPENSSL") {
        let output=Command::new(exe).args(["cms","-verify","-binary","-inform","DER","-noverify","-in"]).arg(&cms).arg("-content").arg(&content).arg("-out").arg(dir.join("oracle-verified.bin")).output().unwrap();
        assert!(output.status.success(),"independent OpenSSL CMS integrity failed: {}",String::from_utf8_lossy(&output.stderr));
        eprintln!("Independent OpenSSL CMS integrity PASS (trust not checked)");
    } else { eprintln!("OpenSSL oracle not run: KINETIC_SIGNING_OPENSSL unset"); }
    if let Some(python)=std::env::var_os("KINETIC_SIGNING_PYTHON") {
        let code="import sys; from pyhanko.pdf_utils.reader import PdfFileReader; from pyhanko.sign.validation import validate_pdf_signature; from pyhanko_certvalidator import ValidationContext; r=PdfFileReader(open(sys.argv[1],'rb')); assert len(r.embedded_signatures)==1; s=validate_pdf_signature(r.embedded_signatures[0], signer_validation_context=ValidationContext(allow_fetching=False,revocation_mode='soft-fail')); assert s.intact and s.valid; print('Independent PyHanko integrity PASS; trust not checked')";
        let output=Command::new(python).arg("-c").arg(code).arg(path).output().unwrap();
        assert!(output.status.success(),"independent PyHanko oracle failed: {}",String::from_utf8_lossy(&output.stderr));
        eprintln!("{}",String::from_utf8_lossy(&output.stdout));
    } else { eprintln!("PyHanko oracle not run: KINETIC_SIGNING_PYTHON unset"); }
}

#[test]
fn native_create_import_approval_p1_history_tamper_and_refusal_round_trip() {
    let dir=tempfile::Builder::new().prefix("kinetic-signing-").tempdir().unwrap();
    let pfx=dir.path().join("identity.pfx");
    let password="Runtime-only integration password 92!";
    let made=call(json!({"kind":"create_pfx","spec":{"name":"Renée Δοκιμή Жуков","organization":"Kinetic test","email":"test@example.invalid","valid_years":1},"password":password,"output":pfx}));
    assert!(made.get("Ok").is_some(),"identity creation: {made}");
    assert!(pfx.exists());
    let actual_subject=made["Ok"]["info"]["subject"].as_str().unwrap();
    assert!(actual_subject.contains("Renée Δοκιμή Жуков"),"actual public certificate subject must retain Unicode");
    assert!(call(json!({"kind":"inspect_pfx","pfx":pfx,"password":"wrong password"})).get("Err").is_some());
    assert!(call(json!({"kind":"inspect_pfx","pfx":pfx,"password":password})).get("Ok").is_some());
    let source=dir.path().join("source.pdf"); let original=plain(); std::fs::write(&source,&original).unwrap();
    let wrong_fingerprint=dir.path().join("wrong-fingerprint.pdf");
    let refused=call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":wrong_fingerprint,"pfx":pfx,"password":password,
        "options":{"mode":"approval","reason":"","expected_identity_fingerprint":"00".repeat(32)}}));
    assert!(refused["Err"].as_str().unwrap().contains("Identity file changed; reopen it to confirm"));
    assert!(!wrong_fingerprint.exists()); assert_eq!(std::fs::read(&source).unwrap(),original);
    for (index,mode) in ["approval","certify_no_changes"].iter().enumerate() {
        let output=dir.path().join(format!("signed-{index}.pdf"));
        let signed=call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":output,"pfx":pfx,"password":password,"options":{"mode":mode,"reason":"Integration integrity only"}}));
        assert!(signed.get("Ok").is_some(),"signing: {signed}");
        let bytes=std::fs::read(&output).unwrap(); assert!(bytes.starts_with(&original));
        let checks=&signed["Ok"]["report"]["signatures"];
        assert_eq!(checks[0]["integrity_valid"],true); assert_eq!(checks[0]["later_changes"],false);
        assert_eq!(checks[0]["identity_trust"],"not_checked");
        if index==1 { assert_eq!(checks[0]["certification"],1); }
        oracle(&output,&bytes,dir.path());
        let mut tampered=bytes.clone(); let marker=tampered.windows(8).position(|w|w==b"ORIGINAL").unwrap(); tampered[marker]=b'X';
        assert_eq!(verify(&dir.path().join("tampered.pdf"),&tampered)["Ok"]["report"]["signatures"][0]["integrity_valid"],false);
        let mut malformed=bytes.clone(); let at=malformed.windows(10).position(|w|w==b"/ByteRange").unwrap(); let at=at+malformed[at..].iter().position(|b|*b==b'[').unwrap()+1; let at=at+malformed[at..].iter().position(|b|b.is_ascii_digit()).unwrap(); malformed[at]=b'1';
        assert_eq!(verify(&dir.path().join("malformed.pdf"),&malformed)["Ok"]["report"]["signatures"][0]["integrity_valid"],false);
        let later=annotate(bytes.clone());
        let later_check=verify(&dir.path().join("later.pdf"),&later);
        assert_eq!(later_check["Ok"]["report"]["signatures"][0]["integrity_valid"],true);
        assert_eq!(later_check["Ok"]["report"]["signatures"][0]["later_changes"],true);
        let refused=dir.path().join("cannot-cosign.pdf");
        assert!(call(json!({"kind":"sign_pdf","input":output,"input_sha256":digest(&bytes),"output":refused,"pfx":pfx,"password":password,"options":{"mode":"approval","reason":""}})).get("Err").is_some());
        assert!(!refused.exists());
        // Existing output must survive a successful crypto operation followed by
        // create_new refusal, independently of the parent UI's output guard.
        assert!(call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":output,"pfx":pfx,"password":password,"options":{"mode":"approval","reason":""}})).get("Err").is_some());
        assert_eq!(std::fs::read(&output).unwrap(),bytes);
    }
    let incremental=annotate(original.clone()); let history=dir.path().join("history.pdf"); std::fs::write(&history,&incremental).unwrap();
    let normalized=dir.path().join("normalized-signed.pdf");
    let result=call(json!({"kind":"sign_pdf","input":history,"input_sha256":digest(&incremental),"output":normalized,"pfx":pfx,"password":password,"options":{"mode":"certify_no_changes","reason":"Unsigned current graph"}}));
    assert!(result.get("Ok").is_some(),"unsigned history normalization: {result}");
    let normalized_bytes=std::fs::read(&normalized).unwrap();
    assert!(!normalized_bytes.starts_with(&incremental));
    assert!(result["Ok"]["report"]["signatures"][0]["message"].as_str().unwrap().contains("Unsigned revision history was normalized"));
    let parsed=Document::load_mem(&normalized_bytes).unwrap();
    assert!(parsed.objects.values().filter_map(|o|o.as_dict().ok()).any(|d|d.get(b"Contents").ok().and_then(|v|v.as_str().ok()).is_some_and(|v|v.starts_with(b"UNSAVED? No, persisted"))));
    oracle(&normalized,&normalized_bytes,dir.path());
    assert_eq!(std::fs::read(&source).unwrap(),original);
    assert_eq!(std::fs::read(&history).unwrap(),incremental);
    // Actual visible signatures on a non-first, inherited cropped/rotated page.
    // PDFium rendering is independent of the appearance writer; no UI-thread render.
    let mut reference_ink:Option<Vec<bool>>=None;
    for rotation in [0u16,90,180,270] {
        let original=cropped_second_page(rotation,1.0);
        let source=dir.path().join(format!("visible-source-{rotation}.pdf")); std::fs::write(&source,&original).unwrap();
        let output=dir.path().join(format!("visible-signed-{rotation}.pdf"));
        let rect=if rotation%180==0 { [100.0f32,200.0,400.0,320.0] } else { [100.0,200.0,220.0,500.0] };
        let reason="Résumé approved — Δοκιμή";
        let signed=call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":output,"pfx":pfx,"password":password,
            "options":{"mode":"approval","reason":reason,"appearance":{"page":1,"rect":rect,"rotation":rotation}}}));
        assert!(signed.get("Ok").is_some(),"visible signature: {signed}");
        let bytes=std::fs::read(&output).unwrap(); assert!(bytes.starts_with(&original));
        assert_eq!(std::fs::read(&source).unwrap(),original);
        let parsed=Document::load_mem(&bytes).unwrap();
        let pages=parsed.get_pages(); let page=*pages.values().nth(1).unwrap();
        let widget=parsed.objects.values().filter_map(|o|o.as_dict().ok()).find(|d|d.get(b"FT").ok().and_then(|o|o.as_name().ok())==Some(b"Sig")).unwrap();
        assert_eq!(widget.get(b"P").unwrap().as_reference().unwrap(),page);
        assert_eq!(widget.get(b"F").unwrap().as_i64().unwrap(),132);
        assert!(!widget.has(b"MK"),"Form Matrix is the only orientation transform");
        let form_id=widget.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
        let form=parsed.get_object(form_id).unwrap().as_stream().unwrap();
        let text=appearance_text(&parsed,form);
        assert!(text.contains(actual_subject)); assert!(text.contains(reason));
        let signature=parsed.objects.values().filter_map(|o|o.as_dict().ok()).find(|d|d.get(b"Type").ok().and_then(|o|o.as_name().ok())==Some(b"Sig")).unwrap();
        let m=String::from_utf8(signature.get(b"M").unwrap().as_str().unwrap().to_vec()).unwrap();
        let date=chrono::NaiveDateTime::parse_from_str(m.trim_start_matches("D:"),"%Y%m%d%H%M%SZ").unwrap().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(text.contains(&date),"AP date and /M must share the actual signing instant");
        let reason_bytes=signature.get(b"Reason").unwrap().as_str().unwrap(); assert!(reason_bytes.starts_with(&[0xfe,0xff]));
        assert!(String::from_utf8(form.content.clone()).unwrap().contains("re W n"));
        let first_before=raster(&original,0); let first_after=raster(&bytes,0);
        assert_eq!(first_before.2,first_after.2,"unselected page must remain visually unchanged");
        let (w,h,before,geometry)=raster(&original,1); let (sw,sh,after,_)=raster(&bytes,1);
        assert_eq!((w,h),(sw,sh)); assert_eq!(u16::from(geometry.rotation)*90,rotation);
        let q=kinetic_pdf::domain::PdfBox {left:rect[0],bottom:rect[1],right:rect[2],top:rect[3]};
        let (l,t,r,b)=geometry.box_to_view(&q);
        let (l,t,r,b)=((l*w as f32).round() as usize,(t*h as f32).round() as usize,(r*w as f32).round() as usize,(b*h as f32).round() as usize);
        assert_eq!((r-l,b-t),(300,120));
        let mut changed=0; let mut ink=vec![];
        for y in 0..h { for x in 0..w {
            let at=(y*w+x)*4;
            let different=before[at..at+3].iter().zip(&after[at..at+3]).any(|(a,b)|a.abs_diff(*b)>3);
            if different { assert!(x+1>=l && x<=r+1 && y+1>=t && y<=b+1,"appearance paints outside chosen rotated/cropped rect at ({x},{y})"); changed+=1; }
            if x>=l && x<r && y>=t && y<b { ink.push(after[at..at+3].iter().map(|v|u32::from(*v)).sum::<u32>()<3*170); }
        } }
        assert!(changed>3000,"standard widget AP must actually render with PDFium form flags");
        assert!(ink.iter().filter(|v|**v).count()>500,"actual embedded signer text must render, not only a blank card");
        if let Some(reference)=&reference_ink {
            let overlap=reference.iter().zip(&ink).filter(|(a,b)|**a && **b).count();
            let total=reference.iter().filter(|v|**v).count()+ink.iter().filter(|v|**v).count();
            assert!(2.0*overlap as f64/total as f64>0.75,"native raster text must remain upright for page rotation {rotation}");
        } else { reference_ink=Some(ink); }
        oracle(&output,&bytes,dir.path());
        let offset=match parsed.reference_table.entries.get(&form_id.0).unwrap() { pdf_content::lopdf::xref::XrefEntry::Normal {offset,..}=>*offset as usize,_=>panic!("expected normal AP object") };
        let relative=bytes[offset..].windows(5).position(|w|w==b"0.985").unwrap();
        let mut corrupted=bytes.clone(); corrupted[offset+relative+3]=b'7';
        assert_eq!(verify(&dir.path().join("corrupt-visible.pdf"),&corrupted)["Ok"]["report"]["signatures"][0]["integrity_valid"],false,"appearance bytes are CMS-protected");
    }
    let visible_p1_source=dir.path().join("visible-source-90.pdf");
    let visible_p1_input=std::fs::read(&visible_p1_source).unwrap();
    let visible_p1=dir.path().join("visible-certified.pdf");
    let certified=call(json!({"kind":"sign_pdf","input":visible_p1_source,"input_sha256":digest(&visible_p1_input),"output":visible_p1,"pfx":pfx,"password":password,
        "options":{"mode":"certify_no_changes","reason":"Visible certified résumé","appearance":{"page":1,"rect":[100.0,200.0,220.0,500.0],"rotation":90}}}));
    assert!(certified.get("Ok").is_some(),"visible P1: {certified}");
    assert_eq!(certified["Ok"]["report"]["signatures"][0]["certification"],1);
    let certified_bytes=std::fs::read(&visible_p1).unwrap();
    let before=raster(&visible_p1_input,1); let after=raster(&certified_bytes,1);
    assert!(before.2.iter().zip(&after.2).filter(|(a,b)|a.abs_diff(**b)>3).count()>3000,"visible P1 AP must render");
    oracle(&visible_p1,&certified_bytes,dir.path());
    assert_eq!(std::fs::read(&visible_p1_source).unwrap(),visible_p1_input);
    multipage_round_trip(dir.path(),&pfx,password,actual_subject);
    let bad_source=dir.path().join("bad-visible.pdf");
    for (case,unit,rect,rotation) in [
        ("tiny",1.0,[100.0,200.0,102.0,202.0],90),
        ("outside",1.0,[0.0,200.0,110.0,500.0],90),
        ("quarterturn",1.0,[100.0,200.0,220.0,500.0],1),
        ("mismatch",1.0,[100.0,200.0,220.0,500.0],0),
        ("userunit",2.0,[100.0,200.0,220.0,500.0],90),
    ] {
        let input=cropped_second_page(90,unit); std::fs::write(&bad_source,&input).unwrap();
        let output=dir.path().join(format!("refused-{case}.pdf"));
        assert!(call(json!({"kind":"sign_pdf","input":bad_source,"input_sha256":digest(&input),"output":output,"pfx":pfx,"password":password,
            "options":{"mode":"approval","reason":"","appearance":{"page":1,"rect":rect,"rotation":rotation}}})).get("Err").is_some(),"invalid visible placement {case} must fail");
        assert!(!output.exists()); assert_eq!(std::fs::read(&bad_source).unwrap(),input);
    }
    let wrong=dir.path().join("wrong-password.pdf");
    assert!(call(json!({"kind":"sign_pdf","input":source,"input_sha256":digest(&original),"output":wrong,"pfx":pfx,"password":"wrong","options":{"mode":"approval","reason":""}})).get("Err").is_some());
    assert!(!wrong.exists());
}

#[test]
fn bounded_helper_transport_rejects_oversized_and_malformed_requests_without_outputs() {
    assert!(!invoke(vec![b' ';65537]).0.success());
    assert!(!invoke(b"not-json".to_vec()).0.success());
    let task=json!({"kind":"verify_pdf","input":"must-not-be-opened.pdf","input_sha256":"00".repeat(32)});
    // Missing version must not default silently to the current protocol.
    assert!(!invoke(serde_json::to_vec(&json!({"id":91,"task":task})).unwrap()).0.success());
    let (status,raw)=invoke(serde_json::to_vec(&json!({"version":2,"id":92,"task":task})).unwrap());
    assert!(status.success());
    let response:Value=serde_json::from_slice(&raw).unwrap();
    assert_eq!(response["version"],kinetic_pdf::signing::PROTOCOL_VERSION);
    assert_eq!(response["id"],92);
    assert!(response["result"]["Err"].as_str().unwrap().contains("protocol version"));
}
