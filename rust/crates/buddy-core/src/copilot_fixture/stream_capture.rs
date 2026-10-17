//! A bounded handshake for the offline CI provider, after its first real delta.
use super::*;

#[derive(Clone)]
pub(super) struct StreamCapture {
    path: PathBuf,
    root: PathBuf,
}
impl StreamCapture {
    pub(super) fn new(path: PathBuf, root: &Path) -> Result<Self, Failure> {
        if !contained(&path, root) || path.extension().is_none_or(|ext| ext != "png") {
            return Err(Failure::InvalidContext);
        }
        Ok(Self {
            path,
            root: root.to_owned(),
        })
    }
    pub(super) fn pending(&self) -> bool {
        self.path
            .symlink_metadata()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    }
    pub(super) async fn wait(
        &self,
        cancelled: impl std::future::Future<Output = ()>,
        limit: Duration,
    ) -> bool {
        tokio::select! {
            _ = cancelled => false,
            result = tokio::time::timeout(limit, async {
                loop {
                    if self.path.canonicalize().is_ok_and(|path| path.starts_with(&self.root)
                        && path.metadata().is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }) => result.is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn capture_must_arrive_before_the_provider_releases_the_remaining_deltas() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("streaming.png");
        let capture = StreamCapture::new(path.clone(), &root).unwrap();
        assert!(capture.pending());
        assert!(
            !capture
                .wait(std::future::pending(), Duration::from_millis(10))
                .await
        );
        std::fs::write(&path, []).unwrap();
        let mut waiting = Box::pin(capture.wait(std::future::pending(), Duration::from_secs(1)));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut waiting)
                .await
                .is_err()
        );
        std::fs::write(&path, b"synthetic capture acknowledgment").unwrap();
        assert!(waiting.await);
        assert!(!StreamCapture::new(path, &root).unwrap().pending());
    }
    #[tokio::test]
    async fn cancellation_does_not_wait_for_an_image_and_paths_remain_owned() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let capture = StreamCapture::new(root.join("streaming.png"), &root).unwrap();
        assert!(
            !capture
                .wait(std::future::ready(()), Duration::from_secs(60))
                .await
        );
        assert!(StreamCapture::new(root.join("../outside.png"), &root).is_err());
        assert!(StreamCapture::new(PathBuf::from("relative.png"), &root).is_err());
        assert!(StreamCapture::new(root.join("wrong.json"), &root).is_err());
    }
}
