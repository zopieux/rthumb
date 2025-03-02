use std::path::Path;

use image::EncodableLayout;
use itertools::Itertools;

pub struct ImageProvider;

impl ImageProvider {
    pub fn new() -> Self {
        Self {}
    }
}

impl rthumb::Provider for ImageProvider {
    fn name(&self) -> &'static str {
        "Images (Rust: image crate)"
    }

    fn supported_mime_types(&self) -> Vec<&'static str> {
        image::ImageFormat::all()
            .map(|f| f.to_mime_type())
            // For some reason, some mime types are missing even if actually supported.
            .chain(["image/vnd.microsoft.icon", "image/webp"])
            .dedup()
            .collect()
    }

    fn process(&self, original_path: &Path, dimension: u32) -> anyhow::Result<rthumb::ProviderOut> {
        let (orig_width, orig_height, thumb) = {
            let im = image::open(&original_path)?;
            (
                im.width(),
                im.height(),
                im.thumbnail(dimension, dimension).to_rgb8(),
            )
        };
        Ok(rthumb::ProviderOut {
            width: thumb.width(),
            height: thumb.height(),
            source_width: orig_width,
            source_height: orig_height,
            data: thumb.as_bytes().to_vec(),
        })
    }
}
