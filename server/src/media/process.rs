//! 画像の検査とエンコードし直し（13-security.md「メディア」、Q96, Q113, Q127）。
//!
//! - 形式はファイルの中身（先頭のバイト列）で判定し、JPEG・PNG・GIF・WebP だけを受け付ける。拡張子や申告の形式は見ない。
//!   SVG・AVIF などは受け付けない（INV-MED-01）。
//! - 画素数は、ヘッダーを読んだ時点（デコードの前）に確かめる。GIFのアニメーションは、全フレームの画素数の合計で確かめる
//!   （解凍爆弾の対策、INV-MED-06）。
//! - 受け付けた画像は、すべてデコードしてエンコードし直す。位置情報（EXIF）などのメタデータは書き出さない（INV-MED-02）。
//!   画像とスクリプトを兼ねるファイルも、ここで無害化される。
//! - 向き（EXIFの Orientation）は画素に反映してから捨てる。色の情報（ICCプロファイル）も捨てる。プロファイルには任意のバイト列を
//!   入れられ、エンコードし直しによる無害化に穴が開くため（第三者の確認の R25）。広色域の写真は、色が少し地味に見えることがある。
//! - 保存の形式: JPEG（画質90）、PNG、GIF（アニメーションを保つ）、WebP（劣化なし。Rustだけのエンコーダーは劣化なしだけのため）。
//!   WebPのアニメーションは最初のフレームだけになる。
//!
//! この処理は時間がかかるので、呼ぶ側が別のスレッドで、時間の上限を付けて動かす。

use std::io::Cursor;

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits,
};

/// 保存する形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    /// 保存したファイルの拡張子から（配信のとき）。
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "jpg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// エンコードし直した画像。
#[derive(Debug)]
pub struct Processed {
    pub format: Format,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 受け付けなかった理由（利用者に返す。ファイルの中身は含めない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// 許可した形式ではない。
    UnsupportedFormat,
    /// 画素数が上限を超えている。
    TooManyPixels,
    /// 画像として読めない。
    Corrupt,
}

fn rejected(e: &ImageError) -> Rejected {
    match e {
        ImageError::Limits(_) => Rejected::TooManyPixels,
        ImageError::Unsupported(_) => Rejected::UnsupportedFormat,
        _ => Rejected::Corrupt,
    }
}

fn limits(max_pixels: u64) -> Limits {
    let mut l = Limits::default();
    // デコードで確保するメモリ（RGBAで16ビットの場合も含め、画素数の上限から決める）
    l.max_alloc = Some(max_pixels.saturating_mul(8).max(64 * 1024 * 1024));
    l
}

fn too_many(width: u32, height: u32, max_pixels: u64) -> bool {
    u64::from(width) * u64::from(height) > max_pixels
}

/// 画像を検査して、エンコードし直す。
pub fn process(input: &[u8], max_pixels: u64) -> Result<Processed, Rejected> {
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| Rejected::Corrupt)?;
    let format = match reader.format() {
        Some(ImageFormat::Jpeg) => Format::Jpeg,
        Some(ImageFormat::Png) => Format::Png,
        Some(ImageFormat::Gif) => Format::Gif,
        Some(ImageFormat::WebP) => Format::Webp,
        _ => return Err(Rejected::UnsupportedFormat),
    };
    if format == Format::Gif {
        return gif(input, max_pixels);
    }
    let mut reader = reader;
    reader.limits(limits(max_pixels));
    let mut decoder = reader.into_decoder().map_err(|e| rejected(&e))?;
    let (w, h) = decoder.dimensions();
    if too_many(w, h, max_pixels) {
        return Err(Rejected::TooManyPixels);
    }
    let orientation = decoder.orientation().map_err(|e| rejected(&e))?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| rejected(&e))?;
    img.apply_orientation(orientation);
    let (width, height) = (img.width(), img.height());
    let mut out = Vec::new();
    let result = match format {
        Format::Jpeg => {
            let enc = JpegEncoder::new_with_quality(&mut out, 90);
            // JPEGは透過を持てないので、色だけにする
            let img = if img.color().has_color() {
                DynamicImage::ImageRgb8(img.to_rgb8())
            } else {
                DynamicImage::ImageLuma8(img.to_luma8())
            };
            img.write_with_encoder(enc)
        }
        Format::Png => {
            let enc = PngEncoder::new_with_quality(
                &mut out,
                CompressionType::Default,
                FilterType::Adaptive,
            );
            img.write_with_encoder(enc)
        }
        Format::Webp => {
            let enc = WebPEncoder::new_lossless(&mut out);
            let img = if img.color().has_alpha() {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            };
            img.write_with_encoder(enc)
        }
        Format::Gif => unreachable!("handled above"),
    };
    result.map_err(|_| Rejected::Corrupt)?;
    Ok(Processed {
        format,
        bytes: out,
        width,
        height,
    })
}

/// GIF（アニメーションを含む）。全フレームの画素数の合計で上限を確かめる。
fn gif(input: &[u8], max_pixels: u64) -> Result<Processed, Rejected> {
    let mut decoder = GifDecoder::new(Cursor::new(input)).map_err(|e| rejected(&e))?;
    decoder
        .set_limits(limits(max_pixels))
        .map_err(|e| rejected(&e))?;
    let (width, height) = decoder.dimensions();
    let per_frame = u64::from(width) * u64::from(height);
    if per_frame > max_pixels {
        return Err(Rejected::TooManyPixels);
    }
    let mut frames = Vec::new();
    let mut total = 0u64;
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| rejected(&e))?;
        total = total.saturating_add(per_frame);
        if total > max_pixels {
            return Err(Rejected::TooManyPixels);
        }
        frames.push(frame);
    }
    if frames.is_empty() {
        return Err(Rejected::Corrupt);
    }
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        if frames.len() > 1 {
            enc.set_repeat(Repeat::Infinite)
                .map_err(|_| Rejected::Corrupt)?;
        }
        enc.encode_frames(frames).map_err(|_| Rejected::Corrupt)?;
    }
    Ok(Processed {
        format: Format::Gif,
        bytes: out,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder as _, Rgb, RgbImage, Rgba, RgbaImage};

    fn encode(img: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        encode(
            DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([10, 20, 30]))),
            ImageFormat::Png,
        )
    }

    /// PNGに、位置情報を模した eXIf のチャンクを足す（IENDの前）。
    fn with_exif_chunk(mut png: Vec<u8>, payload: &[u8]) -> Vec<u8> {
        let iend = png.len() - 12;
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        let mut body = b"eXIf".to_vec();
        body.extend_from_slice(payload);
        chunk.extend_from_slice(&body);
        let crc = crc32(&body);
        chunk.extend_from_slice(&crc.to_be_bytes());
        png.splice(iend..iend, chunk);
        png
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn accepts_allowed_formats_by_content() {
        let rgba = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 3, Rgba([1, 2, 3, 128])));
        for (bytes, format) in [
            (png(4, 3), Format::Png),
            (
                encode(
                    DynamicImage::ImageRgb8(RgbImage::new(4, 3)),
                    ImageFormat::Jpeg,
                ),
                Format::Jpeg,
            ),
            (encode(rgba.clone(), ImageFormat::Gif), Format::Gif),
            (encode(rgba, ImageFormat::WebP), Format::Webp),
        ] {
            let p = process(&bytes, 1_000).unwrap();
            assert_eq!((p.format, p.width, p.height), (format, 4, 3));
            assert_eq!(
                image::guess_format(&p.bytes).unwrap(),
                ImageFormat::from_extension(format.extension()).unwrap()
            );
        }
    }

    #[test]
    fn rejects_other_content() {
        for bad in [
            b"<html><script>alert(1)</script></html>".to_vec(),
            b"%PDF-1.4\n".to_vec(),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec(),
            // AVIF（ftyp avif）
            [
                &[0, 0, 0, 0x1c][..],
                b"ftypavif",
                &[0, 0, 0, 0],
                b"avifmif1miaf",
            ]
            .concat(),
            Vec::new(),
        ] {
            assert_eq!(
                process(&bad, 1_000).unwrap_err(),
                Rejected::UnsupportedFormat
            );
        }
        // PNGの署名だけで中身が壊れている
        assert_eq!(
            process(&png(4, 4)[..40], 1_000).unwrap_err(),
            Rejected::Corrupt
        );
    }

    #[test]
    fn rejects_too_many_pixels_before_decoding() {
        assert_eq!(
            process(&png(100, 100), 9_999).unwrap_err(),
            Rejected::TooManyPixels
        );
        assert!(process(&png(100, 100), 10_000).is_ok());
        // ヘッダーだけが巨大な大きさを宣言するPNG
        let mut huge = png(1, 1);
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = crc32(&huge[12..29]);
        huge[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            process(&huge, 50_000_000).unwrap_err(),
            Rejected::TooManyPixels
        );
    }

    #[test]
    fn metadata_is_not_written_back() {
        let marker = b"Exif\0\0GPS-MARKER-0123456789";
        let input = with_exif_chunk(png(8, 8), marker);
        assert!(input.windows(10).any(|w| w == b"GPS-MARKER"));
        let out = process(&input, 1_000).unwrap();
        assert!(!out.bytes.windows(10).any(|w| w == b"GPS-MARKER"));
        assert!(!out.bytes.windows(4).any(|w| w == b"eXIf"));
    }

    #[test]
    fn color_profiles_are_removed() {
        // 任意のバイト列を色の情報として入れた画像
        let marker = b"ICC-PROFILE-MARKER-0123456789".repeat(8);
        let pixels = RgbImage::from_pixel(4, 4, Rgb([1, 2, 3]));
        let mut input = Vec::new();
        let mut enc = PngEncoder::new(&mut input);
        enc.set_icc_profile(marker.clone()).unwrap();
        DynamicImage::ImageRgb8(pixels)
            .write_with_encoder(enc)
            .unwrap();
        let mut decoder = ImageReader::new(Cursor::new(&input))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(
            decoder.icc_profile().unwrap().as_deref(),
            Some(&marker[..]),
            "control"
        );

        let out = process(&input, 1_000).unwrap();
        let mut decoder = ImageReader::new(Cursor::new(&out.bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), None);
        assert!(!out.bytes.windows(4).any(|w| w == b"iCCP"));
    }
}
