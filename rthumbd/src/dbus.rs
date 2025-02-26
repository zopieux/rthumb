use std::{
    fmt,
    path::{Path, PathBuf},
    sync::atomic,
};

use itertools::Itertools;
use tokio::sync::mpsc;
use zbus::{
    fdo,
    object_server::SignalEmitter,
    zvariant::{self},
};

pub struct MediaRef {
    pub uri: String,
    pub mime_type: String,
}

pub struct ThumbJob {
    pub handle: u32,
    pub flavor: ThumbFlavor,
    pub medias: Vec<MediaRef>,
}

impl fmt::Debug for ThumbJob {
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

pub struct ThumbReply {
    pub handle: u32,
    pub uris: Vec<String>,
}

pub enum Reply {
    Ready {
        handle: u32,
        uris: Vec<String>,
    },
    Finished {
        handle: u32,
    },
    Error {
        handle: u32,
        uri: String,
        message: String,
    },
}

#[derive(zvariant::Type, serde::Serialize)]
struct Supported {
    schemes: Vec<String>,
    mime_types: Vec<String>,
}

pub struct Thumbnailer1 {
    req_tx: mpsc::Sender<ThumbJob>,
    next_handle: atomic::AtomicU32,
}

impl Thumbnailer1 {
    pub async fn create_and_listen()
    -> anyhow::Result<(mpsc::Receiver<ThumbJob>, mpsc::Sender<Reply>)> {
        const WELL_KNOWN_NAME: &str = "org.freedesktop.thumbnails.Thumbnailer1";
        const INTERFACE_PATH: &str = "/org/freedesktop/thumbnails/Thumbnailer1";

        const CHANNEL_CAPACITY: usize = 256;
        let (req_tx, mut req_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (job_tx, job_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (result_tx, mut result_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let dbus_thumbnailer = Self {
            req_tx,
            next_handle: atomic::AtomicU32::new(1),
        };
        let connection = zbus::connection::Builder::session()?
            .name(WELL_KNOWN_NAME)?
            .serve_at(INTERFACE_PATH, dbus_thumbnailer)?
            .build()
            .await?;

        let object_server = connection.object_server();
        let interface = object_server
            .interface::<_, Thumbnailer1>(INTERFACE_PATH)
            .await?;

        let _handle = tokio::spawn(async move {
            let dbus_ctx = interface.signal_emitter();
            loop {
                tokio::select! {
                    Some(job) = req_rx.recv() => {
                        _ = Thumbnailer1::started(dbus_ctx, job.handle).await;
                        _ = job_tx.send(job).await;
                    },
                    Some(res) = result_rx.recv() => match res {
                        Reply::Ready { handle, uris } => _ = Thumbnailer1::ready(dbus_ctx, handle, &uris).await,
                        Reply::Finished { handle } => _ = Thumbnailer1::finished(dbus_ctx, handle).await,
                        Reply::Error { handle, uri, message } => _ = Thumbnailer1::error(dbus_ctx, handle, &uri, 1, &message).await,
                    }
                }
            }
        });

        Ok((job_rx, result_tx))
    }

    fn next_handle(&mut self) -> u32 {
        self.next_handle.fetch_add(1, atomic::Ordering::SeqCst)
    }
}

#[zbus::interface(name = "org.freedesktop.thumbnails.Thumbnailer1")]
impl Thumbnailer1 {
    #[zbus(name = "Queue")]
    async fn queue(
        &mut self,
        uris: Vec<&str>,
        mime_types: Vec<&str>,
        flavor: &str,
        _scheduler: &str,
        _handle_to_unqueue: u32,
    ) -> fdo::Result<u32> {
        let flavor: ThumbFlavor = ThumbFlavor::try_from(flavor)
            .map_err(|_| fdo::Error::InvalidArgs(format!("invalid flavor '{flavor}'")))?;
        let handle = self.next_handle();
        let medias = uris
            .into_iter()
            .zip(mime_types)
            .map(|(uri, mime_type)| MediaRef {
                uri: uri.to_owned(),
                mime_type: mime_type.to_owned(),
            })
            .collect();
        self.req_tx
            .send(ThumbJob {
                handle,
                flavor,
                medias,
            })
            .await
            .map_err(|_| fdo::Error::Failed(format!("could not send job: {handle}")))?;
        Ok(handle)
    }

    #[zbus(name = "Dequeue")]
    async fn dequeue(&self, _handle: u32) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "dequeuing is not implemented".to_owned(),
        ))
    }

    #[zbus(name = "GetSupported")]
    async fn get_supported(&self) -> fdo::Result<Supported> {
        let schemes = vec!["file".to_owned()];
        let mime_types: Vec<_> = image::ImageFormat::all()
            .map(|f| f.to_mime_type().to_owned())
            .chain(
                ["image/vnd.microsoft.icon"]
                    .into_iter()
                    .map(|s| s.to_owned()),
            )
            .dedup()
            .collect();
        let it = schemes.into_iter().cartesian_product(mime_types);
        let schemes = it.clone().map(|(scheme, _)| scheme).collect();
        let mime_types = it.map(|(_, mime_type)| mime_type).collect();
        Ok(Supported {
            schemes,
            mime_types,
        })
    }

    #[zbus(name = "GetFlavors")]
    async fn get_flavors(&self) -> fdo::Result<Vec<String>> {
        Ok(ThumbFlavor::all().map(|f| format!("{f}")).collect())
    }

    #[zbus(signal, name = "Error")]
    pub async fn error(
        emitter: &SignalEmitter<'_>,
        handle: u32,
        uri: &str,
        error_code: i32,
        message: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "Ready")]
    pub async fn ready(
        emitter: &SignalEmitter<'_>,
        handle: u32,
        uri: &[String],
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "Started")]
    pub async fn started(emitter: &SignalEmitter<'_>, handle: u32) -> zbus::Result<()>;

    #[zbus(signal, name = "Finished")]
    pub async fn finished(emitter: &SignalEmitter<'_>, handle: u32) -> zbus::Result<()>;
}
