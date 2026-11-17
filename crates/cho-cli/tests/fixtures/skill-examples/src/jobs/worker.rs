pub struct Worker {
    queue: Queue,
}

impl Worker {
    pub fn start(queue: Queue) -> Worker {
        let worker = Worker { queue };
        spawn(worker.clone(), Worker::run);
        worker
    }

    pub fn run(&self) {
        while let Some(job) = self.queue.pop() {
            self.handle(job);
        }
    }

    fn handle(&self, job: Job) {
        match job {
            Job::Confirm(id) => send_confirmation(id),
        }
    }
}
