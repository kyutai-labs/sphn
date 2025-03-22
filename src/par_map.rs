use std::sync::{Arc, Mutex};

struct Sender<T> {
    s: std::sync::mpsc::SyncSender<T>,
    current_len: Arc<std::sync::atomic::AtomicUsize>,
}

impl<T> Sender<T> {
    fn send(&self, t: T) -> Result<(), std::sync::mpsc::SendError<T>> {
        self.current_len.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.s.send(t)
    }
}

struct Receiver<T> {
    r: std::sync::mpsc::Receiver<T>,
    current_len: Arc<std::sync::atomic::AtomicUsize>,
}

impl<T> Receiver<T> {
    fn current_len(&self) -> usize {
        self.current_len.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn recv(&self) -> Result<T, std::sync::mpsc::RecvError> {
        let v = self.r.recv()?;
        self.current_len.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        Ok(v)
    }
}

fn sync_channel<T>(channel_len: usize) -> (Sender<T>, Receiver<T>) {
    let (s, r) = std::sync::mpsc::sync_channel(channel_len);
    let current_len = std::sync::atomic::AtomicUsize::new(0);
    let current_len = Arc::new(current_len);
    let s = Sender { s, current_len: current_len.clone() };
    let r = Receiver { r, current_len };
    (s, r)
}

pub struct ParMap<T> {
    cnt: usize,
    len: usize,
    receivers: Mutex<Vec<Receiver<T>>>,
}

pub fn par_map<
    T: Send + Sync + 'static,
    U: Send + 'static,
    OP: Fn(usize, &T) -> U + Send + Sync + 'static,
>(
    values: Vec<T>,
    nthreads: usize,
    channel_len: usize,
    op: OP,
) -> ParMap<U> {
    let values = Arc::new(values);
    let nthreads = usize::min(nthreads, values.len());
    let mut receivers = Vec::with_capacity(nthreads);
    let op = Arc::new(op);
    for thread_idx in 0..nthreads {
        let values = values.clone();
        let op = op.clone();
        let (sender, receiver) = sync_channel(channel_len);
        receivers.push(receiver);
        let _handle = std::thread::spawn(move || {
            for (index, value) in values.iter().enumerate().skip(thread_idx).step_by(nthreads) {
                let res = op(index, value);
                if sender.send(res).is_err() {
                    break;
                }
            }
        });
    }
    ParMap { cnt: 0, len: values.len(), receivers: Mutex::new(receivers) }
}

pub fn par_range<
    T: Send + Sync + 'static,
    U: Send + 'static,
    INIT: Fn(usize) -> T + Send + Sync + 'static,
    OP: Fn(&mut T) -> U + Send + Sync + 'static,
>(
    len: Option<usize>,
    nthreads: usize,
    channel_len: usize,
    init: INIT,
    op: OP,
) -> ParMap<U> {
    let len = len.unwrap_or(usize::MAX);
    let nthreads = usize::min(len, nthreads);
    let mut receivers = Vec::with_capacity(nthreads);
    let op = Arc::new(op);
    for thread_index in 0..nthreads {
        let op = op.clone();
        let (sender, receiver) = sync_channel(channel_len);
        receivers.push(receiver);
        let mut index = 0;
        let mut t = init(thread_index);
        let _handle = std::thread::spawn(move || loop {
            if len <= index {
                break;
            }
            let res = op(&mut t);
            if sender.send(res).is_err() {
                break;
            }
            index += nthreads;
        });
    }
    ParMap { cnt: 0, len, receivers: receivers.into() }
}

impl<T> Iterator for ParMap<T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        if self.cnt >= self.len {
            None
        } else {
            let receivers = self.receivers.lock().unwrap();
            let thread_idx = self.cnt % receivers.len();
            self.cnt += 1;
            receivers[thread_idx].recv().ok()
        }
    }
}

impl<T> ParMap<T> {
    pub fn buffered_lens(&self) -> Vec<usize> {
        self.receivers.lock().unwrap().iter().map(|r| r.current_len()).collect()
    }
}
