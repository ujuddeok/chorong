pub struct Queue {
    tx: Sender<Job>,
    rx: Receiver<Job>,
}

impl Queue {
    pub fn new(size: usize) -> Queue {
        let (tx, rx) = bounded(size);
        Queue { tx, rx }
    }

    pub fn push(&self, job: Job) {
        self.tx.send(job).expect("queue is open");
    }

    pub fn pop(&self) -> Option<Job> {
        self.rx.recv().ok()
    }
}
