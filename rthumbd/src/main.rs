use std::path::{Path, PathBuf};

use anyhow::anyhow;
use image::EncodableLayout;
use itertools::{
    Either::{Left, Right},
    Itertools,
};
use log::{debug, info, warn};
use rayon::iter::ParallelIterator;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator};
use rthumbd::{
    dbus::{self, MediaRef, Reply, ThumbFlavor},
    xdg::{
        ThumbFsMeta, ThumbFullMeta, cache_destination, destination_filename,
        get_thumb_original_metadata, temp_filename, write_thumb_with_original_metadata,
    },
};
use tokio::sync::mpsc;

fn process_item(
    id: usize,
    cache_dir: &Path,
    flavor: &ThumbFlavor,
    media: &MediaRef,
) -> anyhow::Result<()> {
    let original_path = match url::Url::parse(&media.uri)?.to_file_path() {
        Ok(path) => path,
        Err(_) => return Err(anyhow!("not a file://")),
    };
    let cache_dir = flavor.cache_path(cache_dir);
    let original_meta = ThumbFsMeta::from(&media.uri, &original_path)?;
    let thumb_path = destination_filename(&cache_dir, &media.uri);
    // Bail cheaply if already on disk & no changes.
    if let Ok(existing_original_meta) = get_thumb_original_metadata(&thumb_path) {
        if existing_original_meta == original_meta {
            debug!("cache hit for {}", &media.uri);
            return Ok(());
        }
    }
    let (orig_width, orig_height, thumb) = {
        let im = image::open(&original_path)?;
        let dimension = flavor.dimension();
        (
            im.width(),
            im.height(),
            im.thumbnail(dimension, dimension).to_rgb8(),
        )
    };
    let original_meta = ThumbFullMeta::from(original_meta, orig_width, orig_height);
    let temp_thumb_path = temp_filename(&cache_dir, &media.uri, id);
    write_thumb_with_original_metadata(
        &temp_thumb_path,
        &original_meta,
        thumb.width(),
        thumb.height(),
        thumb.as_bytes(),
    )?;
    std::fs::rename(&temp_thumb_path, &thumb_path)?;
    Ok(())
}

type Successes<'a> = Vec<&'a MediaRef>;
type Failures<'a> = Vec<(&'a MediaRef, String)>;

fn process_chunk_concurrently<'a>(
    cache_dir: &Path,
    flavor: &ThumbFlavor,
    chunk: &'a Vec<MediaRef>,
) -> (Successes<'a>, Failures<'a>) {
    chunk.par_iter().enumerate().partition_map(|(i, media)| {
        match process_item(i, cache_dir, flavor, media) {
            Ok(_) => Left(media),
            Err(err) => Right((media, err.to_string())),
        }
    })
}

fn send_results(
    handle: u32,
    successes: Successes,
    failures: Failures,
    tx: mpsc::Sender<Reply>,
) -> anyhow::Result<()> {
    tx.blocking_send(Reply::Ready {
        handle,
        uris: successes
            .into_iter()
            .map(|media| media.uri.clone())
            .collect(),
    })?;
    for (media, message) in failures {
        warn!("error creating thumbnail for {}: {}", &media.uri, &message);
        tx.blocking_send(Reply::Error {
            handle,
            uri: media.uri.clone(),
            message,
        })?;
    }
    Ok(())
}

fn process_chunk_and_reply(
    cache_dir: &Path,
    handle: u32,
    flavor: &ThumbFlavor,
    chunk: Vec<MediaRef>,
    tx: mpsc::Sender<Reply>,
) -> anyhow::Result<()> {
    let (successes, failures) = process_chunk_concurrently(cache_dir, flavor, &chunk);
    send_results(handle, successes, failures, tx)
}

async fn create_cache_dir_for_flavor(
    flavor: ThumbFlavor,
    cache_dir: PathBuf,
) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || std::fs::create_dir_all(flavor.cache_path(&cache_dir)))
        .await??;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let chunk_size: usize = std::env::var("RTHUMB_CHUNK_SIZE")
        .unwrap_or_default()
        .parse()
        .unwrap_or(2);

    let cache_dir = cache_destination()?;
    info!("using chunk size: {chunk_size:?}");
    info!("using cache directory: {cache_dir:?}");

    let (mut rx, tx) = dbus::Thumbnailer1::create_and_listen().await?;

    _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    info!("successfully installed DBus service");

    while let Some(req) = rx.recv().await {
        info!("new thumbnail request: {req:?}");
        create_cache_dir_for_flavor(req.flavor, cache_dir.clone()).await?;
        let handle = req.handle;
        let mut handles: Vec<_> = Vec::new();
        for chunk in &req.medias.into_iter().rev().chunks(chunk_size) {
            let cache_dir = cache_dir.clone();
            let tx = tx.clone();
            let chunk: Vec<_> = chunk.collect();
            handles.push(tokio::task::spawn_blocking(move || {
                process_chunk_and_reply(&cache_dir, handle, &req.flavor, chunk, tx)
            }));
        }
        for h in handles {
            h.await??;
        }
        tx.send(Reply::Finished { handle }).await?;
    }

    std::panic!()
}
