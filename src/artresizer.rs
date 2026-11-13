//! A port of beets' `beets/util/artresizer.py`: the image operations album
//! art handling needs (size, format, resizing, deinterlacing, format
//! conversion and similarity), done in Rust with the `image` crate rather
//! than by shelling out to ImageMagick or using PIL.
//!
//! Where beets' two backends differ, this follows ImageMagick (beets' first
//! choice): `resize` limits the width only (`-resize WIDTHx>`), and the
//! comparison reproduces ImageMagick's `PHASH` metric (see [`phash`]).
//!
//! As in beets, an operation that fails logs a warning and hands back the
//! input path unchanged.

use crate::logging::debug;
use image::{DynamicImage, ImageFormat, ImageReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// ImageMagick's JPEG quality when none is asked for and it can't estimate
/// the input's.
pub const DEFAULT_JPEG_QUALITY: u8 = 92;

/// beets' `util.get_temp_filename`: a fresh path in the temp directory with
/// the given prefix and the extension of `like` (if any).
pub fn temp_filename(prefix: &str, like: Option<&Path>) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let suffix = like
        .and_then(|p| p.extension())
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "beets-{prefix}{}-{n}-{nanos:08x}{suffix}",
        std::process::id()
    ))
}

/// A name like Python's `tempfile.NamedTemporaryFile`: `tmp` and eight
/// random characters, in the temp directory.
// Matches beets/PIL image and temp-name integer math (Python int()/float()).
#[allow(clippy::cast_possible_truncation)]
pub fn py_temp_name() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id()) << 32
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut name = String::from("tmp");
    for _ in 0..8 {
        // xorshift
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        name.push(CHARS[(x % CHARS.len() as u64) as usize] as char);
    }
    std::env::temp_dir().join(name)
}

fn open(path: &Path) -> image::ImageResult<(DynamicImage, Option<ImageFormat>)> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    Ok((reader.decode()?, format))
}

/// The image's `(width, height)`, or `None` (with a warning) if it can't be
/// read.
pub fn get_size(path: &Path) -> Option<(u32, u32)> {
    let r = ImageReader::open(path)
        .and_then(image::ImageReader::with_guessed_format)
        .map_err(image::ImageError::IoError)
        .and_then(image::ImageReader::into_dimensions);
    match r {
        Ok(size) => Some(size),
        Err(e) => {
            crate::logging::emit("ImageMagick size query failed".into());
            debug!("could not get the size of {}: {e}", path.display());
            None
        }
    }
}

/// ImageMagick's name for a format (`%[magick]`).
fn magick_name(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::WebP => "WEBP",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Ico => "ICO",
        ImageFormat::Avif => "AVIF",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Tga => "TGA",
        ImageFormat::Dds => "DDS",
        ImageFormat::Hdr => "HDR",
        ImageFormat::OpenExr => "EXR",
        ImageFormat::Farbfeld => "FARBFELD",
        ImageFormat::Qoi => "QOI",
        _ => "UNKNOWN",
    }
}

/// The image's format as ImageMagick names it (`JPEG`, `PNG`, ...), from
/// its contents, or `None` if it isn't a recognised image.
pub fn get_format(path: &Path) -> Option<String> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let reader = ImageReader::new(file).with_guessed_format().ok()?;
    reader.format().map(|f| magick_name(f).to_string())
}

/// The format to write `path` in: from its extension (as ImageMagick
/// does), else the input's.
fn output_format(path: &Path, input: Option<ImageFormat>) -> ImageFormat {
    ImageFormat::from_path(path)
        .ok()
        .or(input)
        .unwrap_or(ImageFormat::Png)
}

/// Encode `img` to `format`. JPEGs are baseline (never progressive) at
/// `quality` (0: [`DEFAULT_JPEG_QUALITY`]); other formats are written
/// non-interlaced.
pub fn encode(img: &DynamicImage, format: ImageFormat, quality: u8) -> image::ImageResult<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => {
            let q = if quality == 0 {
                DEFAULT_JPEG_QUALITY
            } else {
                quality.min(100)
            };
            return encode_jpeg(img, q);
        }
        ImageFormat::WebP => {
            // The encoder takes 8-bit RGB(A) only.
            let img = if img.color().has_alpha() {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            };
            img.write_to(&mut out, format)?;
        }
        ImageFormat::Gif | ImageFormat::Ico | ImageFormat::Tga | ImageFormat::Bmp => {
            DynamicImage::ImageRgba8(img.to_rgba8()).write_to(&mut out, format)?;
        }
        _ => img.write_to(&mut out, format)?,
    }
    Ok(out.into_inner())
}

/// A baseline JPEG as ImageMagick (libjpeg) writes one: 4:2:0 chroma
/// subsampling below quality 90 (none from 90 up). JPEG has no alpha: it's
/// dropped.
// Matches beets/PIL image and temp-name integer math (Python int()/float()).
#[allow(clippy::cast_possible_truncation)]
fn encode_jpeg(img: &DynamicImage, quality: u8) -> image::ImageResult<Vec<u8>> {
    use jpeg_encoder::{ColorType, Encoder};
    let to_err = |e: jpeg_encoder::EncodingError| {
        image::ImageError::Encoding(image::error::EncodingError::new(
            image::error::ImageFormatHint::Exact(ImageFormat::Jpeg),
            e,
        ))
    };
    let (w, h) = (img.width(), img.height());
    if w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::DimensionError),
        ));
    }
    let mut out = Vec::new();
    // The encoder's default sampling is libjpeg's (2x2 below quality 90).
    // Its optimised Huffman tables give corrupt files together with chroma
    // subsampling (jpeg-encoder 0.7.1), so they're used without it only.
    let mut enc = Encoder::new(&mut out, quality);
    enc.set_optimized_huffman_tables(quality >= 90);
    if img.color().has_color() {
        enc.encode(img.to_rgb8().as_raw(), w as u16, h as u16, ColorType::Rgb)
    } else {
        enc.encode(img.to_luma8().as_raw(), w as u16, h as u16, ColorType::Luma)
    }
    .map_err(to_err)?;
    Ok(out)
}

/// Resize to at most `maxwidth` pixels wide, keeping the aspect ratio
/// (never enlarging), into a temporary file of the same type; re-encode
/// at `quality` and, with `max_filesize`, lower a JPEG's quality until it
/// fits (ImageMagick's `-define jpeg:extent`). Returns the new path, or
/// `path_in` on failure.
pub fn resize(maxwidth: u32, path_in: &Path, quality: u8, max_filesize: u64) -> PathBuf {
    let path_out = temp_filename("resize_", Some(path_in));
    debug!(
        "artresizer: resizing {} to {}",
        path_in.display(),
        path_out.display()
    );
    match resize_to(maxwidth, path_in, &path_out, quality, max_filesize) {
        Ok(()) => path_out,
        Err(e) => {
            crate::logging::emit(format!(
                "artresizer: convert failed for {}",
                path_in.display()
            ));
            debug!("artresizer: {e}");
            path_in.to_path_buf()
        }
    }
}

/// The scaled size for `-resize WIDTHx>`.
pub fn scaled_size(w: u32, h: u32, maxwidth: u32) -> (u32, u32) {
    if maxwidth == 0 || w <= maxwidth {
        return (w, h);
    }
    let nh = (f64::from(h) * f64::from(maxwidth) / f64::from(w))
        .round()
        .max(1.0) as u32;
    (maxwidth, nh)
}

pub fn resize_to(
    maxwidth: u32,
    path_in: &Path,
    path_out: &Path,
    quality: u8,
    max_filesize: u64,
) -> image::ImageResult<()> {
    let (img, input) = open(path_in)?;
    let (w, h) = scaled_size(img.width(), img.height(), maxwidth);
    let img = if (w, h) == (img.width(), img.height()) {
        img
    } else {
        img.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
    };
    let format = output_format(path_out, input);
    let mut data = encode(&img, format, quality)?;
    if max_filesize > 0 && format == ImageFormat::Jpeg && data.len() as u64 > max_filesize {
        // The best quality that fits, by bisection (as jpeg:extent).
        let top = if quality > 0 {
            quality.min(100)
        } else {
            DEFAULT_JPEG_QUALITY
        };
        let (mut lo, mut hi) = (1u8, top - 1);
        let mut best = None;
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            let d = encode(&img, format, mid)?;
            if d.len() as u64 <= max_filesize {
                best = Some(d);
                lo = mid + 1;
            } else if mid == 1 {
                break;
            } else {
                hi = mid - 1;
            }
        }
        data = match best {
            Some(d) => d,
            None => {
                crate::logging::emit(format!(
                    "artresizer: could not resize {} to below {max_filesize}B",
                    path_in.display()
                ));
                encode(&img, format, 1)?
            }
        };
    }
    std::fs::write(path_out, data)?;
    Ok(())
}

/// Rewrite the image without interlacing into a temporary file; returns
/// `path_in` on failure.
pub fn deinterlace(path_in: &Path) -> PathBuf {
    let path_out = temp_filename("deinterlace_", Some(path_in));
    let r = open(path_in).and_then(|(img, input)| {
        let format = output_format(&path_out, input);
        let data = encode(&img, format, 0)?;
        std::fs::write(&path_out, data)?;
        Ok(())
    });
    match r {
        Ok(()) => path_out,
        Err(_) => path_in.to_path_buf(),
    }
}

/// beets' `ArtResizer.reformat`: convert to `new_format` (`JPEG`, `png`,
/// ...), giving the file the matching extension and removing the
/// original. Returns the new path, or `path_in` on failure.
pub fn reformat(path_in: &Path, new_format: &str, _deinterlaced: bool) -> PathBuf {
    let mut fmt = new_format.to_lowercase();
    if fmt == "jpeg" {
        fmt = "jpg".into();
    }
    let path_new = path_in.with_extension(&fmt);
    let r = (|| -> image::ImageResult<()> {
        let (img, input) = open(path_in)?;
        let format = ImageFormat::from_extension(&fmt).or(input).ok_or_else(|| {
            image::ImageError::Unsupported(image::error::UnsupportedError::from_format_and_kind(
                image::error::ImageFormatHint::Name(fmt.clone()),
                image::error::UnsupportedErrorKind::Format(image::error::ImageFormatHint::Name(
                    fmt.clone(),
                )),
            ))
        })?;
        // Our encoders never interlace, so `deinterlaced` holds either way.
        let data = encode(&img, format, 0)?;
        std::fs::write(&path_new, data)?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            if path_new != path_in {
                let _ = std::fs::remove_file(path_in);
            }
            path_new
        }
        Err(e) => {
            crate::logging::emit(format!(
                "failed to convert image {} -> {}: {e}",
                path_in.display(),
                path_new.display()
            ));
            path_in.to_path_buf()
        }
    }
}

// ---------------------------------------------------------------- compare

/// ImageMagick's `MagickSafeLog10`.
fn safe_log10(x: f64) -> f64 {
    const EPS: f64 = 1.0e-11;
    if x.abs() < EPS {
        EPS.log10()
    } else {
        x.abs().log10()
    }
}

/// An image's perceptual hash as ImageMagick computes it for `-metric
/// PHASH` on a grayscale image: the image is converted to gray (Rec. 709
/// luma), blurred (Gaussian, sigma 1) and quantised to 8 bits, and the
/// hash is `-log10|I|` of its 8 image-moment invariants (Hu's seven plus
/// ImageMagick's eighth). Scale- and translation-invariant, so differently
/// sized versions of one picture hash alike.
pub fn phash(img: &DynamicImage) -> [f64; 8] {
    let rgb = img.to_rgb32f();
    let (w, h) = rgb.dimensions();
    let mut gray = image::GrayImage::new(w, h);
    for (x, y, p) in rgb.enumerate_pixels() {
        let [r, g, b] = p.0;
        let l = 0.212656 * r + 0.715158 * g + 0.072186 * b;
        gray.put_pixel(
            x,
            y,
            image::Luma([(l.clamp(0.0, 1.0) * 255.0).round() as u8]),
        );
    }
    let gray = image::imageops::blur(&gray, 1.0);
    let v = |x: u32, y: u32| f64::from(gray.get_pixel(x, y).0[0]) / 255.0;

    let (mut m00, mut m10, mut m01) = (0.0, 0.0, 0.0);
    for y in 0..h {
        for x in 0..w {
            let p = v(x, y);
            m00 += p;
            m10 += f64::from(x) * p;
            m01 += f64::from(y) * p;
        }
    }
    let recip = |d: f64| if d.abs() < 1e-12 { 0.0 } else { 1.0 / d };
    let cx = m10 * recip(m00);
    let cy = m01 * recip(m00);
    let (mut m11, mut m20, mut m02, mut m21, mut m12, mut m30, mut m03) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for y in 0..h {
        for x in 0..w {
            let p = v(x, y);
            let dx = f64::from(x) - cx;
            let dy = f64::from(y) - cy;
            m11 += dx * dy * p;
            m20 += dx * dx * p;
            m02 += dy * dy * p;
            m21 += dx * dx * dy * p;
            m12 += dx * dy * dy * p;
            m30 += dx * dx * dx * p;
            m03 += dy * dy * dy * p;
        }
    }
    let n = |m: f64, p: f64, q: f64| m * recip(m00.powf(1.0 + (p + q) / 2.0));
    let (n11, n20, n02) = (n(m11, 1.0, 1.0), n(m20, 2.0, 0.0), n(m02, 0.0, 2.0));
    let (n21, n12) = (n(m21, 2.0, 1.0), n(m12, 1.0, 2.0));
    let (n30, n03) = (n(m30, 3.0, 0.0), n(m03, 0.0, 3.0));

    let a = n30 + n12;
    let b = n21 + n03;
    let i1 = n20 + n02;
    let i2 = (n20 - n02).powi(2) + 4.0 * n11 * n11;
    let i3 = (n30 - 3.0 * n12).powi(2) + (3.0 * n21 - n03).powi(2);
    let i4 = a * a + b * b;
    let i5 = (n30 - 3.0 * n12) * a * (a * a - 3.0 * b * b)
        + (3.0 * n21 - n03) * b * (3.0 * a * a - b * b);
    let i6 = (n20 - n02) * (a * a - b * b) + 4.0 * n11 * a * b;
    let i7 = (3.0 * n21 - n03) * a * (a * a - 3.0 * b * b)
        - (n30 - 3.0 * n12) * b * (3.0 * a * a - b * b);
    let i8 = n11 * (a * a - b * b) - (n20 - n02) * a * b;
    [i1, i2, i3, i4, i5, i6, i7, i8].map(|i| -safe_log10(i))
}

/// The `PHASH` distortion between two images: the sum of squared
/// differences of their hashes (0 for the same picture).
pub fn phash_distance(a: &DynamicImage, b: &DynamicImage) -> f64 {
    let (ha, hb) = (phash(a), phash(b));
    ha.iter().zip(&hb).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// beets' `ArtResizer.compare`: whether the two images are similar, i.e.
/// their PHASH distortion is at most `compare_threshold`; `None` if either
/// can't be read.
pub fn compare(im1: &Path, im2: &Path, compare_threshold: f64) -> Option<bool> {
    let read = |p: &Path| match open(p) {
        Ok((img, _)) => Some(img),
        Err(e) => {
            debug!("could not read image {} for comparison: {e}", p.display());
            None
        }
    };
    let (a, b) = (read(im1)?, read(im2)?);
    let score = phash_distance(&b, &a);
    debug!("image compare score: {score}");
    Some(score <= compare_threshold)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A test picture: a gradient with a few shapes, so it has structure
    /// for the perceptual hash. `variant` changes the shapes.
    // Matches beets/PIL image and temp-name integer math (Python int()/float()).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    pub fn picture(w: u32, h: u32, variant: u32) -> DynamicImage {
        let img = RgbImage::from_fn(w, h, |x, y| {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let mut c = [(fx * 200.0) as u8, (fy * 180.0) as u8, 90];
            let in_circle =
                |cx: f32, cy: f32, r: f32| (fx - cx).powi(2) + (fy - cy).powi(2) < r * r;
            match variant {
                0 => {
                    if in_circle(0.3, 0.35, 0.18) {
                        c = [250, 240, 30];
                    }
                    if fx > 0.6 && fy > 0.55 && fx < 0.9 && fy < 0.85 {
                        c = [20, 30, 160];
                    }
                }
                _ => {
                    if in_circle(0.75, 0.2, 0.12) {
                        c = [10, 10, 10];
                    }
                    if fx < 0.25 && fy > 0.1 {
                        c = [255, 255, 255];
                    }
                    if fy > 0.8 {
                        c = [200, 20, 20];
                    }
                }
            }
            Rgb(c)
        });
        DynamicImage::ImageRgb8(img)
    }

    pub fn write_image(path: &Path, img: &DynamicImage, quality: u8) {
        let f = ImageFormat::from_path(path).unwrap();
        std::fs::write(path, encode(img, f, quality).unwrap()).unwrap();
    }

    #[test]
    fn size_and_format() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        write_image(&p, &picture(30, 20, 0), 0);
        assert_eq!(get_size(&p), Some((30, 20)));
        assert_eq!(get_format(&p).as_deref(), Some("PNG"));
        // Detected from the contents, not the name.
        let j = d.path().join("b.png");
        std::fs::write(&j, encode(&picture(4, 4, 0), ImageFormat::Jpeg, 0).unwrap()).unwrap();
        assert_eq!(get_format(&j).as_deref(), Some("JPEG"));
        let bad = d.path().join("c.jpg");
        std::fs::write(&bad, b"not an image").unwrap();
        assert_eq!(get_size(&bad), None);
        assert_eq!(get_format(&bad), None);
    }

    #[test]
    fn resize_keeps_aspect_and_type() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        write_image(&p, &picture(400, 200, 0), 95);
        let out = resize(100, &p, 0, 0);
        assert_ne!(out, p);
        assert_eq!(out.extension().unwrap(), "jpg");
        assert_eq!(get_size(&out), Some((100, 50)));
        assert_eq!(get_format(&out).as_deref(), Some("JPEG"));
        std::fs::remove_file(&out).unwrap();
        // Width only, never enlarging (`WIDTHx>`).
        let tall = d.path().join("t.png");
        write_image(&tall, &picture(50, 300, 0), 0);
        let out = resize(100, &tall, 0, 0);
        assert_eq!(get_size(&out), Some((50, 300)));
        std::fs::remove_file(&out).unwrap();
        assert_eq!(scaled_size(500, 490, 250), (250, 245));
    }

    #[test]
    fn resize_quality_and_max_filesize() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        write_image(&p, &picture(300, 300, 1), 100);
        let hi = resize(300, &p, 95, 0);
        let lo = resize(300, &p, 20, 0);
        let (hs, ls) = (
            std::fs::metadata(&hi).unwrap().len(),
            std::fs::metadata(&lo).unwrap().len(),
        );
        assert!(ls < hs, "{ls} >= {hs}");
        let limit = (ls + hs) / 2;
        let fit = resize(300, &p, 95, limit);
        let fs = std::fs::metadata(&fit).unwrap().len();
        assert!(fs <= limit && fs > ls, "{fs} not in ({ls}, {limit}]");
        for f in [hi, lo, fit] {
            std::fs::remove_file(f).unwrap();
        }
    }

    #[test]
    fn reformat_replaces_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cover.png");
        write_image(&p, &picture(20, 20, 0), 0);
        let out = reformat(&p, "JPEG", true);
        assert_eq!(out, d.path().join("cover.jpg"));
        assert!(!p.exists());
        assert_eq!(get_format(&out).as_deref(), Some("JPEG"));
        // Same extension: converted in place.
        let q = d.path().join("x.jpg");
        std::fs::write(&q, encode(&picture(8, 8, 0), ImageFormat::Png, 0).unwrap()).unwrap();
        assert_eq!(reformat(&q, "jpeg", false), q);
        assert_eq!(get_format(&q).as_deref(), Some("JPEG"));
        // Unreadable: unchanged.
        let bad = d.path().join("bad.png");
        std::fs::write(&bad, b"junk").unwrap();
        assert_eq!(reformat(&bad, "JPEG", false), bad);
        assert!(bad.exists());
    }

    /// JPEGs decode back to the picture, with and without chroma
    /// subsampling (a guard against encoder settings that corrupt them).
    #[test]
    fn jpeg_round_trip() {
        let img = picture(500, 490, 0);
        let orig = img.to_rgb8();
        for q in [20u8, 60, 89, 90, 95] {
            let d = encode(&img, ImageFormat::Jpeg, q).unwrap();
            let back = image::load_from_memory(&d).unwrap().to_rgb8();
            let err: f64 = orig
                .pixels()
                .zip(back.pixels())
                .flat_map(|(a, b)| {
                    (0..3).map(move |i| (f64::from(a.0[i]) - f64::from(b.0[i])).abs())
                })
                .sum::<f64>()
                / (500.0 * 490.0 * 3.0);
            assert!(err < 6.0, "quality {q}: mean error {err}");
        }
    }

    #[test]
    fn deinterlace_rewrites() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        write_image(&p, &picture(10, 10, 0), 0);
        let out = deinterlace(&p);
        assert_ne!(out, p);
        assert_eq!(get_size(&out), Some((10, 10)));
        std::fs::remove_file(out).unwrap();
    }

    #[test]
    fn phash_compares_like_imagemagick() {
        let a = picture(225, 225, 0);
        // The same picture, resized and recompressed: similar.
        let d = tempfile::tempdir().unwrap();
        let big = d.path().join("big.jpg");
        write_image(&big, &picture(500, 490, 0), 60);
        let small = d.path().join("small.jpg");
        write_image(&small, &a, 90);
        let other = d.path().join("other.jpg");
        write_image(&other, &picture(348, 348, 1), 90);
        assert_eq!(phash_distance(&a, &a), 0.0);
        let same = phash_distance(&a, &open(&big).unwrap().0);
        let diff = phash_distance(&a, &open(&other).unwrap().0);
        assert!(same < 2.0, "similar: {same}");
        assert!(diff > 6.0, "different: {diff}");
        assert_eq!(compare(&small, &big, 5.0), Some(true));
        assert_eq!(compare(&small, &other, 5.0), Some(false));
        let bad = d.path().join("bad.jpg");
        std::fs::write(&bad, b"junk").unwrap();
        assert_eq!(compare(&small, &bad, 5.0), None);
    }

    /// Against ImageMagick's own scores for beets' test images (from
    /// `magick a b -colorspace gray MIFF:- | magick compare -define
    /// phash:colorspaces=sRGB,HCLp -metric PHASH - null:`): 2.25655 for
    /// abbey/abbey-similar, 10.0746 for abbey/abbey-different.
    #[test]
    #[ignore = "needs the beets reference checkout's test images"]
    fn phash_matches_imagemagick_on_beets_images() {
        let rsrc = dirs::home_dir()
            .unwrap()
            .join(".cache/beets-reference/src/test/rsrc");
        let img = |n: &str| open(&rsrc.join(n)).unwrap().0;
        let abbey = img("abbey.jpg");
        let sim = phash_distance(&img("abbey-similar.jpg"), &abbey);
        let diff = phash_distance(&img("abbey-different.jpg"), &abbey);
        eprintln!("similar {sim}, different {diff}");
        // Ours: 2.95 and 11.2.
        assert!((sim - 2.25655).abs() < 1.0, "{sim}");
        assert!((diff - 10.0746).abs() < 1.5, "{diff}");
    }
}
