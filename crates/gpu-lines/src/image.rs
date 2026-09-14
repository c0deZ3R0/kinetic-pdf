//! Decoding image XObjects into pixels: their filters undone, JPEG included,
//! their samples through their colour space and decode ranges, soft masks
//! applied, and image masks painted in the fill colour.

use pdf_content::lopdf::{Dictionary, Document, Object, Stream};
use pdf_content::objects::number;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::JpegDecoder;

use crate::colour::{space, Space};
use crate::pdf::numbers;
use crate::shapes::Unsupported;

/// Images with more pixels than this aren't decoded, against ones that would
/// take all the memory there is.
const MOST_PIXELS: u64 = 1 << 26;

/// An image's pixels: rows from the top, each pixel red, green, blue and
/// alpha, premultiplied by alpha.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    /// This bitmap averaged down to `width` by `height` pixels, each no more
    /// than it has.
    pub(crate) fn shrunk(&self, width: u32, height: u32) -> Bitmap {
        let (from_width, from_height) = (self.width as usize, self.height as usize);
        let (width, height) = (width.clamp(1, self.width), height.clamp(1, self.height));
        let (to_width, to_height) = (width as usize, height as usize);
        // Each new pixel's red, green, blue and alpha summed, and how many.
        let mut sums = vec![[0_u64; 5]; to_width * to_height];
        for y in 0..from_height {
            let row = y * to_height / from_height * to_width;
            for x in 0..from_width {
                let sum = &mut sums[row + x * to_width / from_width];
                let from = (y * from_width + x) * 4;
                for (total, &byte) in sum.iter_mut().zip(&self.pixels[from..from + 4]) {
                    *total += u64::from(byte);
                }
                sum[4] += 1;
            }
        }
        let pixels = sums
            .iter()
            .flat_map(|sum| {
                let count = sum[4].max(1);
                [0, 1, 2, 3].map(|channel| ((sum[channel] + count / 2) / count) as u8)
            })
            .collect();
        Bitmap { width, height, pixels }
    }
}

/// Image XObject `image`, decoded. An image mask is painted in `fill`, the
/// fill colour it's drawn with.
pub(crate) fn decode(doc: &Document, image: &Stream, fill: Option<[f32; 3]>) -> Result<Bitmap, Unsupported> {
    let dict = &image.dict;
    let decode_ranges = dict.get(b"Decode").ok().and_then(|d| numbers(doc, d));
    if dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
        let fill = fill.ok_or("image masks in colours not drawn yet")?;
        let mask = samples(doc, image, 1, 1)?;
        let painted = if decode_ranges.as_deref().is_some_and(|d| d.first() == Some(&1.0)) { 1 } else { 0 };
        return Ok(mask.pixels(|at| if mask.get(at, 0) == painted { (fill, 1.0) } else { ([0.0; 3], 0.0) }));
    }
    if dict.has(b"Mask") {
        return Err("images with colour key or stencil masks");
    }

    let named = dict.get(b"ColorSpace").map_or(Space::Unsupported, |s| space(doc, s));
    let colours = samples(doc, image, named.components(), 8)?;
    // A JPEG says itself how many components it has.
    let colour_space = if colours.components == named.components() { named } else { Space::with_components(colours.components) };
    if matches!(colour_space, Space::Unsupported | Space::Pattern) {
        return Err("images in colour spaces not drawn yet");
    }
    let most = colours.most() as f32;
    let indexed = matches!(colour_space, Space::Indexed { .. });
    let ranges: Vec<[f32; 2]> = (0..colours.components)
        .map(|c| match decode_ranges.as_deref().and_then(|d| d.get(c * 2..c * 2 + 2)) {
            Some(&[low, high]) => [low, high],
            _ if indexed => [0.0, most],
            _ => [0.0, 1.0],
        })
        .collect();
    let soft_mask = image.dict.get(b"SMask").ok().and_then(|m| doc.dereference(m).ok()).and_then(|(_, m)| m.as_stream().ok());
    let alpha = soft_mask.map(|mask| samples(doc, mask, 1, 8)).transpose()?;

    let alpha_at = |at| alpha.as_ref().map_or(1.0, |mask| mask.get(mask.nearest(at, &colours), 0) as f32 / mask.most() as f32);
    // Most images are plain bytes of gray or RGB, read straight off.
    if colours.bits == 8 && decode_ranges.is_none() && matches!(colour_space, Space::Gray | Space::Rgb) {
        let (width, components) = (colours.width as usize, colours.components);
        return Ok(colours.pixels(|(x, y)| {
            let start = (y as usize * width + x as usize) * components;
            let byte = |i: usize| f32::from(colours.data.get(start + i).copied().unwrap_or(0)) / 255.0;
            let colour = if components == 1 { [byte(0); 3] } else { [byte(0), byte(1), byte(2)] };
            (colour, alpha_at((x, y)))
        }));
    }
    Ok(colours.pixels(|at| {
        let mut values = [0.0; 4];
        for (c, (value, [low, high])) in values.iter_mut().zip(&ranges).enumerate() {
            *value = low + colours.get(at, c) as f32 * (high - low) / most;
        }
        let colour = colour_space.colour(&values[..colours.components]).unwrap_or([0.0; 3]);
        (colour, alpha_at(at))
    }))
}

/// An image's samples with its filters undone.
struct Samples {
    data: Vec<u8>,
    width: u32,
    height: u32,
    /// A pixel's samples, and each one's bits.
    components: usize,
    bits: u32,
}

impl Samples {
    /// The largest a sample can be.
    fn most(&self) -> u32 {
        ((1_u64 << self.bits) - 1) as u32
    }

    /// Sample `component` of the pixel at `(x, y)`.
    fn get(&self, (x, y): (u32, u32), component: usize) -> u32 {
        let bits = self.bits as usize;
        let row = (self.width as usize * self.components * bits).div_ceil(8);
        let bit = (x as usize * self.components + component) * bits;
        let byte = |i: usize| u32::from(self.data.get(y as usize * row + i).copied().unwrap_or(0));
        match bits {
            8 => byte(bit / 8),
            16 => (byte(bit / 8) << 8) | byte(bit / 8 + 1),
            _ => (byte(bit / 8) >> (8 - bits - bit % 8)) & self.most(),
        }
    }

    /// The pixel of these samples nearest the pixel at `at` in `other`, which
    /// may be a different size.
    fn nearest(&self, (x, y): (u32, u32), other: &Samples) -> (u32, u32) {
        let scale = |at: u32, to: u32, from: u32| (u64::from(at) * u64::from(to) / u64::from(from)) as u32;
        (scale(x, self.width, other.width), scale(y, self.height, other.height))
    }

    /// A bitmap this size, each pixel's colour and alpha from `at`.
    fn pixels(&self, at: impl Fn((u32, u32)) -> ([f32; 3], f32)) -> Bitmap {
        let mut pixels = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for y in 0..self.height {
            for x in 0..self.width {
                let (colour, alpha) = at((x, y));
                let alpha = alpha.clamp(0.0, 1.0);
                let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels.extend(colour.map(|c| byte(c * alpha)));
                pixels.push(byte(alpha));
            }
        }
        Bitmap { width: self.width, height: self.height, pixels }
    }
}

/// The samples of image stream `image`: `components` a pixel, of the bits it
/// says or else `default_bits` -- unless it's a JPEG, which says both.
fn samples(doc: &Document, image: &Stream, components: usize, default_bits: u32) -> Result<Samples, Unsupported> {
    let (width, height) = dimensions(doc, &image.dict)?;
    let filters = filters(&image.dict);
    if filters.iter().any(|f| matches!(*f, b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode")) {
        return Err("images in JPEG 2000, JBIG2 or fax encodings");
    }
    let unreadable = |_| "images that couldn't be read";
    if let Some((&b"DCTDecode", before)) = filters.split_last() {
        let mut compressed = image.clone();
        match before {
            [] => drop(compressed.dict.remove(b"Filter")),
            _ => compressed.dict.set("Filter", before.iter().map(|f| Object::Name(f.to_vec())).collect::<Vec<_>>()),
        }
        return jpeg(&compressed.decompressed_content().map_err(unreadable)?);
    }
    let bits = image.dict.get(b"BitsPerComponent").ok().and_then(|b| number(doc, b)).map_or(default_bits, |b| b as u32);
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) || components == 0 {
        return Err("images that couldn't be read");
    }
    Ok(Samples { data: image.decompressed_content().map_err(unreadable)?, width, height, components, bits })
}

/// A JPEG's samples, as many components as it has.
fn jpeg(data: &[u8]) -> Result<Samples, Unsupported> {
    let broken = |_| "JPEG images that couldn't be decoded";
    let mut decoder = JpegDecoder::new(ZCursor::new(data));
    let pixels = decoder.decode().map_err(broken)?;
    let info = decoder.info().ok_or("JPEG images that couldn't be decoded")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let components = pixels.len().checked_div(width as usize * height as usize).filter(|&c| c > 0).ok_or("JPEG images that couldn't be decoded")?;
    Ok(Samples { data: pixels, width, height, components, bits: 8 })
}

/// An image's width and height, if it has pixels and not too many.
fn dimensions(doc: &Document, dict: &Dictionary) -> Result<(u32, u32), Unsupported> {
    let side = |key: &[u8]| dict.get(key).ok().and_then(|v| number(doc, v)).filter(|&v| v >= 1.0).map(|v| v as u32);
    match (side(b"Width"), side(b"Height")) {
        (Some(width), Some(height)) if u64::from(width) * u64::from(height) <= MOST_PIXELS => Ok((width, height)),
        _ => Err("images that are empty or too big"),
    }
}

/// A stream's filters, in the order they're undone.
fn filters(dict: &Dictionary) -> Vec<&[u8]> {
    match dict.get(b"Filter") {
        Ok(Object::Name(name)) => vec![name.as_slice()],
        Ok(Object::Array(names)) => names.iter().filter_map(|n| n.as_name().ok()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::lopdf::dictionary;

    fn decoded(dict: Dictionary, data: Vec<u8>, fill: Option<[f32; 3]>) -> Result<Bitmap, Unsupported> {
        decode(&Document::with_version("1.7"), &Stream::new(dict, data), fill)
    }

    fn image(width: i64, height: i64, space: Object, bits: i64) -> Dictionary {
        dictionary! { "Subtype" => "Image", "Width" => width, "Height" => height, "ColorSpace" => space, "BitsPerComponent" => bits }
    }

    #[test]
    fn rgb_samples_become_pixels_rows_from_the_top() {
        let bitmap = decoded(image(2, 1, "DeviceRGB".into(), 8), vec![255, 0, 0, 0, 0, 255], None).unwrap();
        assert_eq!((bitmap.width, bitmap.height, bitmap.pixels), (2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]));
    }

    #[test]
    fn samples_narrower_than_a_byte_follow_their_decode_ranges() {
        let mut dict = image(3, 1, "DeviceGray".into(), 1);
        dict.set("Decode", vec![1.into(), 0.into()]);
        let bitmap = decoded(dict, vec![0b1010_0000], None).unwrap();
        let grays: Vec<u8> = bitmap.pixels.chunks(4).map(|p| p[0]).collect();
        assert_eq!(grays, [0, 255, 0], "1, 0, 1 inverted");
    }

    #[test]
    fn indexed_samples_look_their_colours_up() {
        let palette = Object::Array(vec!["Indexed".into(), "DeviceRGB".into(), 1.into(), Object::string_literal(vec![255, 0, 0, 0, 255, 0])]);
        let bitmap = decoded(image(2, 1, palette, 2), vec![0b0100_0000], None).unwrap();
        assert_eq!(bitmap.pixels, [0, 255, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn a_soft_mask_gives_alpha_and_colours_are_premultiplied() {
        let mut dict = image(1, 1, "DeviceGray".into(), 8);
        dict.set("SMask", Stream::new(image(1, 1, "DeviceGray".into(), 8), vec![128]));
        assert_eq!(decoded(dict, vec![255], None).unwrap().pixels, [128, 128, 128, 128]);
    }

    #[test]
    fn an_image_mask_paints_the_fill_colour_where_its_samples_are_zero() {
        let dict = dictionary! { "Subtype" => "Image", "Width" => 2, "Height" => 1, "ImageMask" => true };
        let bitmap = decoded(dict.clone(), vec![0b0100_0000], Some([0.0, 0.0, 1.0])).unwrap();
        assert_eq!(bitmap.pixels, [0, 0, 255, 255, 0, 0, 0, 0]);
        assert_eq!(decoded(dict, vec![0], None).err(), Some("image masks in colours not drawn yet"));
    }

    #[test]
    fn a_bitmap_shrinks_by_averaging() {
        let pixels = [[0, 0, 0, 255], [100, 0, 0, 255], [200, 0, 0, 255], [255, 0, 0, 255]].concat();
        let wide = Bitmap { width: 4, height: 1, pixels };
        assert_eq!(wide.shrunk(2, 1).pixels, [50, 0, 0, 255, 228, 0, 0, 255]);
        assert_eq!((wide.shrunk(9, 9).width, wide.shrunk(0, 9).height), (4, 1), "never bigger, never empty");
    }

    #[test]
    fn images_that_cant_be_drawn_say_why() {
        let mut jpx = image(1, 1, "DeviceRGB".into(), 8);
        jpx.set("Filter", "JPXDecode");
        assert_eq!(decoded(jpx, vec![0; 3], None).err(), Some("images in JPEG 2000, JBIG2 or fax encodings"));
        assert_eq!(decoded(image(1, 1, "Lab".into(), 8), vec![0], None).err(), Some("images in colour spaces not drawn yet"));
        assert_eq!(decoded(image(0, 1, "DeviceGray".into(), 8), vec![], None).err(), Some("images that are empty or too big"));
    }
}
