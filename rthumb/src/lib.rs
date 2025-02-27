use std::{
    fmt,
    ops::Deref,
    os::linux::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};
use png::text_metadata::TEXtChunk;

use itertools::{Either, Itertools};
use rayon::iter::{
    IndexedParallelIterator, IntoParallelIterator, ParallelIterator,
};

#[derive(Clone)]
pub struct MediaRef {
    pub uri: String,
    pub mime_type: String,
}

pub struct ThumbJobBatch {
    pub handle: u32,
    pub flavor: ThumbFlavor,
    pub medias: Vec<MediaRef>,
}

pub struct ThumbJob {
    pub handle: u32,
    pub flavor: ThumbFlavor,
    pub media: MediaRef,
}

impl fmt::Debug for ThumbJobBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThumbJob")
            .field("handle", &self.handle)
            .field("flavor", &self.flavor)
            .field("medias (len)", &self.medias.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ThumbFlavor {
    Normal,
    Large,
    XLarge,
    XXLarge,
}

impl ThumbFlavor {
    pub fn all() -> impl Iterator<Item = ThumbFlavor> {
        [
            ThumbFlavor::Normal,
            ThumbFlavor::Large,
            ThumbFlavor::XLarge,
            ThumbFlavor::XXLarge,
        ]
        .iter()
        .copied()
    }

    pub fn dimension(&self) -> u32 {
        match &self {
            ThumbFlavor::Normal => 128,
            ThumbFlavor::Large => 256,
            ThumbFlavor::XLarge => 512,
            ThumbFlavor::XXLarge => 1024,
        }
    }

    pub fn cache_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(format!("{}", self))
    }
}

impl TryFrom<&str> for ThumbFlavor {
    type Error = std::io::ErrorKind;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "normal" => Ok(Self::Normal),
            "large" => Ok(Self::Large),
            "x-large" => Ok(Self::XLarge),
            "xx-large" => Ok(Self::XXLarge),
            _ => Err(std::io::ErrorKind::InvalidInput),
        }
    }
}

impl fmt::Display for ThumbFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ThumbFlavor::Normal => "normal",
                ThumbFlavor::Large => "large",
                ThumbFlavor::XLarge => "x-large",
                ThumbFlavor::XXLarge => "xx-large",
            }
        )
    }
}

pub type Successes = Vec<MediaRef>;
pub type Failures = Vec<(MediaRef, String)>;

pub trait Provider: Send + Sync {
    // Returns some descriptive name for logging.
    fn name(&self) -> &'static str;

    // Returns the list of supported mime types.
    fn supported_mime_types(&self) -> Vec<&'static str>;

    // Processes the singular job.
    fn process(&self, opaque: usize, cache_dir: &Path, job: ThumbJob) -> anyhow::Result<()>;
}

pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
    mime_type_map: std::collections::HashMap<String, usize>,
    cache_dir: PathBuf,
}

impl ProviderRegistry {
    fn get_provider(&self, mime_type: &str) -> Option<&(dyn Provider + 'static)> {
        let idx = self.mime_type_map.get(mime_type)?;
        Some(self.providers[*idx].as_ref())
    }

    fn process_batch_sequentially(
        provider: &dyn Provider,
        cache_dir: &Path,
        ThumbJobBatch {
            flavor,
            handle,
            medias,
        }: ThumbJobBatch,
    ) -> (Successes, Failures) {
        medias
            // .into_iter()
            // MOAR CONCURRENCY
            .into_par_iter()
            .enumerate()
            .partition_map(|(opaque, media)| {
                let media_to_return = media.clone();
                let job = ThumbJob {
                    flavor,
                    handle,
                    media,
                };
                match provider.process(opaque, cache_dir, job) {
                    Ok(_) => Either::Left(media_to_return),
                    Err(err) => Either::Right((media_to_return, err.to_string())),
                }
            })
    }

    pub fn process_request(
        &self,
        ThumbJobBatch {
            flavor,
            handle,
            medias,
        }: ThumbJobBatch,
    ) -> (Successes, Failures) {
        let chunked_by_mime: Vec<(String, ThumbJobBatch)> = medias
            .into_iter()
            .into_group_map_by(|m| m.mime_type.clone())
            .into_iter()
            .flat_map(|(mime_type, medias)| {
                medias
                    .into_iter()
                    .rev()
                    .chunks(2)
                    .into_iter()
                    .map(|chunk| {
                        (
                            mime_type.clone(),
                            ThumbJobBatch {
                                flavor,
                                handle,
                                medias: chunk.collect(),
                            },
                        )
                    })
                    .collect_vec()
            })
            .collect();

        let cache_dir = self.cache_dir.clone();
        let (all_successes, all_failures): (Vec<_>, Vec<_>) = chunked_by_mime
            .into_par_iter()
            .map(|(mime_type, sub_job)| {
                if let Some(provider) = self.get_provider(&mime_type) {
                    // eprintln!("using '{}' provider for '{mime_type}'", provider.name());
                    Self::process_batch_sequentially(provider, &cache_dir, sub_job)
                } else {
                    // FIXME: all into failures
                    (vec![], vec![])
                }
            })
            .collect();
        let all_successes = all_successes.into_iter().flatten().collect();
        let all_failures = all_failures.into_iter().flatten().collect();
        (all_successes, all_failures)
    }

    pub fn supported_mime_types(&self) -> impl Iterator<Item = &'static str> {
        self.providers
            .iter()
            .flat_map(|p| p.supported_mime_types())
            .sorted()
    }
}

pub struct ProviderRegistryBuilder {
    providers: Vec<Box<dyn Provider>>,
    cache_dir: PathBuf,
}

impl ProviderRegistryBuilder {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            providers: Vec::new(),
            cache_dir: cache_dir.into(),
        }
    }

    pub fn register<P: Provider + 'static>(&mut self, provider: P) -> &mut Self {
        self.providers.push(Box::new(provider));
        self
    }

    pub fn build(self) -> ProviderRegistry {
        let mut mime_type_map = std::collections::HashMap::new();
        for (idx, provider) in self.providers.iter().enumerate() {
            for mime_type in provider.supported_mime_types() {
                mime_type_map.entry(mime_type.to_owned()).or_insert(idx);
            }
        }
        ProviderRegistry {
            providers: self.providers,
            mime_type_map,
            cache_dir: self.cache_dir,
        }
    }
}

/// Macro to register providers based on features
#[macro_export]
macro_rules! register_providers {
    ($registry:expr, $(#[cfg(feature = $feature:literal)] $provider:expr),* $(,)?) => {
        $(
            #[cfg(feature = $feature)]
            $registry.register($provider);
        )*
    };
}

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
