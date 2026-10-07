//! PDF page-label metadata. No application or MCP dependencies.
use pdf_content::lopdf::{dictionary, Document, Object, StringFormat};

pub fn write(doc: &mut Document, labels: &[Option<String>]) -> Result<(), String> {
    let count = doc.get_pages().len();
    if labels.len() > count {
        return Err("More labels than pages".into());
    }
    if labels.iter().all(Option::is_none) {
        doc.catalog_mut()
            .map_err(|e| e.to_string())?
            .remove(b"PageLabels");
        return Ok(());
    }
    let mut nums = Vec::with_capacity(count * 2);
    for page in 0..count {
        nums.push(Object::Integer(page as i64));
        let label = match labels.get(page).and_then(Option::as_ref) {
            Some(text) => {
                let bytes: Vec<u8> = [0xfe, 0xff]
                    .into_iter()
                    .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
                    .collect();
                dictionary! { "P" => Object::String(bytes, StringFormat::Hexadecimal) }
            }
            None => dictionary! { "S" => "D", "St" => page as i64 + 1 },
        };
        nums.push(Object::Dictionary(label));
    }
    doc.catalog_mut()
        .map_err(|e| e.to_string())?
        .set("PageLabels", dictionary! { "Nums" => nums });
    Ok(())
}

pub fn append(bytes: Vec<u8>, labels: &[Option<String>]) -> Result<Vec<u8>, String> {
    let mut doc = Document::load_mem(&bytes).map_err(|e| e.to_string())?;
    write(&mut doc, labels)?;
    let mut output = Vec::new();
    doc.save_to(&mut output).map_err(|e| e.to_string())?;
    Ok(output)
}
