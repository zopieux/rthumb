use std::{
    fmt,
    ops::Deref,
    os::linux::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use png::text_metadata::TEXtChunk;

#[derive(Debug)]
pub struct ThumbFsMeta {
    pub uri: String,
    pub mtime_nsec: f64,
    pub size: u64,
}

impl ThumbFsMeta {
    pub fn from(uri: &str, path: &Path) -> anyhow::Result<Self> {
        let file_meta = std::fs::metadata(path)?;
        let mtime_nsec = file_meta.st_mtime() as f64 + file_meta.st_mtime_nsec() as f64 / 1e9;
        let size = file_meta.st_size();
        Ok(Self {
            uri: uri.to_owned(),
            mtime_nsec,
            size,
        })
    }
}

impl PartialEq for ThumbFsMeta {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri
            && self.mtime_nsec == other.mtime_nsec
            && (self.size == 0 || other.size == 0 || self.size == other.size)
    }
}

#[derive(Debug)]
pub struct ThumbFullMeta {
    pub width: u32,
    pub height: u32,
    pub fs: ThumbFsMeta,
}

impl ThumbFullMeta {
    pub fn from(fs: ThumbFsMeta, width: u32, height: u32) -> Self {
        Self { width, height, fs }
    }
}

pub fn write_thumb_with_original_metadata(
    path: &Path,
    meta: &ThumbFullMeta,
    thumb_width: u32,
    thumb_height: u32,
    data: &[u8],
) -> anyhow::Result<()> {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .with_context(|| "open")?;
    let mut encoder = png::Encoder::new(f, thumb_width, thumb_height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_text_chunk(&TEXtChunk::new("Thumb::URI", &meta.fs.uri))?;
    writer.write_text_chunk(&TEXtChunk::new(
        "Thumb::MTime",
        format!("{:.6}", meta.fs.mtime_nsec),
    ))?;
    writer.write_text_chunk(&TEXtChunk::new("Thumb::Size", format!("{}", meta.fs.size)))?;
    writer.write_image_data(data)?;
    Ok(())
}

pub fn get_thumb_original_metadata(path: &Path) -> anyhow::Result<ThumbFsMeta> {
    let decoder = png::Decoder::new(
        std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .with_context(|| "open")?,
    );
    let mut uri = None;
    let mut mtime_nsec = None;
    let mut size = None;
    let info_reader = decoder.read_info()?;
    let png::Info {
        uncompressed_latin1_text,
        ..
    } = info_reader.info();
    for chunk in uncompressed_latin1_text {
        match chunk.keyword.deref() {
            "Thumb::URI" => uri = Some(chunk.text.clone()),
            "Thumb::MTime" => mtime_nsec = chunk.text.parse::<f64>().ok(),
            "Thumb::Size" => size = chunk.text.parse::<u64>().ok(),
            _ => continue,
        }
    }
    Ok(ThumbFsMeta {
        uri: uri.ok_or(anyhow!("missing uri"))?,
        mtime_nsec: mtime_nsec.ok_or(anyhow!("missing mtime_nsec"))?,
        size: size.unwrap_or(0),
    })
}

pub fn destination_filename(dir: &Path, uri: &str) -> PathBuf {
    dir.join(format!("{}.png", uri_hash(uri)))
}

pub fn temp_filename(dir: &Path, uri: &str, id: usize) -> PathBuf {
    dir.join(format!("{}.tmp{}", uri_hash(uri), id))
}

pub fn cache_destination() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("XDG_CACHE_HOME") {
        Ok(PathBuf::from(path).join("thumbnails"))
    } else if let Ok(path) = std::env::var("HOME") {
        Ok(PathBuf::from(path).join(".cache").join("thumbnails"))
    } else {
        Err(anyhow!("both XDG_CACHE_HOME and HOME are unset"))
    }
}

fn uri_hash(uri: &str) -> String {
    format!("{}", HexSlice(&md5::compute(uri).to_vec()))
}

struct HexSlice<'a>(&'a [u8]);

impl fmt::Display for HexSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in self.0 {
            write!(f, "{:0>2x}", byte)?;
        }
        Ok(())
    }
}
