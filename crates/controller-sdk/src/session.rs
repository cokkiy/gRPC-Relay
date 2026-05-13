use crate::error::{ControllerSdkError, Result};
use bytes::Bytes;
use std::collections::HashMap;
use tokio::sync::{oneshot, Mutex};

pub type SequenceResult = std::result::Result<Bytes, ControllerSdkError>;

#[derive(Debug, Default)]
pub struct PendingRequests {
    inner: Mutex<HashMap<i64, oneshot::Sender<SequenceResult>>>,
}

impl PendingRequests {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub async fn insert(&self, sequence_number: i64) -> Result<oneshot::Receiver<SequenceResult>> {
        let (tx, rx) = oneshot::channel::<SequenceResult>();
        let previous_tx = {
            let mut guard = self.inner.lock().await;
            guard.insert(sequence_number, tx)
        };

        if let Some(previous_tx) = previous_tx {
            drop(previous_tx);
        }

        Ok(rx)
    }

    pub async fn remove(&self, sequence_number: i64) {
        let mut guard = self.inner.lock().await;
        guard.remove(&sequence_number);
    }

    pub async fn complete(&self, sequence_number: i64, result: SequenceResult) {
        let maybe_tx = {
            let mut guard = self.inner.lock().await;
            guard.remove(&sequence_number)
        };

        if let Some(tx) = maybe_tx {
            let _ = tx.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_seq_insert_and_complete() {
        let pending = PendingRequests::new();
        let rx = pending.insert(1).await.unwrap();
        pending.complete(1, Ok(Bytes::from("hello"))).await;
        let result = rx.await.unwrap().unwrap();
        assert_eq!(result, Bytes::from("hello"));
    }

    #[tokio::test]
    async fn session_seq_increments_correctly() {
        let pending = PendingRequests::new();
        let rx1 = pending.insert(1).await.unwrap();
        let rx2 = pending.insert(2).await.unwrap();
        pending.complete(1, Ok(Bytes::from("resp1"))).await;
        pending.complete(2, Ok(Bytes::from("resp2"))).await;
        assert_eq!(rx1.await.unwrap().unwrap(), Bytes::from("resp1"));
        assert_eq!(rx2.await.unwrap().unwrap(), Bytes::from("resp2"));
    }

    #[tokio::test]
    async fn session_seq_remove_after_complete() {
        let pending = PendingRequests::new();
        let _rx = pending.insert(1).await.unwrap();
        pending.complete(1, Ok(Bytes::from("done"))).await;
        // After completion, the entry is removed from the map
        // Subsequent remove should not panic
        pending.remove(1).await;
    }
}
