//! Screenshots for `screen {image:true}`: JPEG, at most 1024 px on a side.

use std::io::Cursor;

const MAX_SIZE: u32 = 1024;
const QUALITY: u8 = 70;

/// (bytes, mime type); the PNG as it is when it can't be re-encoded.
pub fn shrink(png: Vec<u8>) -> (Vec<u8>, &'static str) {
    let Ok(img) = image::load_from_memory_with_format(&png, image::ImageFormat::Png) else {
        return (png, "image/png");
    };
    let img = if img.width().max(img.height()) > MAX_SIZE {
        img.resize(MAX_SIZE, MAX_SIZE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY);
    match rgb.write_with_encoder(enc) {
        Ok(()) => (out.into_inner(), "image/jpeg"),
        Err(_) => (png, "image/png"),
    }
}
