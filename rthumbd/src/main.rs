use std::{path::Path, sync::Arc};

use log::{info, warn};
use rthumb::{cache_destination, Failures, Successes, ThumbFlavor};
use rthumbd::dbus::Reply;
use tokio::sync::mpsc;

async fn send_results(
    handle: u32,
    successes: Successes,
    failures: Failures,
    tx: mpsc::Sender<Reply>,
) -> anyhow::Result<()> {
    tx.send(Reply::Ready {
        handle,
        uris: successes
            .into_iter()
            .map(|media| media.uri.clone())
            .collect(),
    })
    .await?;
    for (media, message) in failures {
        warn!("error creating thumbnail for {}: {}", &media.uri, &message);
        tx.send(Reply::Error {
            handle,
            uri: media.uri.clone(),
            message,
        })
        .await?;
    }
    Ok(())
}

async fn create_cache_dir_for_flavor(flavor: ThumbFlavor, cache_dir: &Path) -> anyhow::Result<()> {
    let cache_dir = cache_dir.to_owned();
    tokio::task::spawn_blocking(move || std::fs::create_dir_all(flavor.cache_path(&cache_dir)))
        .await??;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cache_dir = cache_destination()?;

    let mut registry_builder = rthumb::ProviderRegistryBuilder::new(&cache_dir);
    rthumb::register_providers!(
        registry_builder,
        #[cfg(feature = "image")]
        rthumb_image::ImageProvider::new(),
        // #[cfg(feature = "video")] VideoProvider::new(),
    );
    let registry = Arc::new(registry_builder.build());

    let chunk_size: usize = std::env::var("RTHUMB_CHUNK_SIZE")
        .unwrap_or_default()
        .parse()
        .unwrap_or(2);

    info!("using chunk size: {chunk_size:?}");
    info!("using cache directory: {cache_dir:?}");

    let (mut rx, tx) = rthumbd::dbus::Thumbnailer1::create_and_listen(registry.clone()).await?;

    _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    info!("successfully installed DBus service");

    while let Some(req) = rx.recv().await {
        info!("new thumbnail request: {req:?}");
        create_cache_dir_for_flavor(req.flavor, &cache_dir).await?;
        let handle = req.handle;
        let registry = registry.clone();
        let (successes, failures) =
            tokio::task::spawn_blocking(move || registry.process_request(req)).await?;
        send_results(handle, successes, failures, tx.clone()).await?;
        tx.send(Reply::Finished { handle }).await?;
    }

    std::panic!()
}
