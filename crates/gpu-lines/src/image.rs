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

/// Pixels an image needs before its rows are turned into pixels on more than
/// one thread; below it, starting the threads costs more than the work.
const WORTH_THREADS: usize = 1 << 20;

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
    /// than it has. Which new pixel each old column falls in is worked out
    /// once for the whole image rather than for every pixel: on a 20 megapixel
    /// photo those two divisions a pixel were most of the time.
    pub(crate) fn shrunk(&self, width: u32, height: u32) -> Bitmap {
        let (from_width, from_height) = (self.width as usize, self.height as usize);
        let (width, height) = (width.clamp(1, self.width), height.clamp(1, self.height));
        let (to_width, to_height) = (width as usize, height as usize);
        let column: Vec<usize> = (0..from_width).map(|x| x * to_width / from_width).collect();
        // Each new pixel's red, green, blue and alpha summed, and how many.
        let mut sums = vec![[0_u64; 4]; to_width * to_height];
        let mut counts = vec![0_u32; to_width * to_height];
        for y in 0..from_height {
            let row = y * to_height / from_height * to_width;
            let pixels = &self.pixels[y * from_width * 4..];
            for (x, &into) in column.iter().enumerate() {
                let sum = &mut sums[row + into];
                for (total, &byte) in sum.iter_mut().zip(&pixels[x * 4..x * 4 + 4]) {
                    *total += u64::from(byte);
                }
                counts[row + into] += 1;
            }
        }
        let pixels = sums
            .iter()
            .zip(&counts)
            .flat_map(|(sum, &count)| {
                let count = u64::from(count).max(1);
                [0, 1, 2, 3].map(|channel| ((sum[channel] + count / 2) / count) as u8)
            })
            .collect();
        Bitmap { width, height, pixels }
    }
}

/// An image's samples, and its soft mask's, with their filters undone: the
/// part of decoding that doesn't need to know how big the image is drawn, so
/// it can be done for a page's images on every core before drawing reaches
/// them (`Interpreter::decode_ahead`).
pub(crate) struct Raw {
    colours: Samples,
    alpha: Option<Samples>,
}

/// Image XObject `image`'s samples, and its soft mask's.
pub(crate) fn raw(doc: &Document, image: &Stream) -> Result<Raw, Unsupported> {
    if image.dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
        return Ok(Raw { colours: samples(doc, image, 1, 1)?, alpha: None });
    }
    let named = image.dict.get(b"ColorSpace").map_or(Space::Unsupported, |s| space(doc, s));
    let colours = samples(doc, image, named.components(), 8)?;
    let soft_mask = image.dict.get(b"SMask").ok().and_then(|m| doc.dereference(m).ok()).and_then(|(_, m)| m.as_stream().ok());
    let alpha = soft_mask.map(|mask| samples(doc, mask, 1, 8)).transpose()?;
    Ok(Raw { colours, alpha })
}

/// `bitmap` averaged down to `target` where it's bigger; see `Bitmap::shrunk`.
fn fit(bitmap: Bitmap, target: Option<[u32; 2]>) -> Bitmap {
    match target {
        Some([width, height]) if width < bitmap.width || height < bitmap.height => bitmap.shrunk(width, height),
        _ => bitmap,
    }
}

/// Image XObject `image`, decoded, no bigger than `target` pixels -- the size
/// it's drawn at, so a photo on a sheet shown at one pixel a point isn't kept
/// at twenty megapixels. An image mask is painted in `fill`, the fill colour
/// it's drawn with. With `ready`, its samples have already been read
/// (`raw`).
pub(crate) fn decode(doc: &Document, image: &Stream, fill: Option<[f32; 3]>, target: Option<[u32; 2]>, ready: Option<Raw>) -> Result<Bitmap, Unsupported> {
    let dict = &image.dict;
    let decode_ranges = dict.get(b"Decode").ok().and_then(|d| numbers(doc, d));
    if dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
        let fill = fill.ok_or("image masks in colours not drawn yet")?;
        let mask = match ready {
            Some(raw) => raw.colours,
            None => samples(doc, image, 1, 1)?,
        };
        let painted = if decode_ranges.as_deref().is_some_and(|d| d.first() == Some(&1.0)) { 1 } else { 0 };
        return Ok(fit(mask.pixels(|at| if mask.get(at, 0) == painted { (fill, 1.0) } else { ([0.0; 3], 0.0) }), target));
    }
    if dict.has(b"Mask") {
        return Err("images with colour key or stencil masks");
    }

    let named = dict.get(b"ColorSpace").map_or(Space::Unsupported, |s| space(doc, s));
    let Raw { colours, alpha } = match ready {
        Some(raw) => raw,
        None => raw(doc, image)?,
    };
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
    let alpha_at = |at| alpha.as_ref().map_or(1.0, |mask| mask.get(mask.nearest(at, &colours), 0) as f32 / mask.most() as f32);
    // Most images are plain bytes of gray or RGB, read straight off, a row at
    // a time in whole bytes and into the size they're kept at (`plain_pixels`).
    if colours.bits == 8 && decode_ranges.is_none() && matches!(colour_space, Space::Gray | Space::Rgb) && alpha.as_ref().is_none_or(|mask| mask.bits == 8) {
        return Ok(plain_pixels(&colours, alpha.as_ref(), target));
    }
    Ok(fit(
        colours.pixels(|at| {
            let mut values = [0.0; 4];
            for (c, (value, [low, high])) in values.iter_mut().zip(&ranges).enumerate() {
                *value = low + colours.get(at, c) as f32 * (high - low) / most;
            }
            let colour = colour_space.colour(&values[..colours.components]).unwrap_or([0.0; 3]);
            (colour, alpha_at(at))
        }),
        target,
    ))
}

/// Plain 8-bit gray or RGB samples as premultiplied RGBA pixels, with an
/// 8-bit soft mask's alpha where there is one, averaged down to `target` if
/// the image is drawn smaller than it is.
///
/// This is `Samples::pixels`, and `Bitmap::shrunk` after it, in one pass over
/// whole bytes. The general path calls a closure and works in floating point
/// for every pixel, about 9 ns each, and then reads and writes the full-size
/// image again to shrink it: 175 ms and 88 ms for one 20 megapixel photo,
/// which on a sheet shown at a pixel a point ends up a few hundred pixels
/// across.
fn plain_pixels(colours: &Samples, alpha: Option<&Samples>, target: Option<[u32; 2]>) -> Bitmap {
    let (width, height) = (colours.width as usize, colours.height as usize);
    let components = colours.components;
    let stride = width * components;
    // Which column of the mask each column of the image takes its alpha from,
    // when the mask is a different size; worked out once, not per pixel.
    let columns: Option<Vec<usize>> = alpha.map(|mask| {
        (0..width).map(|x| (x as u64 * u64::from(mask.width) / u64::from(colours.width).max(1)) as usize).collect()
    });
    // A row of samples, and the row of the mask that goes with it.
    let rows = |y: usize| {
        let row = colours.data.get(y * stride..).unwrap_or_default();
        let mask_row = alpha.map(|mask| {
            let from = (y as u64 * u64::from(mask.height) / u64::from(colours.height).max(1)) as usize * mask.width as usize;
            mask.data.get(from..).unwrap_or_default()
        });
        (row, mask_row)
    };
    // One pixel: its colour premultiplied by its alpha. Missing samples read
    // as 0, as the general path takes them.
    let pixel = |row: &[u8], mask_row: &Option<&[u8]>, x: usize| {
        let sample = |i: usize| row.get(x * components + i).copied().unwrap_or(0);
        let colour = if components == 1 { [sample(0); 3] } else { [sample(0), sample(1), sample(2)] };
        let a = match (mask_row, &columns) {
            (Some(mask_row), Some(columns)) => mask_row.get(columns[x]).copied().unwrap_or(0),
            _ => 255,
        };
        let premultiplied = colour.map(|c| ((u32::from(c) * u32::from(a) + 127) / 255) as u8);
        [premultiplied[0], premultiplied[1], premultiplied[2], a]
    };

    let [to_width, to_height] = match target {
        Some([w, h]) => [(w as usize).clamp(1, width), (h as usize).clamp(1, height)],
        None => [width, height],
    };
    let column: Vec<usize> = (0..width).map(|x| x * to_width / width).collect();

    // The pixels of target rows `from_row` to `to_row`. Each target row takes
    // in a run of the image's own rows and nothing else, so the bands can be
    // made side by side and joined.
    let band = |from_row: usize, to_row: usize| {
        let rows_here = to_row - from_row;
        if to_width == width && to_height == height {
            let mut pixels = vec![0_u8; width * rows_here * 4];
            for y in from_row..to_row {
                let (row, mask_row) = rows(y);
                let out = &mut pixels[(y - from_row) * width * 4..(y - from_row + 1) * width * 4];
                for (x, out) in out.chunks_exact_mut(4).enumerate() {
                    out.copy_from_slice(&pixel(row, &mask_row, x));
                }
            }
            return pixels;
        }
        // Smaller: every pixel of the image averaged into the one it falls in,
        // as `Bitmap::shrunk` does, without the full-size image in between.
        let mut sums = vec![[0_u64; 4]; to_width * rows_here];
        let mut counts = vec![0_u32; to_width * rows_here];
        for y in (from_row * height).div_ceil(to_height)..(to_row * height).div_ceil(to_height) {
            let (row, mask_row) = rows(y);
            let into = (y * to_height / height - from_row) * to_width;
            for (x, &column) in column.iter().enumerate() {
                let at = into + column;
                let sum = &mut sums[at];
                for (total, byte) in sum.iter_mut().zip(pixel(row, &mask_row, x)) {
                    *total += u64::from(byte);
                }
                counts[at] += 1;
            }
        }
        sums.iter()
            .zip(&counts)
            .flat_map(|(sum, &count)| {
                let count = u64::from(count).max(1);
                [0, 1, 2, 3].map(|channel| ((sum[channel] + count / 2) / count) as u8)
            })
            .collect()
    };

    // A photo's rows go to every core: one image of a drawing sheet is twenty
    // megapixels, and its pixels don't depend on each other.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(to_height);
    let pixels = if width * height < WORTH_THREADS || threads < 2 {
        band(0, to_height)
    } else {
        let rows_each = to_height.div_ceil(threads);
        let bands: Vec<(usize, usize)> = (0..threads).map(|i| (i * rows_each, ((i + 1) * rows_each).min(to_height))).filter(|(from, to)| from < to).collect();
        let made: Vec<Vec<u8>> = std::thread::scope(|scope| {
            let running: Vec<_> = bands.iter().map(|&(from, to)| scope.spawn(move || band(from, to))).collect();
            running.into_iter().map(|thread| thread.join().unwrap_or_default()).collect()
        });
        made.concat()
    };
    Bitmap { width: to_width as u32, height: to_height as u32, pixels }
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
        decode(&Document::with_version("1.7"), &Stream::new(dict, data), fill, None, None)
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
    fn plain_rgb_reads_the_same_as_the_general_path() {
        // Two rows of RGB, under a mask half the size: each mask sample covers
        // two columns and both rows.
        let mut dict = image(4, 2, "DeviceRGB".into(), 8);
        dict.set("SMask", Stream::new(image(2, 1, "DeviceGray".into(), 8), vec![255, 128]));
        let data: Vec<u8> = (0..24).map(|i| i * 10).collect();
        let fast = decoded(dict.clone(), data.clone(), None).unwrap();

        // The general path, as `decode` takes it when a decode range is set:
        // the same image, with the ranges that change nothing.
        dict.set("Decode", vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()]);
        let general = decoded(dict, data, None).unwrap();
        assert_eq!(fast, general);
        // The mask's first sample covers columns 0 and 1, its second the rest.
        assert_eq!(&fast.pixels[..8], &[0, 10, 20, 255, 30, 40, 50, 255], "fully opaque under the first sample");
        assert_eq!(&fast.pixels[8..16], &[30, 35, 40, 128, 45, 50, 55, 128], "premultiplied by half under the second");
    }

    #[test]
    fn converting_to_a_smaller_size_averages_as_shrinking_would() {
        // Four rows of gray, decoded into two by two.
        let dict = image(4, 4, "DeviceGray".into(), 8);
        let data: Vec<u8> = (0..16).map(|i| i * 16).collect();
        let doc = Document::with_version("1.7");
        let stream = Stream::new(dict, data);
        let straight = decode(&doc, &stream, None, Some([2, 2]), None).unwrap();
        let whole_then_shrunk = decode(&doc, &stream, None, None, None).unwrap().shrunk(2, 2);
        assert_eq!((straight.width, straight.height), (2, 2));
        assert_eq!(straight, whole_then_shrunk);

        // Its samples read ahead of drawing come out the same too.
        let ready = raw(&doc, &stream).unwrap();
        assert_eq!(decode(&doc, &stream, None, Some([2, 2]), Some(ready)).unwrap(), straight);
        // A target no smaller than the image leaves it as it is.
        assert_eq!(decode(&doc, &stream, None, Some([9, 9]), None).unwrap().width, 4);
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
