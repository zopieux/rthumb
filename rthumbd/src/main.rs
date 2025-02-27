use std::{path::Path, sync::Arc};

use log::{info, warn};
use rthumb::{cache_destination, ThumbFlavor};
use rthumbd::dbus::Reply;

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

    while let Some(job) = rx.recv().await {
        info!("new thumbnail request: {job:?}");
        create_cache_dir_for_flavor(job.flavor, &cache_dir).await?;
        let handle = job.handle;
        let registry = registry.clone();
        let (sync_tx, sync_rx) = std::sync::mpsc::sync_channel(32);
        let tx_blocking = tx.clone();
        let h_comms = tokio::task::spawn_blocking(move || {
            while let Ok(res) = sync_rx.recv() {
                match res {
                    rthumb::JobResult::Success { handle, uris } => {
                        tx_blocking
                            .blocking_send(Reply::Ready { handle, uris })
                            .expect("blocking_send ready");
                    }
                    rthumb::JobResult::Error {
                        handle,
                        uri,
                        message,
                    } => {
                        warn!("error creating thumbnail for {}: {}", &uri, &message);
                        tx_blocking
                            .blocking_send(Reply::Error {
                                handle,
                                uri,
                                message,
                            })
                            .expect("blocking_send error");
                    }
                }
            }
        });
        let h_process = tokio::task::spawn_blocking(move || registry.process_request(job, chunk_size, sync_tx));
        for h in [h_comms, h_process] {
            h.await?;
        }
        tx.send(Reply::Finished { handle }).await?;
    }

    std::panic!()
}
