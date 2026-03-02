use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::clock;
use crate::connection::{ConnectionPool, extract_pop, get_cf_ray};
use crate::metrics::TimestampRecord;
use crate::request::RequestTemplate;
use crate::trigger::TriggerMessage;

/// Ingress thread: receives triggers and pushes to queue with timestamp.
pub struct IngressThread {
    tx: Sender<TriggerMessage>,
}

impl IngressThread {
    pub fn new(tx: Sender<TriggerMessage>) -> Self {
        Self { tx }
    }

    /// Inject a trigger message, stamping t_trigger_rx.
    pub fn inject(&self, mut msg: TriggerMessage) -> Result<(), String> {
        msg.timestamp_ns = clock::now_ns();
        self.tx
            .try_send(msg)
            .map_err(|e| format!("queue full: {}", e))
    }
}

/// Execution thread: dequeues triggers, patches request, sends on warm H2 connection.
pub struct ExecutionThread {
    rx: Receiver<TriggerMessage>,
    running: Arc<AtomicBool>,
    records: Arc<Mutex<Vec<TimestampRecord>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ExecutionThread {
    pub fn new(rx: Receiver<TriggerMessage>) -> Self {
        Self {
            rx,
            running: Arc::new(AtomicBool::new(false)),
            records: Arc::new(Mutex::new(Vec::new())),
            handle: None,
        }
    }

    /// Process a single trigger synchronously (for testing or single-shot mode).
    pub fn process_one(
        pool: &ConnectionPool,
        template: &mut RequestTemplate,
        msg: &TriggerMessage,
        rt: &tokio::runtime::Runtime,
    ) -> TimestampRecord {
        let mut rec = TimestampRecord::default();
        rec.t_trigger_rx = msg.timestamp_ns;
        rec.t_dispatch_q = clock::now_ns();

        rec.t_exec_start = clock::now_ns();

        // Patch template with trigger data (price as a simple example)
        // For now, just build the request from the template
        rec.t_buf_ready = clock::now_ns();

        let req = template.build_request();
        rec.t_write_begin = clock::now_ns();

        let result = rt.block_on(async {
            pool.send(req).await
        });

        rec.t_write_end = clock::now_ns();

        match result {
            Ok(resp) => {
                rec.t_first_resp_byte = clock::now_ns();
                if let Some(cf_ray) = get_cf_ray(&resp) {
                    rec.cf_ray_pop = extract_pop(&cf_ray);
                }
                rec.t_headers_done = clock::now_ns();
                rec.is_reconnect = false;
            }
            Err(_e) => {
                rec.t_first_resp_byte = clock::now_ns();
                rec.t_headers_done = clock::now_ns();
                rec.is_reconnect = true;
            }
        }

        rec
    }

    /// Start the execution thread with a pool and template.
    pub fn start(&mut self, pool: Arc<ConnectionPool>, mut template: RequestTemplate) {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let rx = self.rx.clone();
        let records = self.records.clone();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");

            while running.load(Ordering::SeqCst) {
                match rx.try_recv() {
                    Ok(msg) => {
                        let rec = Self::process_one(&pool, &mut template, &msg, &rt);
                        records.lock().unwrap().push(rec);
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        thread::yield_now();
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                }
            }
        });
        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    pub fn get_records(&self) -> Vec<TimestampRecord> {
        self.records.lock().unwrap().clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for ExecutionThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Maintenance thread: periodic health checks and reconnection.
pub struct MaintenanceThread {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    health_check_count: Arc<AtomicUsize>,
    reconnect_count: Arc<AtomicUsize>,
}

use std::sync::atomic::AtomicUsize;

impl MaintenanceThread {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            health_check_count: Arc::new(AtomicUsize::new(0)),
            reconnect_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn start(&mut self, pool: Arc<ConnectionPool>, interval: Duration) {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let hc_count = self.health_check_count.clone();
        let rc_count = self.reconnect_count.clone();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");

            while running.load(Ordering::SeqCst) {
                // Sleep in small increments for responsive shutdown
                let sleep_step = Duration::from_millis(50);
                let mut elapsed = Duration::ZERO;
                while elapsed < interval && running.load(Ordering::SeqCst) {
                    thread::sleep(sleep_step);
                    elapsed += sleep_step;
                }
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let healthy = rt.block_on(async { pool.health_check().await });
                hc_count.fetch_add(1, Ordering::Relaxed);

                if healthy < pool.pool_size() {
                    rc_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    pub fn health_check_count(&self) -> usize {
        self.health_check_count.load(Ordering::Relaxed)
    }

    pub fn reconnect_count(&self) -> usize {
        self.reconnect_count.load(Ordering::Relaxed)
    }
}

impl Drop for MaintenanceThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// CPU pinning (Linux only, returns false on other platforms).
pub fn pin_to_core(core_id: usize) -> bool {
    let core_ids = core_affinity::get_core_ids().unwrap_or_default();
    if let Some(id) = core_ids.get(core_id) {
        core_affinity::set_for_current(*id)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::AddressFamily;
    use crate::queue::TriggerQueue;
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
    fn ingress_stamps_timestamp() {
        let q = TriggerQueue::new();
        let ingress = IngressThread::new(q.sender());
        ingress.inject(make_trigger(1)).unwrap();
        let msg = q.try_recv().unwrap();
        assert!(msg.timestamp_ns > 0);
    }

    #[test]
    fn ingress_delivers_to_queue() {
        let q = TriggerQueue::new();
        let ingress = IngressThread::new(q.sender());
        for i in 0..5 {
            ingress.inject(make_trigger(i)).unwrap();
        }
        assert_eq!(q.len(), 5);
    }

    #[tokio::test]
    async fn process_one_populates_timestamps() {
        let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
        pool.warmup().await.expect("warmup failed");
        let pool = Arc::new(pool);

        let mut template = RequestTemplate::new(
            http::Method::GET,
            "/".parse().unwrap(),
        );
        template.add_header("host", "clob.polymarket.com");

        let mut msg = make_trigger(1);
        msg.timestamp_ns = clock::now_ns();

        // process_one needs a sync Runtime, run in blocking thread
        let pool_clone = pool.clone();
        let rec = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            ExecutionThread::process_one(&pool_clone, &mut template, &msg, &rt)
        })
        .await
        .unwrap();

        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_exec_start > 0);
        assert!(rec.t_write_begin > 0);
        assert!(rec.t_write_end > 0);
        assert!(rec.t_first_resp_byte > 0);
        assert!(rec.t_headers_done > 0);
        assert!(!rec.cf_ray_pop.is_empty(), "POP should be extracted");
    }

    #[test]
    fn cpu_pin_does_not_panic() {
        // May return true or false depending on platform
        let _ = pin_to_core(0);
    }

    #[test]
    fn maintenance_thread_new() {
        let mt = MaintenanceThread::new();
        assert_eq!(mt.health_check_count(), 0);
        assert_eq!(mt.reconnect_count(), 0);
    }

    #[tokio::test]
    async fn end_to_end_pipeline() {
        // Full pipeline: ingress → queue → execution thread → response
        let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
        pool.warmup().await.expect("warmup failed");
        let pool = Arc::new(pool);

        let q = TriggerQueue::new();
        let ingress = IngressThread::new(q.sender());

        let mut template = RequestTemplate::new(
            http::Method::GET,
            "/".parse().unwrap(),
        );
        template.add_header("host", "clob.polymarket.com");

        let mut exec = ExecutionThread::new(q.receiver());
        exec.start(pool.clone(), template);

        // Inject a trigger
        ingress.inject(make_trigger(1)).unwrap();

        // Wait for processing
        tokio::time::sleep(Duration::from_secs(2)).await;

        exec.stop();
        let records = exec.get_records();
        assert_eq!(records.len(), 1, "should have 1 record");
        let rec = &records[0];

        // All timestamps should be populated
        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_exec_start >= rec.t_trigger_rx);
        assert!(rec.t_write_begin >= rec.t_exec_start);
        assert!(rec.t_first_resp_byte >= rec.t_write_begin);
        assert!(!rec.cf_ray_pop.is_empty(), "POP should be extracted");

        // Trigger-to-wire should be reasonable (< 10ms for warm connection)
        let ttw = rec.trigger_to_wire();
        assert!(ttw < 10_000_000, "trigger_to_wire {} ns too high", ttw);
    }
}
