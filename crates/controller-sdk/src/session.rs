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
