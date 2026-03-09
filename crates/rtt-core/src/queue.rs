use crate::trigger::TriggerMessage;
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};

pub const QUEUE_CAPACITY: usize = 1024;

pub struct TriggerQueue {
    tx: Sender<TriggerMessage>,
    rx: Receiver<TriggerMessage>,
}

impl TriggerQueue {
    pub fn new() -> Self {
        let (tx, rx) = bounded(QUEUE_CAPACITY);
        Self { tx, rx }
    }

    pub fn sender(&self) -> Sender<TriggerMessage> {
        self.tx.clone()
    }

    pub fn receiver(&self) -> Receiver<TriggerMessage> {
        self.rx.clone()
    }

    pub fn try_send(&self, msg: TriggerMessage) -> Result<(), TrySendError<TriggerMessage>> {
        self.tx.try_send(msg)
    }

    pub fn try_recv(&self) -> Result<TriggerMessage, TryRecvError> {
        self.rx.try_recv()
    }

    pub fn len(&self) -> usize {
        self.rx.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{OrderType, Side};

    fn make_trigger(id: u64) -> TriggerMessage {
        TriggerMessage {
            trigger_id: id,
            token_id: "tok".to_string(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 0,
        }
    }

    #[test]
    fn push_pop() {
        let q = TriggerQueue::new();
        q.try_send(make_trigger(1)).unwrap();
        let msg = q.try_recv().unwrap();
        assert_eq!(msg.trigger_id, 1);
    }

    #[test]
    fn empty_pop_fails() {
        let q = TriggerQueue::new();
        assert!(q.try_recv().is_err());
    }

    #[test]
    fn fifo_order() {
        let q = TriggerQueue::new();
        for i in 0..10 {
            q.try_send(make_trigger(i)).unwrap();
        }
        for i in 0..10 {
            assert_eq!(q.try_recv().unwrap().trigger_id, i);
        }
    }

    #[test]
    fn capacity_limit() {
        let q = TriggerQueue::new();
        for i in 0..QUEUE_CAPACITY as u64 {
            q.try_send(make_trigger(i)).unwrap();
        }
        // Queue is full
        assert!(q.try_send(make_trigger(9999)).is_err());
        assert_eq!(q.len(), QUEUE_CAPACITY);
    }

    #[test]
    fn concurrent_producer_consumer() {
        let q = TriggerQueue::new();
        let tx = q.sender();
        let rx = q.receiver();
        let count = 10_000u64;

        let producer = std::thread::spawn(move || {
            for i in 0..count {
                loop {
                    match tx.try_send(make_trigger(i)) {
                        Ok(()) => break,
                        Err(TrySendError::Full(_)) => std::thread::yield_now(),
                        Err(e) => panic!("send error: {}", e),
                    }
                }
            }
        });

        let consumer = std::thread::spawn(move || {
            let mut received = Vec::with_capacity(count as usize);
            while received.len() < count as usize {
                match rx.try_recv() {
                    Ok(msg) => received.push(msg.trigger_id),
                    Err(TryRecvError::Empty) => std::thread::yield_now(),
                    Err(e) => panic!("recv error: {}", e),
                }
            }
            received
        });

        producer.join().unwrap();
        let received = consumer.join().unwrap();
        assert_eq!(received.len(), count as usize);
        // Verify FIFO order
        for (i, id) in received.iter().enumerate() {
            assert_eq!(*id, i as u64);
        }
    }
}
