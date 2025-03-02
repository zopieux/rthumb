use std::path::Path;

pub struct VideoProvider {
    runtime: tokio::runtime::Runtime,
    with_film_strips: bool,
    seek_percentage: f32,
}

impl VideoProvider {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        Self {
            runtime,
            with_film_strips: match std::env::var("RTHUMB_VIDEO_STRIPS") {
                Err(_) => true,
                Ok(x) if x == "1" || x == "true" || x == "yes" || x == "on" => true,
                Ok(_) => false,
            },
            seek_percentage: (match std::env::var("RTHUMB_VIDEO_PERCENTAGE") {
                Ok(x) => match x.parse::<u8>() {
                    Ok(x) if x <= 100 => Some(x),
                    _ => None,
                },
                Err(_) => None,
            })
            .unwrap_or(15) as f32
                / 100.,
        }
    }
}

impl rthumb::Provider for VideoProvider {
    fn name(&self) -> &'static str {
        "Videos (Rust: ffmpegthumbnailer_rs crate)"
    }

    fn supported_mime_types(&self) -> Vec<&'static str> {
        vec![
            "video/3gpp",
            "video/3gpp2",
            "video/mp4",
            "video/mpeg",
            "video/ogg",
            "video/quicktime",
            "video/webm",
            "video/x-f4v",
            "video/x-flv",
            "video/x-m4v",
            "video/x-matroska",
            "video/x-mng",
            "video/x-ms-asf",
            "video/x-ms-wmv",
            "video/x-msvideo",
        ]
    }

    fn process(&self, original_path: &Path, dimension: u32) -> anyhow::Result<rthumb::ProviderOut> {
        let thumbnailer = ffmpegthumbnailer_rs::ThumbnailerBuilder::default()
            .size(dimension)
            .seek_percentage(self.seek_percentage)
            .unwrap()
            .with_film_strip(self.with_film_strips)
            .build();
        let frame = self
            .runtime
            .block_on(async { thumbnailer.process_to_video_frame(original_path).await })?;
        Ok(rthumb::ProviderOut {
            width: frame.width,
            height: frame.height,
            source_width: frame.source_width,
            source_height: frame.source_height,
            data: frame.data,
        })
    }
}
