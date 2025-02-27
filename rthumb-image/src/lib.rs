use std::path::Path;

use anyhow::anyhow;
use image::EncodableLayout;
use itertools::Itertools;
use rthumb::{ThumbFsMeta, ThumbFullMeta, ThumbJob};

pub struct ImageProvider;

impl Default for ImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageProvider {
    pub fn new() -> Self {
        Self {}
    }
}

impl rthumb::Provider for ImageProvider {
    fn supported_mime_types(&self) -> Vec<&'static str> {
        image::ImageFormat::all()
            .map(|f| f.to_mime_type())
            .chain(["image/vnd.microsoft.icon"])
            .dedup()
            .collect()
    }

    fn process(&self, opaque: usize, cache_dir: &Path, job: ThumbJob) -> anyhow::Result<()> {
        self.process_one_media(opaque, cache_dir, job)
    }

    fn name(&self) -> &'static str {
        "Rust image crate"
    }
}

impl ImageProvider {
    fn process_one_media(
        &self,
        opaque: usize,
        cache_dir: &Path,
        job: ThumbJob,
    ) -> anyhow::Result<()> {
        let original_path = match url::Url::parse(&job.media.uri)?.to_file_path() {
            Ok(path) => path,
            Err(_) => return Err(anyhow!("not a file://")),
        };
        let cache_dir = job.flavor.cache_path(cache_dir);
        let original_meta = ThumbFsMeta::from(&job.media.uri, &original_path)?;
        let thumb_path = rthumb::destination_filename(&cache_dir, &job.media.uri);
        // Bail cheaply if already on disk & no changes.
        if let Ok(existing_original_meta) = rthumb::get_thumb_original_metadata(&thumb_path) {
            if existing_original_meta == original_meta {
                // debug!("cache hit for {}", &media.uri);
                return Ok(());
            }
        }
        let (orig_width, orig_height, thumb) = {
            let im = image::open(&original_path)?;
            let dimension = job.flavor.dimension();
            (
                im.width(),
                im.height(),
                im.thumbnail(dimension, dimension).to_rgb8(),
            )
        };
        let original_meta = ThumbFullMeta::from(original_meta, orig_width, orig_height);
        let temp_thumb_path = rthumb::temp_filename(&cache_dir, &job.media.uri, opaque);
        rthumb::write_thumb_with_original_metadata(
            &temp_thumb_path,
            &original_meta,
            thumb.width(),
            thumb.height(),
            thumb.as_bytes(),
        )?;
        std::fs::rename(&temp_thumb_path, &thumb_path)?;
        Ok(())
    }
}
