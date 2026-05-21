use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, MutexGuard};

#[derive(Clone)]
pub(super) struct PendingVideoFrame {
    pub(super) seq: u64,
    pub(super) source_width: u32,
    pub(super) source_height: u32,
    pub(super) image_width: u32,
    pub(super) image_height: u32,
    pub(super) format: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Default)]
pub(super) struct VideoFrameCoalescer {
    state: Mutex<VideoFrameCoalescerState>,
}

#[derive(Default)]
struct VideoFrameCoalescerState {
    frames: HashMap<VideoFrameKey, (VideoSource, PendingVideoFrame)>,
    queued: HashSet<VideoFrameKey>,
}

impl VideoFrameCoalescer {
    pub(super) fn push(
        &self,
        client_id: String,
        source: VideoSource,
        frame: PendingVideoFrame,
    ) -> bool {
        let key = VideoFrameKey::new(&client_id, &source);
        let Ok(mut state) = self.state.lock() else {
            return true;
        };
        state.frames.insert(key.clone(), (source, frame));
        state.queued.insert(key)
    }

    pub(super) fn take(&self, client_id: &str, source: &VideoSource) -> Option<PendingVideoFrame> {
        let key = VideoFrameKey::new(client_id, source);
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        state.queued.remove(&key);
        state.frames.remove(&key).map(|(_, frame)| frame)
    }
}

#[derive(Default)]
pub(super) struct VideoDecodeWorkers {
    workers: HashMap<VideoFrameKey, VideoDecodeWorker>,
}

impl VideoDecodeWorkers {
    fn submit(
        &mut self,
        event_tx: Sender<AdminEvent>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
        client_id: String,
        source: VideoSource,
        frame: PendingVideoFrame,
    ) {
        let key = VideoFrameKey::new(&client_id, &source);
        let worker = self.workers.entry(key).or_insert_with(|| {
            VideoDecodeWorker::spawn(event_tx, repaint_handle, client_id, source)
        });
        worker.submit(frame);
    }
}

impl Drop for VideoDecodeWorkers {
    fn drop(&mut self) {
        for worker in self.workers.values() {
            worker.stop();
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct VideoFrameKey {
    client_id: String,
    source: u8,
}

impl VideoFrameKey {
    fn new(client_id: &str, source: &VideoSource) -> Self {
        Self {
            client_id: client_id.to_string(),
            source: video_source_key(source),
        }
    }
}

fn video_source_key(source: &VideoSource) -> u8 {
    match source {
        VideoSource::RemoteDesktop => 1,
        VideoSource::Camera => 2,
    }
}

struct VideoDecodeWorker {
    shared: Arc<VideoDecodeShared>,
}

struct VideoDecodeShared {
    state: Mutex<VideoDecodeState>,
    ready: Condvar,
}

#[derive(Default)]
struct VideoDecodeState {
    latest: Option<PendingVideoFrame>,
    stopped: bool,
}

impl VideoDecodeWorker {
    fn spawn(
        event_tx: Sender<AdminEvent>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
        client_id: String,
        source: VideoSource,
    ) -> Self {
        let shared = Arc::new(VideoDecodeShared {
            state: Mutex::new(VideoDecodeState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = shared.clone();
        thread::spawn(move || {
            video_decode_worker_loop(event_tx, repaint_handle, client_id, source, worker_shared);
        });
        Self { shared }
    }

    fn submit(&self, frame: PendingVideoFrame) {
        let Ok(mut state) = self.shared.state.lock() else {
            return;
        };
        state.latest = Some(frame);
        self.shared.ready.notify_one();
    }

    fn stop(&self) {
        let Ok(mut state) = self.shared.state.lock() else {
            return;
        };
        state.stopped = true;
        state.latest = None;
        self.shared.ready.notify_all();
    }
}

fn video_decode_worker_loop(
    event_tx: Sender<AdminEvent>,
    repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    client_id: String,
    source: VideoSource,
    shared: Arc<VideoDecodeShared>,
) {
    let sink = AdminEventSink::new(event_tx, Some(repaint_handle), None);
    let mut desktop_decode_state = live_control::remote_desktop::DesktopFrameDecodeState::default();
    loop {
        let Some(frame) = wait_for_latest_video_frame(&shared) else {
            break;
        };
        let decode_started = Instant::now();
        match source {
            VideoSource::RemoteDesktop => {
                let result = live_control::remote_desktop::decode_video_frame_with_state(
                    &mut desktop_decode_state,
                    frame.seq,
                    frame.source_width,
                    frame.source_height,
                    frame.image_width,
                    frame.image_height,
                    frame.format,
                    frame.bytes,
                );
                match result {
                    Ok(Some(frame)) => {
                        sink.send(AdminEvent::DecodedDesktopFrame {
                            client_id: client_id.clone(),
                            result: Ok(frame),
                            decode_ms: Some(decode_started.elapsed().as_millis()),
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        sink.send(AdminEvent::DecodedDesktopFrame {
                            client_id: client_id.clone(),
                            result: Err(error),
                            decode_ms: Some(decode_started.elapsed().as_millis()),
                        });
                    }
                }
            }
            VideoSource::Camera => {
                let result = live_control::camera::decode_video_frame(
                    frame.seq,
                    frame.image_width,
                    frame.image_height,
                    frame.format,
                    frame.bytes,
                );
                sink.send(AdminEvent::DecodedCameraFrame {
                    client_id: client_id.clone(),
                    result,
                    decode_ms: Some(decode_started.elapsed().as_millis()),
                });
            }
        }
    }
}

fn wait_for_latest_video_frame(shared: &VideoDecodeShared) -> Option<PendingVideoFrame> {
    let mut state = shared.state.lock().ok()?;
    loop {
        if state.stopped {
            return None;
        }
        if state.latest.is_some() {
            return state.latest.take();
        }
        state = wait_for_video_frame(shared, state)?;
    }
}

fn wait_for_video_frame<'a>(
    shared: &'a VideoDecodeShared,
    state: MutexGuard<'a, VideoDecodeState>,
) -> Option<MutexGuard<'a, VideoDecodeState>> {
    shared.ready.wait(state).ok()
}

impl AdminApp {
    pub(super) fn spawn_camera_frame_decode(&self, client_id: String, payload: String) {
        let sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            None,
        );
        thread::spawn(move || {
            let decode_started = Instant::now();
            let result = live_control::camera::decode_frame_payload(&payload);
            sink.send(AdminEvent::DecodedCameraFrame {
                client_id,
                result,
                decode_ms: Some(decode_started.elapsed().as_millis()),
            });
        });
    }

    pub(super) fn spawn_video_frame_decode(
        &mut self,
        client_id: String,
        source: VideoSource,
        frame: PendingVideoFrame,
    ) {
        self.video_decode_workers.submit(
            self.event_tx.clone(),
            self.repaint_handle.clone(),
            client_id,
            source,
            frame,
        );
    }
}
